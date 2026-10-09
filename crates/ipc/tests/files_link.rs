//! File copies on the agent link, both ends over duplex streams (specs:
//! agent-link, ipc-protocol, file-transfer): manifest runs with their
//! numbers, file runs in both directions, and the window.

use ipc::clipboard::{Copied, NotShared};
use ipc::files::FILE_WINDOW;
use ipc::frame::{self, Decoded};
use ipc::link::{
    AgentEnd, AgentHandle, AgentRole, FromAgent, FromService, ServiceEnd, ServiceHandle,
    SetClipboardError,
};
use ipc::message::{self, ClipboardToAgent, Hello, Role, ToService};
use model::{InputEvent, Key, Millis, Origin};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::sync::mpsc::UnboundedReceiver;

const MIB: usize = 1 << 20;

/// Both ends of a session agent's link at this release.
async fn linked() -> (
    AgentHandle,
    UnboundedReceiver<FromAgent>,
    ServiceHandle,
    UnboundedReceiver<FromService>,
) {
    let (a, b) = tokio::io::duplex(1 << 16);
    let (service, agent) = tokio::join!(
        ServiceEnd::accept(a, AgentRole::Session, || Millis(0)),
        AgentEnd::connect(b, AgentRole::Session, || Millis(0)),
    );
    let ((handle, events), (reports, commands)) = (service.unwrap(), agent.unwrap());
    assert_eq!((handle.version(), reports.version()), (3, 3));
    (handle, events, reports, commands)
}

/// An agent driven by hand, whose `Hello` advertises `min..=max` as `role`;
/// it has answered a `Ping`, so the service passes its reports on at once.
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

