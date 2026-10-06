//! Clipboard runs on the agent link, both ends over duplex streams (specs:
//! agent-link, clipboard).

use ipc::clipboard::{Copied, MAX_TEXT_LEN, NotShared};
use ipc::frame::{self, Decoded};
use ipc::link::{AgentEnd, FromAgent, FromService, ServiceEnd};
use ipc::message::{Hello, Role, ToService};
use model::{InputEvent, Key, Millis, Origin};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::sync::mpsc::UnboundedReceiver;

/// An agent driven by hand: it has said `Hello` and answered a `Ping`, so
/// the service passes its reports on at once.
struct Raw {
    s: DuplexStream,
}

impl Raw {
    async fn report(&mut self, m: ToService) {
        let mut out = Vec::new();
        m.encode_frame(&mut out).unwrap();
        self.s.write_all(&out).await.unwrap();
    }

    async fn raw_frame(&mut self, ty: u16, payload: &[u8]) {
        let mut out = Vec::new();
        frame::encode(ty, payload, &mut out).unwrap();
        self.s.write_all(&out).await.unwrap();
    }
}

async fn raw_agent() -> (ipc::link::AgentHandle, UnboundedReceiver<FromAgent>, Raw) {
    let (a, b) = tokio::io::duplex(1 << 20);
    let mut raw = Raw { s: b };
    let mut out = Vec::new();
    Hello::this_release(Role::SessionAgent)
        .encode_frame(&mut out)
        .unwrap();
    raw.s.write_all(&out).await.unwrap();
    let (handle, events) = ServiceEnd::accept(a, || Millis(0)).await.unwrap();
    // Read the service's Hello so its writer never blocks.
    let mut buf = Vec::new();
    while !matches!(frame::decode(&buf), Ok(Decoded::Frame { .. })) {
        let mut chunk = [0u8; 256];
        let n = raw.s.read(&mut chunk).await.unwrap();
        buf.extend_from_slice(&chunk[..n]);
    }
    raw.report(ToService::Pong {
        service: Millis(0),
        agent: Millis(0),
    })
    .await;
    (handle, events, raw)
}

fn key(usage: u16) -> ToService {
    ToService::Input {
        at: Millis(1),
        input: InputEvent::Key {
            key: Key(usage),
            down: true,
        },
        origin: Origin::Physical,
    }
}

#[tokio::test]
async fn copy_run_reaches_service() {
    let (a, b) = tokio::io::duplex(1 << 16);
    let (service, agent) = tokio::join!(
        ServiceEnd::accept(a, || Millis(0)),
        AgentEnd::connect(b, || Millis(0)),
    );
    let ((_handle, mut events), (reports, _commands)) = (service.unwrap(), agent.unwrap());
    let big = "ü".repeat(100_000);
    reports.send_clipboard_text("hello").unwrap();
    reports.send_clipboard_text(&big).unwrap();
    reports.send_clipboard_text("").unwrap();
    reports
        .send(ToService::ClipboardNotShared(NotShared::Private))
        .unwrap();
    let over = "x".repeat(MAX_TEXT_LEN as usize + 1);
    reports.send_clipboard_text(&over).unwrap();

    let want = [
        Copied::Text("hello".into()),
        Copied::Text(big),
        Copied::Text(String::new()),
        Copied::NotShared(NotShared::Private),
        Copied::NotShared(NotShared::TooLarge),
    ];
    for w in want {
        assert_eq!(events.recv().await, Some(FromAgent::Clipboard(w)));
    }
}

#[tokio::test]
async fn stray_chunk_closes_link() {
    let (_handle, mut events, mut raw) = raw_agent().await;
    raw.report(ToService::ClipboardChunk(b"hi".to_vec())).await;
    assert_eq!(events.recv().await, None);
}

#[tokio::test]
async fn short_run_then_new_run_closes_link() {
    let (_handle, mut events, mut raw) = raw_agent().await;
    raw.report(ToService::ClipboardText(5)).await;
    raw.report(ToService::ClipboardChunk(b"hi".to_vec())).await;
    raw.report(ToService::ClipboardText(2)).await;
    assert_eq!(events.recv().await, None);
}

#[tokio::test]
async fn not_shared_mid_run_closes_link() {
    let (_handle, mut events, mut raw) = raw_agent().await;
    raw.report(ToService::ClipboardText(5)).await;
    raw.report(ToService::ClipboardNotShared(NotShared::NoText))
        .await;
    assert_eq!(events.recv().await, None);
}

#[tokio::test]
async fn too_many_bytes_closes_link() {
    let (_handle, mut events, mut raw) = raw_agent().await;
    raw.report(ToService::ClipboardText(2)).await;
    raw.report(ToService::ClipboardChunk(b"abc".to_vec())).await;
    assert_eq!(events.recv().await, None);
}

#[tokio::test]
async fn over_limit_and_empty_chunks_close_link() {
    let (_h1, mut e1, mut r1) = raw_agent().await;
    r1.raw_frame(
        ipc::message::CLIPBOARD_TEXT,
        &(MAX_TEXT_LEN + 1).to_le_bytes(),
    )
    .await;
    assert_eq!(e1.recv().await, None);

    let (_h2, mut e2, mut r2) = raw_agent().await;
    r2.report(ToService::ClipboardText(1)).await;
    r2.raw_frame(ipc::message::CLIPBOARD_CHUNK, &[]).await;
    assert_eq!(e2.recv().await, None);
}

#[tokio::test]
async fn input_between_chunks_is_accepted() {
    let (_handle, mut events, mut raw) = raw_agent().await;
    raw.report(ToService::ClipboardText(4)).await;
    raw.report(ToService::ClipboardChunk(b"ab".to_vec())).await;
    raw.report(key(0x04)).await;
    raw.report(ToService::ClipboardChunk(b"cd".to_vec())).await;
    assert!(matches!(events.recv().await, Some(FromAgent::Input { .. })));
    assert_eq!(
        events.recv().await,
        Some(FromAgent::Clipboard(Copied::Text("abcd".into())))
    );
}

#[tokio::test]
async fn invalid_utf8_closes_link() {
    let (_handle, mut events, mut raw) = raw_agent().await;
    raw.report(ToService::ClipboardText(2)).await;
    raw.report(ToService::ClipboardChunk(vec![0xff, 0xfe]))
        .await;
    assert_eq!(events.recv().await, None);
}

#[tokio::test]
async fn set_clipboard_reaches_agent() {
    let (a, b) = tokio::io::duplex(1 << 16);
    let (service, agent) = tokio::join!(
        ServiceEnd::accept(a, || Millis(0)),
        AgentEnd::connect(b, || Millis(0)),
    );
    let ((handle, _events), (_reports, mut commands)) = (service.unwrap(), agent.unwrap());
    let big = "w\r\n".repeat(60_000);
    handle.set_clipboard("world").unwrap();
    // Over the limit: not sent at all.
    handle
        .set_clipboard(&"x".repeat(MAX_TEXT_LEN as usize + 1))
        .unwrap();
    handle.set_clipboard(&big).unwrap();
    assert_eq!(
        commands.recv().await,
        Some(FromService::SetClipboard("world".into()))
    );
    assert_eq!(commands.recv().await, Some(FromService::SetClipboard(big)));
}
