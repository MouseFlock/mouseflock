//! IPC frames decoded as every end would: `Hello`, then each frame as an
//! agent-link message to the service and to an agent, and as a UI command
//! and a UI event. Decoded messages re-encode to the same payload, agent
//! messages are checked against both agent roles, and clipboard messages
//! drive a run at the end that receives them.

#![no_main]
#![forbid(unsafe_code)]

use ipc::clipboard::RunAssembler;
use ipc::frame::{self, Decoded, HEADER_LEN};
use ipc::message::{
    ClipboardToAgent, Hello, Role, ToAgent, ToAgentAny, ToService, check_agent_role,
};
use ipc::ui::{UiCommand, UiEvent};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut rest = data;
    let (mut service_runs, mut agent_runs) = (RunAssembler::default(), RunAssembler::default());
    loop {
        match frame::decode(rest) {
            Ok(Decoded::NeedMore) | Err(_) => return,
            Ok(Decoded::Frame { frame, consumed }) => {
                assert!(consumed >= HEADER_LEN && consumed <= rest.len());
                let _ = Hello::decode(frame.payload);
                if let Ok(m) = ToService::decode(frame.ty, frame.payload, 1) {
                    assert_eq!(m.payload().ok().as_deref(), Some(frame.payload));
                    for role in [Role::SessionAgent, Role::WinlogonAgent] {
                        let _ = check_agent_role(frame.ty, role);
                    }
                    let _ = match &m {
                        ToService::ClipboardText(len) => service_runs.start(frame.ty, *len).map(|_| ()),
                        ToService::ClipboardChunk(b) => service_runs.chunk(frame.ty, b).map(|_| ()),
                        ToService::ClipboardNotShared(_) => service_runs.not_in_run(frame.ty),
                        _ => Ok(()),
                    };
                }
                if let Ok(m) = ToAgent::decode(frame.ty, frame.payload, 1) {
                    assert_eq!(m.payload(), frame.payload);
                }
                if let Ok(ToAgentAny::Clipboard(m)) = ToAgentAny::decode(frame.ty, frame.payload, 1) {
                    assert_eq!(m.payload().ok().as_deref(), Some(frame.payload));
                    let _ = match &m {
                        ClipboardToAgent::Set(len) => agent_runs.start(frame.ty, *len),
                        ClipboardToAgent::Chunk(b) => agent_runs.chunk(frame.ty, b),
                    };
                }
                if let Ok(m) = UiCommand::decode(frame.ty, frame.payload, 1) {
                    assert_eq!(m.payload().ok().as_deref(), Some(frame.payload));
                }
                if let Ok(m) = UiEvent::decode(frame.ty, frame.payload, 1) {
                    assert_eq!(m.payload().ok().as_deref(), Some(frame.payload));
                }
                rest = &rest[consumed..];
            }
        }
    }
});