async fn raw_agent(max: u16, role: AgentRole) -> (AgentHandle, UnboundedReceiver<FromAgent>, Raw) {
    let (a, b) = tokio::io::duplex(4 << 20);
    let mut raw = Raw { s: b };
    let mut out = Vec::new();
    Hello {
        min: 1,
        max,
        role: role.role(),
    }
    .encode_frame(&mut out)
    .unwrap();
    raw.s.write_all(&out).await.unwrap();
    let (handle, events) = ServiceEnd::accept(a, role, || Millis(0)).await.unwrap();
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

fn manifest(seed: u8) -> Vec<u8> {
    (0..200_000).map(|i| (i as u8) ^ seed).collect()
}

#[tokio::test]
async fn manifests_travel_both_ways_numbered() {
    let (handle, mut events, reports, mut commands) = linked().await;
    assert_eq!(reports.send_clipboard_files(&manifest(1)), Ok(Some(1)));
    assert_eq!(reports.send_clipboard_files(&manifest(2)), Ok(Some(2)));
    for (copy, seed) in [(1, 1), (2, 2)] {
        assert_eq!(
            events.recv().await,
            Some(FromAgent::ClipboardFiles {
                copy,
                manifest: manifest(seed)
            })
        );
    }
    assert_eq!(handle.set_clipboard_files(&manifest(3)), Ok(Some(1)));
    assert_eq!(
        commands.recv().await,
        Some(FromService::SetClipboardFiles {
            set: 1,
            manifest: manifest(3)
        })
    );
}

#[tokio::test]
async fn a_served_file_reaches_the_service_within_the_window() {
    let (handle, mut events, reports, mut commands) = linked().await;
    handle.send_file(1, 4).unwrap();
    assert_eq!(
        commands.recv().await,
        Some(FromService::SendFile { copy: 1, index: 4 })
    );
    let body: Vec<u8> = (0..3 * MIB).map(|i| i as u8).collect();
    reports.start_file(4, body.len() as u64).unwrap();
    assert_eq!(
        events.recv().await,
        Some(FromAgent::FileStarted {
            index: 4,
            len: body.len() as u64
        })
    );
    // The agent sends a window's worth, waits for the service's
    // acknowledgements, and goes on: 3 MiB through a 1 MiB window.
    let mut sent = 0;
    let mut got = Vec::new();
    while got.len() < body.len() {
        while sent < body.len() && sent - got.len() < FILE_WINDOW as usize {
            let end = (sent + 65_536).min(body.len());
            reports.file_chunk(&body[sent..end]).unwrap();
            sent = end;
        }
        match events.recv().await.unwrap() {
            FromAgent::FileBytes(b) => {
                handle.ack_file(b.len() as u32).unwrap();
                got.extend(b);
            }
            other => panic!("{other:?}"),
        }
        while let Ok(FromService::FileAcked(_)) = commands.try_recv() {}
    }
    assert_eq!(events.recv().await, Some(FromAgent::FileEnded { index: 4 }));
    assert!(got == body, "the file arrived changed");
}

#[tokio::test]
async fn a_wanted_file_reaches_the_agent() {
    let (handle, mut events, reports, mut commands) = linked().await;
    reports.want_file(1, 2).unwrap();
    assert_eq!(
        events.recv().await,
        Some(FromAgent::FileWanted { set: 1, index: 2 })
    );
    handle.start_file(2, 5).unwrap();
    handle.file_chunk(b"hel").unwrap();
    handle.file_chunk(b"lo").unwrap();
    assert_eq!(
        commands.recv().await,
        Some(FromService::FileStarted { index: 2, len: 5 })
    );
    assert_eq!(
        commands.recv().await,
        Some(FromService::FileBytes(b"hel".to_vec()))
    );
    assert_eq!(
        commands.recv().await,
        Some(FromService::FileBytes(b"lo".to_vec()))
    );
    assert_eq!(
        commands.recv().await,
        Some(FromService::FileEnded { index: 2 })
    );
    reports.file_consumed(5).unwrap();
    assert_eq!(events.recv().await, Some(FromAgent::FileConsumed(5)));
    // An empty file starts and ends at once.
    handle.start_file(3, 0).unwrap();
    assert_eq!(
        commands.recv().await,
        Some(FromService::FileStarted { index: 3, len: 0 })
    );
    assert_eq!(
        commands.recv().await,
        Some(FromService::FileEnded { index: 3 })
    );
}

#[tokio::test]
async fn abandoning_ends_a_run_and_a_new_one_may_start() {
    let (handle, mut events, reports, mut commands) = linked().await;
    reports.start_file(1, 100).unwrap();
    reports.file_chunk(&[1; 10]).unwrap();
    reports.abandon_file(1).unwrap();
    reports.start_file(2, 1).unwrap();
    reports.file_chunk(&[2]).unwrap();
    let mut seen = Vec::new();
    while seen.len() < 6 {
        seen.push(events.recv().await.unwrap());
    }
    assert_eq!(
        seen,
        [
            FromAgent::FileStarted { index: 1, len: 100 },
            FromAgent::FileBytes(vec![1; 10]),
            FromAgent::FileAbandoned { index: 1 },
            FromAgent::FileStarted { index: 2, len: 1 },
            FromAgent::FileBytes(vec![2]),
            FromAgent::FileEnded { index: 2 },
        ]
    );
    handle.abandon_file(9).unwrap();
    assert_eq!(
        commands.recv().await,
        Some(FromService::FileAbandoned { index: 9 })
    );
    // Asking the other side to stop is a message of its own each way.
    handle.stop_file(4).unwrap();
    assert_eq!(
        commands.recv().await,
        Some(FromService::StopFile { index: 4 })
    );
    reports.unwant_file(5).unwrap();
    assert_eq!(
        events.recv().await,
        Some(FromAgent::FileUnwanted { index: 5 })
    );
}

#[tokio::test]
async fn copies_and_input_interleave_with_a_file_run() {
    let (_handle, mut events, mut raw) = raw_agent(3, AgentRole::Session).await;
    raw.report(ToService::FileContent { index: 0, len: 4 })
        .await;
    raw.report(ToService::FileChunk(b"ab".to_vec())).await;
    raw.report(ToService::Input {
        at: Millis(1),
        input: InputEvent::Key {
            key: Key(4),
            down: true,
        },
        origin: Origin::Physical,
    })
    .await;
    raw.report(ToService::ClipboardText(2)).await;
    raw.report(ToService::ClipboardChunk(b"hi".to_vec())).await;
    raw.report(ToService::FileChunk(b"cd".to_vec())).await;
    let mut seen = Vec::new();
    while seen.len() < 5 {
        seen.push(events.recv().await.unwrap());
    }
    assert!(matches!(seen[2], FromAgent::Input { .. }));
    assert_eq!(seen[3], FromAgent::Clipboard(Copied::Text("hi".into())));
    assert_eq!(seen[4], FromAgent::FileBytes(b"cd".to_vec()));
    assert_eq!(events.recv().await, Some(FromAgent::FileEnded { index: 0 }));
}

#[tokio::test]
async fn bytes_past_the_window_close_the_link() {
    let (_handle, mut events, mut raw) = raw_agent(3, AgentRole::Session).await;
    raw.report(ToService::FileContent {
        index: 0,
        len: 2 * FILE_WINDOW,
    })
    .await;
    // Exactly the window, unacknowledged, is fine.
    for _ in 0..FILE_WINDOW as usize / 65_536 {
        raw.report(ToService::FileChunk(vec![0; 65_536])).await;
    }
    raw.report(ToService::FileChunk(vec![0; 1])).await;
    let mut chunks = 0;
    while let Some(e) = events.recv().await {
        if matches!(e, FromAgent::FileBytes(_)) {
            chunks += 1;
        }
    }
    assert_eq!(chunks, FILE_WINDOW as usize / 65_536, "link closed early");
}

#[tokio::test]
async fn file_run_breaches_close_the_link() {
    for breach in 0..3 {
        let (_handle, mut events, mut raw) = raw_agent(3, AgentRole::Session).await;
        match breach {
            // A chunk outside a run.
            0 => raw.report(ToService::FileChunk(vec![1])).await,
            // More bytes than announced.
            1 => {
                raw.report(ToService::FileContent { index: 0, len: 1 })
                    .await;
                raw.report(ToService::FileChunk(vec![1, 2])).await;
            }
            // A second run before the first ends.
            _ => {
                raw.report(ToService::FileContent { index: 0, len: 9 })
                    .await;
                raw.report(ToService::FileContent { index: 1, len: 9 })
                    .await;
            }
        }
        while events.recv().await.is_some() {}
    }
}

#[tokio::test]
async fn file_messages_on_a_version_2_link_close_it() {
    for ty in [
        message::CLIPBOARD_FILES,
        message::FILE_WANTED,
        message::FILE_CONTENT,
        message::FILE_CONSUMED,
    ] {
        let (handle, mut events, mut raw) = raw_agent(2, AgentRole::Session).await;
        assert_eq!(handle.version(), 2);
        let payload: &[u8] = match ty {
            message::FILE_WANTED => &[0; 8],
            message::FILE_CONTENT => &[0; 12],
            _ => &[0; 4],
        };
        raw.raw_frame(ty, payload).await;
        assert_eq!(events.recv().await, None, "type {ty:#06x}");
    }
}

#[tokio::test]
async fn the_service_sends_no_files_to_a_version_2_agent() {
    let (handle, _events, _raw) = raw_agent(2, AgentRole::Session).await;
    assert_eq!(
        handle.set_clipboard_files(b"m"),
        Err(SetClipboardError::NoFiles)
    );
    assert_eq!(handle.send_file(1, 0), Err(SetClipboardError::NoFiles));
    assert_eq!(handle.start_file(0, 1), Err(SetClipboardError::NoFiles));
    handle.set_clipboard("still text").unwrap();
}

#[tokio::test]
async fn an_agent_on_a_version_2_link_reports_files_as_no_text() {
    let (a, b) = tokio::io::duplex(1 << 16);
    let mut raw = Raw { s: a };
    let mut out = Vec::new();
    Hello {
        min: 1,
        max: 2,
        role: Role::Service,
    }
    .encode_frame(&mut out)
    .unwrap();
    raw.s.write_all(&out).await.unwrap();
    let (reports, _commands) = AgentEnd::connect(b, AgentRole::Session, || Millis(0))
        .await
        .unwrap();
    assert_eq!(reports.version(), 2);
    assert_eq!(reports.send_clipboard_files(b"manifest"), Ok(None));
    // File messages are dropped on this link, never sent.
    reports.want_file(1, 0).unwrap();
    reports.file_consumed(9).unwrap();
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
            message::CLIPBOARD_NOT_SHARED,
            vec![NotShared::NoText.code()]
        )
    );
}

