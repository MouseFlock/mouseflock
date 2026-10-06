//! Clipboard offers, requests and transfers between nodes on loopback
//! (spec: clipboard).

mod harness;

use std::sync::Arc;
use std::time::Duration;

use harness::*;
use model::{InputAction, Key, PeerId, PeerMessage};
use pairing::{Identity, Machine, MachineName};
use proto::clipboard::{self, ClipboardMessage, MAX_TEXT_LEN};
use proto::hello::Hello;
use proto::input::{InputMessage, KeyInput, Mark};
use tokio::time::{Instant, timeout};
use transport::tls::{ALPN_MEMBER, ALPN_PAIR, Expect};
use transport::{Event, records_done_frame};

const WAIT: Duration = Duration::from_secs(10);

fn offer_from(peer: PeerId) -> impl Fn(&Event) -> bool {
    move |e| matches!(e, Event::ClipboardOffer { peer: p, .. } if *p == peer)
}

fn text_from(peer: PeerId) -> impl Fn(&Event) -> bool {
    move |e| matches!(e, Event::ClipboardText { peer: p, .. } if *p == peer)
}

/// A raw client that `a`'s machine counts as a member, and `a` started.
fn member_client(name: &str) -> (RawClient, Node) {
    let client = RawClient::new();
    let identity = Identity::from_file(&*client.identity.to_file()).unwrap();
    let mut theirs = Machine::new(identity, MachineName::new("Raw").unwrap());
    let mut mine = machine(name);
    befriend(&mut mine, &mut theirs);
    (client, node(mine))
}

fn hello() -> Vec<u8> {
    let mut out = Vec::new();
    Hello::this_release().encode_frame(&mut out).unwrap();
    out
}

fn transfer(id: u64, text: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    clipboard::encode_data(id, text.len() as u32, &mut out).unwrap();
    for chunk in text.chunks(proto::frame::MAX_FRAME_PAYLOAD) {
        clipboard::encode_chunk(chunk, &mut out).unwrap();
    }
    out
}

/// A raw member connection to `a` whose records are in, so `a` counts the
/// client as up. Returns the connection and both halves of its control
/// stream; dropping the receive half would tell `a` to stop sending on it.
async fn up_client(
    client: &RawClient,
    a: &mut Node,
) -> (quinn::Connection, quinn::SendStream, quinn::RecvStream) {
    let connection = client
        .connect(a.addr, Expect::Peer(a.id), ALPN_MEMBER)
        .await;
    let (mut send, recv) = connection.open_bi().await.unwrap();
    let mut out = hello();
    out.extend(records_done_frame());
    send.write_all(&out).await.unwrap();
    a.up(client.identity.peer_id()).await;
    (connection, send, recv)
}

// ---- 2.1 -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn clipboard_offer_reaches_up_peer() {
    let (a, mut b) = connected_pair().await;
    a.handle.offer_clipboard(PeerId([9; 32]), 1);
    a.handle.offer_clipboard(b.id, 42);
    let e = b.expect(WAIT, offer_from(a.id)).await;
    assert_eq!(e, Event::ClipboardOffer { peer: a.id, id: 42 });
}

