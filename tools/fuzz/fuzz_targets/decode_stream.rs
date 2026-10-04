//! Stream decoding plus the session state machine, then each message's payload
//! decoded by its type. The first input byte picks whether the session has
//! already agreed a version and, bit by bit, whether each frame goes to the
//! control stream or another stream.

#![no_main]
#![forbid(unsafe_code)]

use libfuzzer_sys::fuzz_target;
use proto::frame::{self, Decoded, Frame, HEADER_LEN};
use proto::hello::Hello;
use proto::input::InputMessage;
use proto::pairing::PairingMessage;
use proto::registry;
use proto::session::{ControlEvent, Message, Session, StreamEvent};

/// A decoded payload re-encodes to the same bytes.
fn decode_payload(m: Message<'_>) {
    if m.info.ty == registry::HELLO {
        return;
    }
    if let Ok(decoded) = PairingMessage::decode(m.info.ty, m.payload) {
        let mut out = Vec::new();
        assert!(decoded.encode_frame(&mut out).is_ok());
        assert_eq!(out.get(HEADER_LEN..), Some(m.payload));
    }
    if let Ok(decoded) = InputMessage::decode(m.info.ty, m.payload) {
        assert_eq!(decoded.ty(), m.info.ty);
        assert_eq!(decoded.payload(), m.payload);
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((&routing, mut rest)) = data.split_first() else {
        return;
    };
    let mut session = Session::new(Hello::this_release());
    if routing & 1 == 1 {
        let payload = Hello::this_release().payload();
        let hello = Frame { ty: registry::HELLO, payload: &payload };
        assert!(session.on_control_frame(hello).is_ok());
    }
    let mut bit = 1;
    loop {
        match frame::decode(rest) {
            Ok(Decoded::NeedMore) | Err(_) => return,
            Ok(Decoded::Frame { frame, consumed }) => {
                assert!(consumed >= HEADER_LEN && consumed <= rest.len());
                bit = (bit + 1) % 8;
                let message = if routing >> bit & 1 == 0 {
                    match session.on_control_frame(frame) {
                        Ok(ControlEvent::Message(m)) => Some(m),
                        Ok(ControlEvent::Agreed(_)) => None,
                        Err(_) => return,
                    }
                } else {
                    match session.on_stream_frame(frame) {
                        Ok(StreamEvent::Message(m)) => Some(m),
                        Ok(StreamEvent::Wait) => None,
                        Err(_) => return,
                    }
                };
                if let Some(m) = message {
                    decode_payload(m);
                }
                rest = &rest[consumed..];
            }
        }
    }
});
