//! File copies between nodes on loopback (spec: file-transfer, clipboard):
//! the manifest as a copy, each file's contents on request, and the two
//! lanes that keep a long file from holding up a copy.

mod harness;

use std::sync::Arc;
use std::time::Duration;

use harness::*;
use model::{InputAction, Key, PeerId, PeerMessage};
use pairing::{Identity, Machine, MachineName};
use proto::clipboard::{self, ClipKind};
use proto::files::{self, FileContentRequest, FileEntry};
use proto::hello::Hello;
use proto::input::{InputMessage, KeyInput, Mark};
use tokio::time::{Instant, timeout};
use transport::tls::{ALPN_MEMBER, Expect};
use transport::{Contents, Event, FileChunk, FileFetch, records_done_frame};

const WAIT: Duration = Duration::from_secs(10);
const MIB: usize = 1 << 20;

fn manifest(entries: &[FileEntry<'_>]) -> Arc<[u8]> {
    let mut out = Vec::new();
    files::encode_manifest(entries.iter().copied(), &mut out).unwrap();
    Arc::from(out)
}

fn tree() -> Arc<[u8]> {
    manifest(&[
        FileEntry {
            path: "Photos",
            is_dir: true,
            size: 0,
        },
        FileEntry {
            path: "Photos/a.jpg",
            is_dir: false,
            size: 3 * MIB as u64,
        },
        FileEntry {
            path: "Photos/empty",
            is_dir: true,
            size: 0,
        },
    ])
}

fn data_from(peer: PeerId) -> impl Fn(&Event) -> bool {
    move |e| matches!(e, Event::ClipboardData { peer: p, .. } if *p == peer)
}

fn request_from(peer: PeerId) -> impl Fn(&Event) -> bool {
    move |e| matches!(e, Event::FileRequest { peer: p, .. } if *p == peer)
}

/// A file's deterministic bytes.
fn bytes(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31) ^ seed)
        .collect()
}

/// Reads a fetch to its end: the announced length, the bytes, and whether
/// it ended with `End`.
async fn drain(mut fetch: FileFetch) -> (Option<u64>, Vec<u8>, bool) {
    let (mut len, mut body, mut ended) = (None, Vec::new(), false);
    while let Some(piece) = timeout(WAIT, fetch.recv()).await.expect("fetch stalled") {
        match piece {
            FileChunk::Len(n) => len = Some(n),
            FileChunk::Bytes(b) => body.extend_from_slice(&b),
            FileChunk::End => ended = true,
        }
    }
    (len, body, ended)
}

/// Reads a fetch until it ends or goes quiet for a second: whether it
/// ended with `End`, and the bytes it got.
async fn until_quiet(mut fetch: FileFetch) -> (bool, usize) {
    let (mut ended, mut got) = (false, 0);
    while let Ok(Some(piece)) = timeout(Duration::from_secs(1), fetch.recv()).await {
        match piece {
            FileChunk::Bytes(b) => got += b.len(),
            FileChunk::End => ended = true,
            FileChunk::Len(_) => {}
        }
    }
    (ended, got)
}

/// `b` asks `a` for file `index` of copy `id`; `a` is told, and answers with
/// `body` in 100 KB pieces. Returns `b`'s fetch.
async fn fetch_from(a: &mut Node, b: &mut Node, id: u64, index: u32, body: Vec<u8>) -> FileFetch {
    let fetch = b.handle.request_file(a.id, id, index);
    assert_eq!(
        a.expect(WAIT, request_from(b.id)).await,
        Event::FileRequest {
            peer: b.id,
            id,
            index
        }
    );
    let sink = a.handle.send_file(b.id, id, index, body.len() as u64);
    tokio::spawn(async move {
        for piece in body.chunks(100_000) {
            if sink.send(piece.to_vec()).await.is_err() {
                return;
            }
        }
    });
    fetch
}

