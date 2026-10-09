//! IPC frames decoded as every end would: `Hello`, then each frame as an
//! agent-link message to the service and to an agent, and as a UI command
//! and a UI event. Decoded messages re-encode to the same payload, agent
//! messages are checked against both agent roles, and clipboard and file
//! messages drive the runs and the window at the end that receives them. The
//! first input byte picks the agreed version, 1, 2 or 3, so image runs and
//! file messages are tried both where they are defined and where they are
//! not.

#![no_main]
#![forbid(unsafe_code)]

use ipc::clipboard::{Kind, RunAssembler};
use ipc::files::{FileRun, Window};
use ipc::frame::{self, Decoded, HEADER_LEN};
use ipc::message::{
    CLIPBOARD_IMAGE, ClipboardToAgent, Hello, Role, SET_CLIPBOARD_IMAGE, ToAgent, ToAgentAny,
    ToService, check_agent_role,
};
use ipc::ui::{UiCommand, UiEvent};
use libfuzzer_sys::fuzz_target;

/// One end's clipboard run, file run and window.
#[derive(Default)]
struct Runs {
    clipboard: RunAssembler,
    file: FileRun,
    window: Window,
}

/// Whether `ty` is a file message, defined from version 3.
fn is_file_type(ty: u16) -> bool {
    (0x0040..=0x004f).contains(&ty)
}

fn at_service(runs: &mut Runs, ty: u16, m: &ToService) {
    let _ = match m {
        ToService::ClipboardText(len) => runs.clipboard.start(ty, Kind::Text, *len).map(|_| ()),
        ToService::ClipboardImage(len) => runs.clipboard.start(ty, Kind::Image, *len).map(|_| ()),
        ToService::ClipboardFiles(len) => runs.clipboard.start(ty, Kind::Files, *len).map(|_| ()),
        ToService::ClipboardChunk(b) => runs.clipboard.chunk(ty, b).map(|_| ()),
        ToService::ClipboardNotShared(_) => runs.clipboard.not_in_run(ty),
        ToService::FileContent { index, len } => runs.file.start(ty, *index, *len).map(|_| ()),
        ToService::FileChunk(b) => runs
            .window
            .receive(ty, b.len())
            .and_then(|()| runs.file.chunk(ty, b.len()).map(|_| ())),
        ToService::FileAbandoned { index } => {
            runs.file.abandon(*index);
            Ok(())
        }
        ToService::FileConsumed(n) => {
            runs.window.acknowledge(*n);
            Ok(())
        }
        _ => Ok(()),
    };
}

fn at_agent(runs: &mut Runs, ty: u16, m: &ClipboardToAgent) {
    let _ = match m {
        ClipboardToAgent::Set(len) => runs.clipboard.start(ty, Kind::Text, *len).map(|_| ()),
        ClipboardToAgent::SetImage(len) => runs.clipboard.start(ty, Kind::Image, *len).map(|_| ()),
        ClipboardToAgent::SetFiles(len) => runs.clipboard.start(ty, Kind::Files, *len).map(|_| ()),
        ClipboardToAgent::Chunk(b) => runs.clipboard.chunk(ty, b).map(|_| ()),
        ClipboardToAgent::SetFileContent { index, len } => {
            runs.file.start(ty, *index, *len).map(|_| ())
        }
        ClipboardToAgent::SetFileChunk(b) => runs
            .window
            .receive(ty, b.len())
            .and_then(|()| runs.file.chunk(ty, b.len()).map(|_| ())),
        ClipboardToAgent::AbandonFile { index } => {
            runs.file.abandon(*index);
            Ok(())
        }
        ClipboardToAgent::FileAcked(n) => {
            runs.window.acknowledge(*n);
            Ok(())
        }
        ClipboardToAgent::SendFile { .. } | ClipboardToAgent::StopFile { .. } => Ok(()),
    };
}

fuzz_target!(|data: &[u8]| {
    let Some((&pick, mut rest)) = data.split_first() else {
        return;
    };
    let version = 1 + u16::from(pick % 3);
    let (mut service_runs, mut agent_runs) = (Runs::default(), Runs::default());
    loop {
        match frame::decode(rest) {
            Ok(Decoded::NeedMore) | Err(_) => return,
            Ok(Decoded::Frame { frame, consumed }) => {
                assert!(consumed >= HEADER_LEN && consumed <= rest.len());
                let _ = Hello::decode(frame.payload);
                if let Ok(m) = ToService::decode(frame.ty, frame.payload, version) {
                    assert!(version >= 2 || frame.ty != CLIPBOARD_IMAGE);
                    assert!(version >= 3 || !is_file_type(frame.ty));
                    assert_eq!(m.payload().ok().as_deref(), Some(frame.payload));
                    for role in [Role::SessionAgent, Role::WinlogonAgent] {
                        let _ = check_agent_role(frame.ty, role);
                    }
                    at_service(&mut service_runs, frame.ty, &m);
                }
                if let Ok(m) = ToAgent::decode(frame.ty, frame.payload, version) {
                    assert_eq!(m.payload(), frame.payload);
                }
                if let Ok(ToAgentAny::Clipboard(m)) =
                    ToAgentAny::decode(frame.ty, frame.payload, version)
                {
                    assert!(version >= 2 || frame.ty != SET_CLIPBOARD_IMAGE);
                    assert!(version >= 3 || !is_file_type(frame.ty));
                    assert_eq!(m.payload().ok().as_deref(), Some(frame.payload));
                    at_agent(&mut agent_runs, frame.ty, &m);
                }
                if let Ok(m) = UiCommand::decode(frame.ty, frame.payload, version) {
                    assert_eq!(m.payload().ok().as_deref(), Some(frame.payload));
                }
                if let Ok(m) = UiEvent::decode(frame.ty, frame.payload, version) {
                    assert_eq!(m.payload().ok().as_deref(), Some(frame.payload));
                }
                rest = &rest[consumed..];
            }
        }
    }
});
