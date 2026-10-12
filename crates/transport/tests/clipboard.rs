//! Clipboard offers, requests and transfers between nodes on loopback
//! (spec: clipboard).

mod harness;

use std::sync::Arc;
use std::time::Duration;

use harness::*;
use model::{InputAction, Key, PeerId, PeerMessage};
use pairing::{Identity, Machine, MachineName};
use proto::clipboard::{
    self, ClipKind, ClipboardMessage, MAX_IMAGE_LEN, MAX_TEXT_LEN, PNG_SIGNATURE,
};
use proto::hello::Hello;
use proto::input::{InputMessage, KeyInput, Mark};
use tokio::time::{Instant, timeout};
use transport::tls::{ALPN_MEMBER, ALPN_PAIR, Expect};
use transport::{Contents, Event, records_done_frame};

const WAIT: Duration = Duration::from_secs(10);

fn offer_from(peer: PeerId) -> impl Fn(&Event) -> bool {
    move |e| matches!(e, Event::ClipboardOffer { peer: p, .. } if *p == peer)
}

fn text_from(peer: PeerId) -> impl Fn(&Event) -> bool {
    move |e| matches!(e, Event::ClipboardData { peer: p, .. } if *p == peer)
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
    a.handle.offer_clipboard(PeerId([9; 32]), 1, ClipKind::Text);
    a.handle.offer_clipboard(b.id, 42, ClipKind::Text);
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
    a.handle
        .send_clipboard(b.id, 7, Contents::Text(big.clone()));
    a.handle
        .send_clipboard(b.id, 8, Contents::Text(Arc::from("abc")));
    // Each transfer has a stream of its own; either may arrive first.
    let mut got = Vec::new();
    for _ in 0..2 {
        let Event::ClipboardData {
            id,
            contents: Contents::Text(text),
            ..
        } = b.expect(WAIT, text_from(a.id)).await
        else {
            panic!("not text")
        };
        got.push((id, text));
    }
    got.sort_by_key(|(id, _)| *id);
    assert_eq!(got[0].0, 7);
    assert!(got[0].1 == big, "the 4 MiB text arrived changed");
    assert_eq!((got[1].0, &*got[1].1), (8, "abc"));
}

#[tokio::test(flavor = "multi_thread")]
async fn second_copy_stream_waits_unread() {
    let (client, mut a) = member_client("A");
    let peer = client.identity.peer_id();
    let (connection, _send, _recv) = up_client(&client, &mut a).await;
    // An unfinished copy holds the copy lane.
    let mut first = connection.open_uni().await.unwrap();
    let mut out = Vec::new();
    clipboard::encode_data(1, 10, &mut out).unwrap();
    first.write_all(&out).await.unwrap();
    // Let the first header arrive and take the lane.
    tokio::time::sleep(Duration::from_millis(200)).await;
    // A second copy opens (two streams are allowed) but waits unread.
    let mut second = connection.open_uni().await.unwrap();
    second.write_all(&transfer(2, b"second")).await.unwrap();
    second.finish().unwrap();
    a.none_for(Duration::from_millis(500), text_from(peer))
        .await;
    // A third waits for the peer's stream credit.
    assert!(
        timeout(Duration::from_millis(500), connection.open_uni())
            .await
            .is_err(),
        "a third clipboard stream opened while two were unfinished"
    );
    // Finishing the first lets the second be read, in turn.
    out.clear();
    clipboard::encode_chunk(b"0123456789", &mut out).unwrap();
    first.write_all(&out).await.unwrap();
    first.finish().unwrap();
    let ids = [
        a.expect(WAIT, text_from(peer)).await,
        a.expect(WAIT, text_from(peer)).await,
    ]
    .map(|e| match e {
        Event::ClipboardData { id, .. } => id,
        _ => unreachable!(),
    });
    assert_eq!(ids, [1, 2]);
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
        Event::ClipboardData {
            peer,
            id: 5,
            contents: Contents::Text(Arc::from("early"))
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

/// How long keys may take while a transfer is held half-sent. The tests prove
/// the spec ("arrive without waiting for the transfer to finish") by sending
/// the keys while the transfer cannot finish; this bound only catches a hang.
/// A tighter bound measured the test machine's CPU, not the protocol: on a
/// busy 2-core runner, keys took 45–340 ms with no packet lost, and as long
/// with no transfer at all.
const KEYS_DURING_TRANSFER: Duration = Duration::from_secs(1);

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
                matches!(e, Event::Message { .. } | Event::ClipboardData { .. })
            })
            .await;
        match e {
            Event::ClipboardData { .. } => panic!("a copy from {peer:?} arrived before the keys"),
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
        during < KEYS_DURING_TRANSFER,
        "keys took {during:?} during a transfer, {alone:?} alone"
    );

    timeout(WAIT, stream.write_all(second))
        .await
        .expect("writing the second half stalled")
        .unwrap();
    stream.finish().unwrap();
    let Event::ClipboardData { id, contents, .. } = a.expect(WAIT, text_from(peer)).await else {
        unreachable!()
    };
    assert_eq!((id, contents.bytes().len()), (1, MAX_TEXT_LEN as usize));
}