#[tokio::test(flavor = "multi_thread")]
async fn manifest_travels_as_a_copy_at_version_3() {
    let (a, mut b) = connected_pair().await;
    a.handle.offer_clipboard(b.id, 5, ClipKind::Files);
    assert_eq!(
        b.expect(WAIT, |e| matches!(e, Event::ClipboardOffer { .. }))
            .await,
        Event::ClipboardOffer { peer: a.id, id: 5 }
    );
    a.handle.send_clipboard(b.id, 5, Contents::Files(tree()));
    assert_eq!(
        b.expect(WAIT, data_from(a.id)).await,
        Event::ClipboardData {
            peer: a.id,
            id: 5,
            contents: Contents::Files(tree())
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn requested_file_streams_end_to_end() {
    let (mut a, mut b) = connected_pair().await;
    let body = bytes(3 * MIB + 7, 9);
    let fetch = fetch_from(&mut a, &mut b, 5, 1, body.clone()).await;
    let (len, got, ended) = drain(fetch).await;
    assert_eq!(len, Some(body.len() as u64));
    assert!(got == body, "the file arrived changed");
    assert!(ended, "the fetch did not end with End");
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_file_streams() {
    let (mut a, mut b) = connected_pair().await;
    let fetch = fetch_from(&mut a, &mut b, 5, 0, Vec::new()).await;
    assert_eq!(drain(fetch).await, (Some(0), Vec::new(), true));
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_abandoning_a_file_fails_the_fetch() {
    let (mut a, mut b) = connected_pair().await;
    let fetch = b.handle.request_file(a.id, 5, 0);
    a.expect(WAIT, request_from(b.id)).await;
    let sink = a.handle.send_file(b.id, 5, 0, 1000);
    sink.send(vec![1; 400]).await.unwrap();
    drop(sink);
    // The reset may overtake the header, leaving the fetch to its caller's
    // timeout; either way it never ends whole.
    let (ended, got) = until_quiet(fetch).await;
    assert!(got <= 400);
    assert!(!ended, "a short file must not end the fetch with End");
    // The connection stays up and works.
    a.handle.offer_clipboard(b.id, 6, ClipKind::Text);
    b.expect(WAIT, |e| matches!(e, Event::ClipboardOffer { id: 6, .. }))
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_writing_too_much_fails_the_fetch() {
    let (mut a, b) = connected_pair().await;
    let fetch = b.handle.request_file(a.id, 5, 0);
    a.expect(WAIT, request_from(b.id)).await;
    let sink = a.handle.send_file(b.id, 5, 0, 10);
    sink.send(vec![1; 11]).await.unwrap();
    drop(sink);
    assert!(!until_quiet(fetch).await.0);
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_going_down_mid_file_fails_the_fetch() {
    let (mut a, b) = connected_pair().await;
    let fetch = b.handle.request_file(a.id, 5, 0);
    a.expect(WAIT, request_from(b.id)).await;
    let sink = a.handle.send_file(b.id, 5, 0, 10 * MIB as u64);
    sink.send(vec![1; MIB]).await.unwrap();
    a.handle.shutdown();
    let (_, got, ended) = drain(fetch).await;
    assert!(!ended && got.len() < 10 * MIB);
}

#[tokio::test(flavor = "multi_thread")]
async fn unrequested_file_is_discarded() {
    let (a, mut b) = connected_pair().await;
    let sink = a.handle.send_file(b.id, 5, 0, 3);
    sink.send(b"abc".to_vec()).await.unwrap();
    drop(sink);
    b.none_for(Duration::from_millis(500), |e| {
        matches!(e, Event::ClipboardData { .. } | Event::PeerDown(..))
    })
    .await;
    // Nothing was waiting for it, and the lane is free again.
    let mut a = a;
    let fetch = fetch_from(&mut a, &mut b, 5, 0, b"xyz".to_vec()).await;
    assert_eq!(drain(fetch).await, (Some(3), b"xyz".to_vec(), true));
}

/// A file with a 5 GiB length streams with backpressure: the owner can
/// write only a bounded amount ahead of a reader that does not read, so no
/// side ever holds the file whole.
#[tokio::test(flavor = "multi_thread")]
async fn huge_length_streams_with_backpressure() {
    let (mut a, b) = connected_pair().await;
    let five_gib = 5u64 << 30;
    let mut fetch = b.handle.request_file(a.id, 5, 0);
    a.expect(WAIT, request_from(b.id)).await;
    let sink = a.handle.send_file(b.id, 5, 0, five_gib);
    // Nobody reads the fetch: the owner must stall well short of the file.
    let mut written = 0usize;
    let piece = vec![7u8; MIB];
    while timeout(Duration::from_millis(500), sink.send(piece.clone()))
        .await
        .is_ok()
    {
        written += MIB;
        assert!(written < 256 * MIB, "no backpressure after {written} bytes");
    }
    // Reading lets it continue; the length is carried as a u64.
    assert_eq!(fetch.recv().await, Some(FileChunk::Len(five_gib)));
    let mut read = 0usize;
    while read < written {
        match timeout(WAIT, fetch.recv()).await.unwrap() {
            Some(FileChunk::Bytes(b)) => read += b.len(),
            other => panic!("{other:?}"),
        }
    }
    assert!(
        timeout(WAIT, sink.send(piece)).await.is_ok(),
        "the owner did not resume"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn text_crosses_while_a_file_streams() {
    let (mut a, mut b) = connected_pair().await;
    let mut fetch = b.handle.request_file(a.id, 5, 0);
    a.expect(WAIT, request_from(b.id)).await;
    let sink = a.handle.send_file(b.id, 5, 0, 100 * MIB as u64);
    sink.send(vec![1; MIB]).await.unwrap();
    assert_eq!(fetch.recv().await, Some(FileChunk::Len(100 * MIB as u64)));
    // The file is still open; a text copy uses the other lane.
    a.handle
        .send_clipboard(b.id, 6, Contents::Text(Arc::from("hello")));
    let e = b.expect(WAIT, data_from(a.id)).await;
    assert_eq!(
        e,
        Event::ClipboardData {
            peer: a.id,
            id: 6,
            contents: Contents::Text(Arc::from("hello"))
        }
    );
    drop(sink);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_version_2_peer_never_sees_files() {
    let (mut a, mut old) = (machine("A"), machine("Old"));
    befriend(&mut a, &mut old);
    let mut c = config(old);
    c.hello = Hello { min: 1, max: 2 };
    let (mut a, mut old) = (node(a), start(c));
    a.sees(&old, false);
    let (ida, idold) = (a.id, old.id);
    a.up(idold).await;
    old.up(ida).await;

    a.handle.offer_clipboard(old.id, 1, ClipKind::Files);
    a.handle.send_clipboard(old.id, 1, Contents::Files(tree()));
    let sink = a.handle.send_file(old.id, 1, 1, 3);
    let _ = sink.send(b"abc".to_vec()).await;
    old.none_for(Duration::from_secs(1), |e| {
        matches!(
            e,
            Event::ClipboardOffer { .. } | Event::ClipboardData { .. } | Event::PeerDown(..)
        )
    })
    .await;
    // Asking an old peer for a file ends the fetch at once, unanswered.
    let fetch = a.handle.request_file(old.id, 1, 1);
    assert_eq!(drain(fetch).await, (None, Vec::new(), false));
    old.none_for(Duration::from_millis(300), |e| {
        matches!(e, Event::FileRequest { .. })
    })
    .await;
    // Images and text still work.
    a.handle.offer_clipboard(old.id, 2, ClipKind::Image);
    assert_eq!(
        old.expect(WAIT, |e| matches!(e, Event::ClipboardOffer { .. }))
            .await,
        Event::ClipboardOffer { peer: a.id, id: 2 }
    );
}

// ---- raw peers -----------------------------------------------------------

fn member_client(name: &str) -> (RawClient, Node) {
    let client = RawClient::new();
    let identity = Identity::from_file(&*client.identity.to_file()).unwrap();
    let mut theirs = Machine::new(identity, MachineName::new("Raw").unwrap());
    let mut mine = machine(name);
    befriend(&mut mine, &mut theirs);
    (client, node(mine))
}

async fn up_client(
    client: &RawClient,
    a: &mut Node,
    hello: Hello,
) -> (quinn::Connection, quinn::SendStream, quinn::RecvStream) {
    let connection = client
        .connect(a.addr, Expect::Peer(a.id), ALPN_MEMBER)
        .await;
    let (mut send, recv) = connection.open_bi().await.unwrap();
    let mut out = Vec::new();
    hello.encode_frame(&mut out).unwrap();
    out.extend(records_done_frame());
    send.write_all(&out).await.unwrap();
    a.up(client.identity.peer_id()).await;
    (connection, send, recv)
}

fn manifest_stream(id: u64, manifest: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    clipboard::encode_files_data(id, manifest.len() as u32, &mut out).unwrap();
    for chunk in manifest.chunks(1000) {
        clipboard::encode_chunk(chunk, &mut out).unwrap();
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn hostile_manifest_closes_the_connection() {
    for path in ["../../Windows/System32/evil.dll", "C:/x.dll", "/abs"] {
        let (client, mut a) = member_client("A");
        let (connection, _send, _recv) = up_client(&client, &mut a, Hello::this_release()).await;
        // Written without the encoder's checks, as a hostile sender would.
        let mut bad = 1u32.to_le_bytes().to_vec();
        bad.push(0);
        bad.extend_from_slice(&1u64.to_le_bytes());
        bad.extend_from_slice(&(path.len() as u16).to_le_bytes());
        bad.extend_from_slice(path.as_bytes());
        let mut stream = connection.open_uni().await.unwrap();
        stream.write_all(&manifest_stream(1, &bad)).await.unwrap();
        stream.finish().unwrap();
        assert_eq!(close_code(&connection, WAIT).await, Some(1), "{path}");
        a.none_for(Duration::from_millis(200), |e| {
            matches!(e, Event::ClipboardData { .. })
        })
        .await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn file_messages_on_a_version_2_connection_close_it() {
    for content in [false, true] {
        let (client, mut a) = member_client("A");
        let (connection, mut send, _recv) =
            up_client(&client, &mut a, Hello { min: 1, max: 2 }).await;
        if content {
            let mut out = Vec::new();
            files::encode_content_data(1, 0, 3, &mut out).unwrap();
            clipboard::encode_chunk(b"abc", &mut out).unwrap();
            let mut stream = connection.open_uni().await.unwrap();
            stream.write_all(&out).await.unwrap();
            stream.finish().unwrap();
        } else {
            let mut out = Vec::new();
            FileContentRequest { id: 1, index: 0 }
                .encode(&mut out)
                .unwrap();
            send.write_all(&out).await.unwrap();
        }
        assert_eq!(
            close_code(&connection, WAIT).await,
            Some(1),
            "content: {content}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn file_request_reaches_the_owner() {
    let (client, mut a) = member_client("A");
    let (_connection, mut send, _recv) = up_client(&client, &mut a, Hello::this_release()).await;
    let mut out = Vec::new();
    FileContentRequest {
        id: 9,
        index: 65_535,
    }
    .encode(&mut out)
    .unwrap();
    send.write_all(&out).await.unwrap();
    assert_eq!(
        a.expect(WAIT, |e| matches!(e, Event::FileRequest { .. }))
            .await,
        Event::FileRequest {
            peer: client.identity.peer_id(),
            id: 9,
            index: 65_535
        }
    );
}

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
/// last one.
async fn keys_arrive(control: &mut quinn::SendStream, a: &mut Node, last: u16) -> Duration {
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
        if let Event::Message { msg, .. } = a
            .expect(Duration::from_secs(2), |e| {
                matches!(e, Event::Message { .. })
            })
            .await
            && msg == want
        {
            return start.elapsed();
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn input_not_delayed_by_a_file_transfer() {
    let (client, mut a) = member_client("A");
    let (connection, mut control, _recv) = up_client(&client, &mut a, Hello::this_release()).await;
    let peer = client.identity.peer_id();
    let alone = keys_arrive(&mut control, &mut a, 0x52).await;

    // `a` asks the raw peer for a 40 MiB file and reads it as it comes.
    let fetch = a.handle.request_file(peer, 3, 0);
    let reader = tokio::spawn(drain(fetch));
    let body = vec![0x5a; 40 * MIB];
    let mut all = Vec::new();
    files::encode_content_data(3, 0, body.len() as u64, &mut all).unwrap();
    for chunk in body.chunks(proto::frame::MAX_FRAME_PAYLOAD) {
        clipboard::encode_chunk(chunk, &mut all).unwrap();
    }
    let (first, second) = all.split_at(all.len() / 2);
    let mut stream = connection.open_uni().await.unwrap();
    timeout(WAIT, stream.write_all(first))
        .await
        .expect("writing the first half stalled")
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let during = keys_arrive(&mut control, &mut a, 0x53).await;
    assert!(
        during < alone + Duration::from_millis(50),
        "keys took {during:?} during a file transfer, {alone:?} alone"
    );

    timeout(WAIT, stream.write_all(second))
        .await
        .expect("writing the second half stalled")
        .unwrap();
    stream.finish().unwrap();
    let (len, got, ended) = reader.await.unwrap();
    assert_eq!(len, Some(body.len() as u64));
    assert!(ended && got == body);
}

#[tokio::test(flavor = "multi_thread")]
async fn reset_mid_file_fails_the_fetch_and_keeps_the_connection() {
    let (client, mut a) = member_client("A");
    let (connection, _control, _recv) = up_client(&client, &mut a, Hello::this_release()).await;
    let peer = client.identity.peer_id();
    let fetch = a.handle.request_file(peer, 3, 0);
    let mut out = Vec::new();
    files::encode_content_data(3, 0, 1000, &mut out).unwrap();
    clipboard::encode_chunk(&[1; 100], &mut out).unwrap();
    let mut stream = connection.open_uni().await.unwrap();
    stream.write_all(&out).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    stream.reset(quinn::VarInt::from_u32(0)).unwrap();
    let (len, _, ended) = drain(fetch).await;
    assert_eq!(len, Some(1000));
    assert!(!ended);
    assert_eq!(
        timeout(Duration::from_millis(300), connection.closed())
            .await
            .ok(),
        None,
        "the connection closed"
    );
}