#[tokio::test]
async fn a_winlogon_link_never_carries_files() {
    let (handle, mut events, mut raw) = raw_agent(3, AgentRole::Winlogon).await;
    assert_eq!(
        handle.set_clipboard_files(b"m"),
        Err(SetClipboardError::NoClipboard)
    );
    assert_eq!(handle.send_file(1, 0), Err(SetClipboardError::NoClipboard));
    raw.report(ToService::ClipboardFiles(1)).await;
    assert_eq!(events.recv().await, None);
}

#[tokio::test]
async fn the_agent_closes_on_set_bytes_past_the_window() {
    let (handle, _events, _reports, mut commands) = linked().await;
    handle.start_file(0, 2 * FILE_WINDOW).unwrap();
    handle
        .file_chunk(&vec![0; FILE_WINDOW as usize + 1])
        .unwrap();
    let mut bytes = 0;
    while let Some(c) = commands.recv().await {
        if let FromService::FileBytes(b) = c {
            bytes += b.len();
        }
    }
    assert!(bytes as u64 <= FILE_WINDOW);
    // Acknowledged bytes reopen it: a second link, acknowledging as it goes.
    let (handle, _events, reports, mut commands) = linked().await;
    handle.start_file(0, 2 * FILE_WINDOW).unwrap();
    let mut got = 0u64;
    handle.file_chunk(&vec![0; FILE_WINDOW as usize]).unwrap();
    while got < FILE_WINDOW {
        if let Some(FromService::FileBytes(b)) = commands.recv().await {
            got += b.len() as u64;
            reports.file_consumed(b.len() as u32).unwrap();
        }
    }
    handle.file_chunk(&vec![0; FILE_WINDOW as usize]).unwrap();
    while got < 2 * FILE_WINDOW {
        match commands.recv().await {
            Some(FromService::FileBytes(b)) => {
                got += b.len() as u64;
                reports.file_consumed(b.len() as u32).unwrap();
            }
            Some(_) => {}
            None => panic!("the link closed within the window"),
        }
    }
    let _ = ClipboardToAgent::FileAcked(0);
}