#[tokio::test(flavor = "multi_thread")]
async fn clipboard_request_reaches_up_peer() {
    let (a, mut b) = connected_pair().await;
    a.handle.request_clipboard(b.id, u64::MAX);
    let e = b
        .expect(WAIT, |e| matches!(e, Event::ClipboardRequest { .. }))
        .await;
    assert_eq!(
        e,
        Event::ClipboardRequest {
            peer: a.id,
            id: u64::MAX
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn offer_before_up_is_protocol_error() {
    let (client, a) = member_client("A");
    let connection = client
        .connect(a.addr, Expect::Peer(a.id), ALPN_MEMBER)
        .await;
    let (mut send, _recv) = connection.open_bi().await.unwrap();
    let mut out = hello();
    ClipboardMessage::Offer { id: 1 }.encode(&mut out).unwrap();
    send.write_all(&out).await.unwrap();
    assert_eq!(close_code(&connection, WAIT).await, Some(1));
}

#[tokio::test(flavor = "multi_thread")]
async fn offer_on_a_pairing_connection_is_protocol_error() {
    let a = node(machine("A"));
    a.handle.open_pairing().await.unwrap();
    let client = RawClient::new();
    let connection = client.connect(a.addr, Expect::Any, ALPN_PAIR).await;
    let (mut send, _recv) = connection.open_bi().await.unwrap();
    let mut out = hello();
    ClipboardMessage::Offer { id: 1 }.encode(&mut out).unwrap();
    send.write_all(&out).await.unwrap();
    assert_eq!(close_code(&connection, WAIT).await, Some(1));
}

#[tokio::test(flavor = "multi_thread")]
async fn chunk_on_the_control_stream_is_protocol_error() {
    let (client, mut a) = member_client("A");
    let (connection, mut send, _recv) = up_client(&client, &mut a).await;
    let mut out = Vec::new();
    clipboard::encode_chunk(b"hello", &mut out).unwrap();
    send.write_all(&out).await.unwrap();
    assert_eq!(close_code(&connection, WAIT).await, Some(1));
}

// ---- 2.2 -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn text_transfers_on_its_own_stream() {
    let (a, mut b) = connected_pair().await;
    let big: Arc<str> = Arc::from("é".repeat(MAX_TEXT_LEN as usize / 2));
    assert_eq!(big.len(), MAX_TEXT_LEN as usize);
    a.handle.send_clipboard(b.id, 7, big.clone());
    a.handle.send_clipboard(b.id, 8, Arc::from("abc"));
    // Each transfer has a stream of its own; either may arrive first.
    let mut got = Vec::new();
    for _ in 0..2 {
        let Event::ClipboardText { id, text, .. } = b.expect(WAIT, text_from(a.id)).await else {
            unreachable!()
        };
        got.push((id, text));
    }
    got.sort_by_key(|(id, _)| *id);
    assert_eq!(got[0].0, 7);
    assert!(got[0].1 == big, "the 4 MiB text arrived changed");
    assert_eq!((got[1].0, &*got[1].1), (8, "abc"));
}

#[tokio::test(flavor = "multi_thread")]
async fn second_stream_waits() {
    let (client, mut a) = member_client("A");
    let (connection, _send, _recv) = up_client(&client, &mut a).await;
    // An unfinished transfer holds the only clipboard stream allowed.
    let mut first = connection.open_uni().await.unwrap();
    let mut out = Vec::new();
    clipboard::encode_data(1, 10, &mut out).unwrap();
    first.write_all(&out).await.unwrap();
    assert!(
        timeout(Duration::from_millis(500), connection.open_uni())
            .await
            .is_err(),
        "a second clipboard stream opened while the first was unfinished"
    );
    // Finishing the first frees the slot.
    out.clear();
    clipboard::encode_chunk(b"0123456789", &mut out).unwrap();
    first.write_all(&out).await.unwrap();
    first.finish().unwrap();
    a.expect(WAIT, text_from(client.identity.peer_id())).await;
    timeout(WAIT, connection.open_uni()).await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_transfer_closes() {
    let (client, mut a) = member_client("A");
    let (connection, _send, _recv) = up_client(&client, &mut a).await;
    let mut stream = connection.open_uni().await.unwrap();
    let mut out = Vec::new();
    clipboard::encode_data(1, MAX_TEXT_LEN + 1, &mut out).unwrap();
    stream.write_all(&out).await.unwrap();
    assert_eq!(close_code(&connection, WAIT).await, Some(1));
}

#[tokio::test(flavor = "multi_thread")]
async fn short_or_invalid_transfer_closes() {
    for (header_len, body) in [(10u32, &b"short"[..]), (2, &[0xff, 0xfe][..])] {
        let (client, mut a) = member_client("A");
        let (connection, _send, _recv) = up_client(&client, &mut a).await;
        let mut stream = connection.open_uni().await.unwrap();
        let mut out = Vec::new();
        clipboard::encode_data(1, header_len, &mut out).unwrap();
        clipboard::encode_chunk(body, &mut out).unwrap();
        stream.write_all(&out).await.unwrap();
        stream.finish().unwrap();
        assert_eq!(close_code(&connection, WAIT).await, Some(1));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn uni_stream_on_a_pairing_connection_is_protocol_error() {
    let a = node(machine("A"));
    a.handle.open_pairing().await.unwrap();
    let client = RawClient::new();
    let connection = client.connect(a.addr, Expect::Any, ALPN_PAIR).await;
    let (mut send, _recv) = connection.open_bi().await.unwrap();
    send.write_all(&hello()).await.unwrap();
    let mut stream = connection.open_uni().await.unwrap();
    stream.write_all(&transfer(1, b"hi")).await.unwrap();
    stream.finish().unwrap();
    assert_eq!(close_code(&connection, WAIT).await, Some(1));
}

#[tokio::test(flavor = "multi_thread")]
async fn early_stream_is_held_until_records() {
    let (client, mut a) = member_client("A");
    let connection = client
        .connect(a.addr, Expect::Peer(a.id), ALPN_MEMBER)
        .await;
    let (mut send, _recv) = connection.open_bi().await.unwrap();
    send.write_all(&hello()).await.unwrap();
    // The transfer arrives before the records: QUIC does not order streams.
    let mut stream = connection.open_uni().await.unwrap();
    stream.write_all(&transfer(5, b"early")).await.unwrap();
    stream.finish().unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    send.write_all(&records_done_frame()).await.unwrap();
    let peer = client.identity.peer_id();
    a.up(peer).await;
    let e = a.expect(WAIT, text_from(peer)).await;
    assert_eq!(
        e,
        Event::ClipboardText {
            peer,
            id: 5,
            text: Arc::from("early")
        }
    );
}

// ---- 2.3 -----------------------------------------------------------------

/// 100 `KeyInput` frames, the last a release of usage `last`.
fn key_frames(last: u16) -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..100u16 {
        let usage = if i == 99 { last } else { 0x04 + i % 20 };
        InputMessage::Key(KeyInput {
            mark: Mark::default(),
            usage,
            down: i % 2 == 0,
        })
        .encode(&mut out)
        .unwrap();
    }
    out
}

/// Writes 100 keys on `control` and returns how long until `a` reported the
/// last one. Fails if clipboard text from `peer` is reported first.
async fn keys_arrive(
    control: &mut quinn::SendStream,
    a: &mut Node,
    peer: PeerId,
    last: u16,
) -> Duration {
    let want = PeerMessage::Input(InputAction::Key {
        key: Key(last),
        down: false,
    });
    let start = Instant::now();
    timeout(WAIT, control.write_all(&key_frames(last)))
        .await
        .expect("writing the keys stalled")
        .unwrap();
    loop {
        let e = a
            .expect(Duration::from_secs(2), |e| {
                matches!(e, Event::Message { .. } | Event::ClipboardText { .. })
            })
            .await;
        match e {
            Event::ClipboardText { .. } => panic!("text from {peer:?} arrived before the keys"),
            Event::Message { msg, .. } if msg == want => return start.elapsed(),
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn input_not_delayed_by_transfer() {
    let (client, mut a) = member_client("A");
    let (connection, mut control, _recv) = up_client(&client, &mut a).await;
    let peer = client.identity.peer_id();
    let alone = keys_arrive(&mut control, &mut a, peer, 0x50).await;

    // A transfer still under way: its header and half the text.
    let all = transfer(1, &vec![b'x'; MAX_TEXT_LEN as usize]);
    let (first, second) = all.split_at(all.len() / 2);
    let mut stream = connection.open_uni().await.unwrap();
    timeout(WAIT, stream.write_all(first))
        .await
        .expect("writing the first half stalled")
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let during = keys_arrive(&mut control, &mut a, peer, 0x51).await;
    assert!(
        during < alone + Duration::from_millis(50),
        "keys took {during:?} during a transfer, {alone:?} alone"
    );

    timeout(WAIT, stream.write_all(second))
        .await
        .expect("writing the second half stalled")
        .unwrap();
    stream.finish().unwrap();
    let Event::ClipboardText { id, text, .. } = a.expect(WAIT, text_from(peer)).await else {
        unreachable!()
    };
    assert_eq!((id, text.len()), (1, MAX_TEXT_LEN as usize));
}
