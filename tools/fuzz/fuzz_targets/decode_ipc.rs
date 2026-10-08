//! IPC frames decoded as every end would: `Hello`, then each frame as an
//! agent-link message to the service and to an agent, and as a UI command
//! and a UI event. Decoded messages re-encode to the same payload, agent
//! messages are checked against both agent roles, and clipboard messages
//! drive a run at the end that receives them. The first input byte's low bit
//! picks the agreed version, 1 or 2, so image runs are tried both where they
//! are defined and where they are not.

#![no_main]
#![forbid(unsafe_code)]

use ipc::clipboard::{Kind, RunAssembler};
use ipc::frame::{self, Decoded, HEADER_LEN};
use ipc::message::{
    CLIPBOARD_IMAGE, ClipboardToAgent, Hello, Role, SET_CLIPBOARD_IMAGE, ToAgent, ToAgentAny,
    ToService, check_agent_role,
};
use ipc::ui::{UiCommand, UiEvent};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&pick, mut rest)) = data.split_first() else {
        return;
    };
    let version = 1 + u16::from(pick & 1);
    let (mut service_runs, mut agent_runs) = (RunAssembler::default(), RunAssembler::default());
    loop {
        match frame::decode(rest) {
            Ok(Decoded::NeedMore) | Err(_) => return,
            Ok(Decoded::Frame { frame, consumed }) => {
                assert!(consumed >= HEADER_LEN && consumed <= rest.len());
                let _ = Hello::decode(frame.payload);
                if let Ok(m) = ToService::decode(frame.ty, frame.payload, version) {
                    assert!(version >= 2 || frame.ty != CLIPBOARD_IMAGE);
                    assert_eq!(m.payload().ok().as_deref(), Some(frame.payload));
                    for role in [Role::SessionAgent, Role::WinlogonAgent] {
                        let _ = check_agent_role(frame.ty, role);
                    }
                    let _ = match &m {
                        ToService::ClipboardText(len) => {
                            service_runs.start(frame.ty, Kind::Text, *len).map(|_| ())
                        }
                        ToService::ClipboardImage(len) => {
                            service_runs.start(frame.ty, Kind::Image, *len).map(|_| ())
                        }
                        ToService::ClipboardChunk(b) => service_runs.chunk(frame.ty, b).map(|_| ()),
                        ToService::ClipboardNotShared(_) => service_runs.not_in_run(frame.ty),
                        _ => Ok(()),
                    };
                }
                if let Ok(m) = ToAgent::decode(frame.ty, frame.payload, version) {
                    assert_eq!(m.payload(), frame.payload);
                }
                if let Ok(ToAgentAny::Clipboard(m)) =
                    ToAgentAny::decode(frame.ty, frame.payload, version)
                {
                    assert!(version >= 2 || frame.ty != SET_CLIPBOARD_IMAGE);
                    assert_eq!(m.payload().ok().as_deref(), Some(frame.payload));
                    let _ = match &m {
                        ClipboardToAgent::Set(len) => agent_runs.start(frame.ty, Kind::Text, *len),
                        ClipboardToAgent::SetImage(len) => {
                            agent_runs.start(frame.ty, Kind::Image, *len)
                        }
                        ClipboardToAgent::Chunk(b) => agent_runs.chunk(frame.ty, b),
                    };
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
