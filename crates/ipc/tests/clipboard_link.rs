//! Clipboard runs on the agent link, both ends over duplex streams (specs:
//! agent-link, clipboard).

use ipc::clipboard::{Copied, MAX_IMAGE_LEN, MAX_TEXT_LEN, NotShared, PNG_SIGNATURE};
use ipc::frame::{self, Decoded};
use ipc::link::{AgentEnd, AgentRole, FromAgent, FromService, ServiceEnd, SetClipboardError};
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
    let (handle, events) = ServiceEnd::accept(a, AgentRole::Session, || Millis(0))
        .await
        .unwrap();
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
        ServiceEnd::accept(a, AgentRole::Session, || Millis(0)),
        AgentEnd::connect(b, AgentRole::Session, || Millis(0)),
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
        ServiceEnd::accept(a, AgentRole::Session, || Millis(0)),
        AgentEnd::connect(b, AgentRole::Session, || Millis(0)),
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

// ---- add-clipboard-images 3.1 --------------------------------------------

fn png_like(len: usize) -> Vec<u8> {
    let mut bytes = vec![0x42; len];
    bytes[..8].copy_from_slice(&PNG_SIGNATURE);
    bytes
}

/// A hand-driven agent whose `Hello` advertises `min..=max`.
async fn raw_agent_at(
    min: u16,
    max: u16,
) -> (ipc::link::AgentHandle, UnboundedReceiver<FromAgent>, Raw) {
    let (a, b) = tokio::io::duplex(1 << 20);
    let mut raw = Raw { s: b };
    let mut out = Vec::new();
    Hello {
        min,
        max,
        role: Role::SessionAgent,
    }
    .encode_frame(&mut out)
    .unwrap();
    raw.s.write_all(&out).await.unwrap();
    let (handle, events) = ServiceEnd::accept(a, AgentRole::Session, || Millis(0))
        .await
        .unwrap();
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

#[tokio::test]
async fn image_runs_reach_both_ends() {
    let (a, b) = tokio::io::duplex(1 << 16);
    let (service, agent) = tokio::join!(
        ServiceEnd::accept(a, AgentRole::Session, || Millis(0)),
        AgentEnd::connect(b, AgentRole::Session, || Millis(0)),
    );
    let ((handle, mut events), (reports, mut commands)) = (service.unwrap(), agent.unwrap());
    assert_eq!((handle.version(), reports.version()), (2, 2));

    let png = png_like(300_000);
    reports.send_clipboard_image(&png).unwrap();
    reports.send_clipboard_image(b"not a png at all").unwrap();
    reports
        .send_clipboard_image(&png_like(MAX_IMAGE_LEN as usize + 1))
        .unwrap();
    assert_eq!(
        events.recv().await,
        Some(FromAgent::Clipboard(Copied::Image(png.clone())))
    );
    assert_eq!(
        events.recv().await,
        Some(FromAgent::Clipboard(Copied::NotShared(NotShared::NoText)))
    );
    assert_eq!(
        events.recv().await,
        Some(FromAgent::Clipboard(Copied::NotShared(NotShared::TooLarge)))
    );

    handle.set_clipboard_image(&png).unwrap();
    // Not a PNG, or over the limit: not sent at all.
    handle.set_clipboard_image(b"GIF89a....").unwrap();
    handle
        .set_clipboard_image(&png_like(MAX_IMAGE_LEN as usize + 1))
        .unwrap();
    handle.set_clipboard("after").unwrap();
    assert_eq!(
        commands.recv().await,
        Some(FromService::SetClipboardImage(png))
    );
    assert_eq!(
        commands.recv().await,
        Some(FromService::SetClipboard("after".into()))
    );
}

#[tokio::test]
async fn image_run_with_a_bad_signature_closes_link() {
    let (_handle, mut events, mut raw) = raw_agent().await;
    raw.report(ToService::ClipboardImage(16)).await;
    // Split 3 + 13, the second chunk breaking the signature.
    raw.report(ToService::ClipboardChunk(PNG_SIGNATURE[..3].to_vec()))
        .await;
    let mut rest = PNG_SIGNATURE[3..].to_vec();
    rest[1] = b'X';
    rest.extend([0; 8]);
    raw.report(ToService::ClipboardChunk(rest)).await;
    assert_eq!(events.recv().await, None);
}

#[tokio::test]
async fn image_run_over_the_limit_closes_link() {
    let (_handle, mut events, mut raw) = raw_agent().await;
    raw.raw_frame(
        ipc::message::CLIPBOARD_IMAGE,
        &(MAX_IMAGE_LEN + 1).to_le_bytes(),
    )
    .await;
    assert_eq!(events.recv().await, None);
}

#[tokio::test]
async fn image_run_on_a_version_1_link_closes_it() {
    let (handle, mut events, mut raw) = raw_agent_at(1, 1).await;
    assert_eq!(handle.version(), 1);
    raw.raw_frame(ipc::message::CLIPBOARD_IMAGE, &64u32.to_le_bytes())
        .await;
    assert_eq!(events.recv().await, None);
}

#[tokio::test]
async fn version_1_agent_links_and_text_still_works() {
    let (handle, mut events, mut raw) = raw_agent_at(1, 1).await;
    assert_eq!(handle.version(), 1);
    raw.report(ToService::ClipboardText(2)).await;
    raw.report(ToService::ClipboardChunk(b"hi".to_vec())).await;
    assert_eq!(
        events.recv().await,
        Some(FromAgent::Clipboard(Copied::Text("hi".into())))
    );
    // The service never sends an image to it.
    assert_eq!(
        handle.set_clipboard_image(&png_like(64)),
        Err(SetClipboardError::NoImages)
    );
    handle.set_clipboard("still text").unwrap();
}

#[tokio::test]
async fn agent_on_a_version_1_link_reports_images_as_no_text() {
    // The agent end against a service that speaks only version 1.
    let (a, b) = tokio::io::duplex(1 << 16);
    let mut raw = Raw { s: a };
    let mut out = Vec::new();
    Hello {
        min: 1,
        max: 1,
        role: Role::Service,
    }
    .encode_frame(&mut out)
    .unwrap();
    raw.s.write_all(&out).await.unwrap();
    let (reports, _commands) = AgentEnd::connect(b, AgentRole::Session, || Millis(0))
        .await
        .unwrap();
    assert_eq!(reports.version(), 1);
    reports.send_clipboard_image(&png_like(64)).unwrap();

    // The agent's Hello, then its report: `ClipboardNotShared(NoText)`.
    let mut buf = Vec::new();
    let mut frames = Vec::new();
    while frames.len() < 2 {
        let mut chunk = [0u8; 256];
        let n = raw.s.read(&mut chunk).await.unwrap();
        buf.extend_from_slice(&chunk[..n]);
        while let Ok(Decoded::Frame { frame, consumed }) = frame::decode(&buf) {
            frames.push((frame.ty, frame.payload.to_vec()));
            buf.drain(..consumed);
        }
    }
    assert_eq!(
        frames[1],
        (
            ipc::message::CLIPBOARD_NOT_SHARED,
            vec![NotShared::NoText.code()]
        )
    );
}