// ---- add-clipboard-images 2.1 --------------------------------------------

/// `len` bytes that start with the PNG signature.
fn png_like(len: usize) -> Vec<u8> {
    let mut bytes = vec![0x42; len];
    bytes[..8].copy_from_slice(&PNG_SIGNATURE);
    bytes
}

fn image_transfer(id: u64, png: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    clipboard::encode_image_data(id, png.len() as u32, &mut out).unwrap();
    for chunk in png.chunks(proto::frame::MAX_FRAME_PAYLOAD) {
        clipboard::encode_chunk(chunk, &mut out).unwrap();
    }
    out
}

/// A node that advertises only protocol version 1.
fn old_node(machine: Machine) -> Node {
    let mut c = config(machine);
    c.hello = Hello { min: 1, max: 1 };
    start(c)
}

/// `a` on this release, `old` on version 1 only, members and up.
async fn mixed_pair() -> (Node, Node) {
    let (mut a, mut old) = (machine("A"), machine("Old"));
    befriend(&mut a, &mut old);
    let (mut a, mut old) = (node(a), old_node(old));
    a.sees(&old, false);
    let (ida, idold) = (a.id, old.id);
    a.up(idold).await;
    old.up(ida).await;
    (a, old)
}

#[tokio::test(flavor = "multi_thread")]
async fn image_transfers_at_version_2() {
    let (a, mut b) = connected_pair().await;
    let png: Arc<[u8]> = Arc::from(png_like(1024 * 1024));
    a.handle.offer_clipboard(b.id, 3, ClipKind::Image);
    assert_eq!(
        b.expect(WAIT, offer_from(a.id)).await,
        Event::ClipboardOffer { peer: a.id, id: 3 }
    );
    a.handle
        .send_clipboard(b.id, 3, Contents::Image(png.clone()));
    let e = b.expect(WAIT, text_from(a.id)).await;
    assert_eq!(
        e,
        Event::ClipboardData {
            peer: a.id,
            id: 3,
            contents: Contents::Image(png)
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_version_1_peer_pairs_and_shares_text_but_never_images() {
    // Pairing by code between this release and a node like 0.1.0.
    let (mut a, mut old) = (node(machine("A")), old_node(machine("Old")));
    pair(&mut old, &mut a).await;

    // An image offer and an image transfer are dropped here, not sent.
    a.handle.offer_clipboard(old.id, 1, ClipKind::Image);
    a.handle
        .send_clipboard(old.id, 1, Contents::Image(Arc::from(png_like(64))));
    old.none_for(Duration::from_secs(1), |e| {
        matches!(
            e,
            Event::ClipboardOffer { .. } | Event::ClipboardData { .. } | Event::PeerDown(..)
        )
    })
    .await;

    // The connection is still up, and text still works both ways.
    a.handle.offer_clipboard(old.id, 2, ClipKind::Text);
    assert_eq!(
        old.expect(WAIT, offer_from(a.id)).await,
        Event::ClipboardOffer { peer: a.id, id: 2 }
    );
    a.handle
        .send_clipboard(old.id, 2, Contents::Text(Arc::from("hi")));
    assert_eq!(
        old.expect(WAIT, text_from(a.id)).await,
        Event::ClipboardData {
            peer: a.id,
            id: 2,
            contents: Contents::Text(Arc::from("hi"))
        }
    );
    old.handle
        .send_clipboard(a.id, 9, Contents::Text(Arc::from("back")));
    assert_eq!(
        a.expect(WAIT, text_from(old.id)).await,
        Event::ClipboardData {
            peer: old.id,
            id: 9,
            contents: Contents::Text(Arc::from("back"))
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn members_on_a_mixed_desk_stay_up() {
    let (a, mut old) = mixed_pair().await;
    a.handle.offer_clipboard(old.id, 1, ClipKind::Image);
    old.none_for(Duration::from_millis(500), |e| {
        matches!(e, Event::ClipboardOffer { .. } | Event::PeerDown(..))
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn image_header_on_a_version_1_connection_closes() {
    let (client, a) = member_client("A");
    let connection = client
        .connect(a.addr, Expect::Peer(a.id), ALPN_MEMBER)
        .await;
    let (mut send, _recv) = connection.open_bi().await.unwrap();
    let mut out = Vec::new();
    Hello { min: 1, max: 1 }.encode_frame(&mut out).unwrap();
    out.extend(records_done_frame());
    send.write_all(&out).await.unwrap();
    let mut stream = connection.open_uni().await.unwrap();
    stream
        .write_all(&image_transfer(1, &png_like(64)))
        .await
        .unwrap();
    stream.finish().unwrap();
    assert_eq!(close_code(&connection, WAIT).await, Some(1));
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_or_non_png_image_closes() {
    for oversized in [true, false] {
        let (client, mut a) = member_client("A");
        let (connection, _send, _recv) = up_client(&client, &mut a).await;
        let mut stream = connection.open_uni().await.unwrap();
        let out = if oversized {
            let mut out = Vec::new();
            clipboard::encode_image_data(1, MAX_IMAGE_LEN + 1, &mut out).unwrap();
            out
        } else {
            let mut png = png_like(64);
            png[1] = b'J';
            image_transfer(1, &png)
        };
        stream.write_all(&out).await.unwrap();
        let _ = stream.finish();
        assert_eq!(
            close_code(&connection, WAIT).await,
            Some(1),
            "oversized: {oversized}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn slow_transfer_reports_progress_at_most_once_a_second() {
    let (client, mut a) = member_client("A");
    let (connection, _send, _recv) = up_client(&client, &mut a).await;
    let peer = client.identity.peer_id();
    let png = png_like(20 * 1024 * 1024);
    let all = image_transfer(4, &png);
    let mut stream = connection.open_uni().await.unwrap();
    // About 2.6 seconds of trickle, then the rest at once.
    let writer = tokio::spawn(async move {
        let pieces: Vec<&[u8]> = all.chunks(all.len() / 8).collect();
        let (last, rest) = pieces.split_last().unwrap();
        for piece in rest {
            stream.write_all(piece).await.unwrap();
            tokio::time::sleep(Duration::from_millis(370)).await;
        }
        stream.write_all(last).await.unwrap();
        stream.finish().unwrap();
    });
    let mut progress = Vec::new();
    loop {
        let e = a
            .expect(WAIT, |e| {
                matches!(
                    e,
                    Event::ClipboardProgress { .. } | Event::ClipboardData { .. }
                )
            })
            .await;
        match e {
            Event::ClipboardProgress { peer: p, id } => {
                assert_eq!((p, id), (peer, 4));
                progress.push(Instant::now());
            }
            Event::ClipboardData { id, contents, .. } => {
                assert_eq!(id, 4);
                assert!(contents == Contents::Image(Arc::from(png)), "image changed");
                break;
            }
            _ => unreachable!(),
        }
    }
    writer.await.unwrap();
    assert!(!progress.is_empty(), "no progress during a 2.6 s transfer");
    for pair in progress.windows(2) {
        let gap = pair[1] - pair[0];
        // A little scheduling slack below the one-second floor.
        assert!(gap >= Duration::from_millis(900), "progress {gap:?} apart");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn input_not_delayed_by_image_transfer() {
    let (client, mut a) = member_client("A");
    let (connection, mut control, _recv) = up_client(&client, &mut a).await;
    let peer = client.identity.peer_id();
    let alone = keys_arrive(&mut control, &mut a, peer, 0x52).await;

    let all = image_transfer(1, &png_like(20 * 1024 * 1024));
    let (first, second) = all.split_at(all.len() / 2);
    let mut stream = connection.open_uni().await.unwrap();
    timeout(WAIT, stream.write_all(first))
        .await
        .expect("writing the first half stalled")
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let during = keys_arrive(&mut control, &mut a, peer, 0x53).await;
    assert!(
        during < KEYS_DURING_TRANSFER,
        "keys took {during:?} during an image transfer, {alone:?} alone"
    );

    timeout(WAIT, stream.write_all(second))
        .await
        .expect("writing the second half stalled")
        .unwrap();
    stream.finish().unwrap();
    let Event::ClipboardData { id, contents, .. } = a.expect(WAIT, text_from(peer)).await else {
        unreachable!()
    };
    assert_eq!(id, 1);
    assert_eq!(contents.kind(), ClipKind::Image);
}
