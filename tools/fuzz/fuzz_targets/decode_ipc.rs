//! IPC frames decoded as both ends would: `Hello`, then each frame as a
//! message to the service and as one to an agent. Decoded messages
//! re-encode to the same payload.

#![no_main]
#![forbid(unsafe_code)]

use ipc::frame::{self, Decoded, HEADER_LEN};
use ipc::message::{Hello, ToAgent, ToService};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut rest = data;
    loop {
        match frame::decode(rest) {
            Ok(Decoded::NeedMore) | Err(_) => return,
            Ok(Decoded::Frame { frame, consumed }) => {
                assert!(consumed >= HEADER_LEN && consumed <= rest.len());
                let _ = Hello::decode(frame.payload);
                if let Ok(m) = ToService::decode(frame.ty, frame.payload, 1) {
                    assert_eq!(m.payload().ok().as_deref(), Some(frame.payload));
                }
                if let Ok(m) = ToAgent::decode(frame.ty, frame.payload, 1) {
                    assert_eq!(m.payload(), frame.payload);
                }
                rest = &rest[consumed..];
            }
        }
    }
});
