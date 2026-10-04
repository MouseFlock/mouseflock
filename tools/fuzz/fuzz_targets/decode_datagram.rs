//! Datagram decoding, before and after a version is agreed, then each
//! datagram's payload decoded by its type.

#![no_main]
#![forbid(unsafe_code)]

use libfuzzer_sys::fuzz_target;
use proto::frame::Frame;
use proto::hello::Hello;
use proto::input::InputMessage;
use proto::registry;
use proto::session::{DatagramEvent, Session};

fuzz_target!(|data: &[u8]| {
    let fresh = Session::new(Hello::this_release());
    let _ = fresh.on_datagram(data);

    let mut agreed = Session::new(Hello::this_release());
    let payload = Hello::this_release().payload();
    let hello = Frame { ty: registry::HELLO, payload: &payload };
    assert!(agreed.on_control_frame(hello).is_ok());
    if let Ok(DatagramEvent::Message(m)) = agreed.on_datagram(data)
        && let Ok(decoded) = InputMessage::decode(m.info.ty, m.payload)
    {
        // A decoded datagram re-encodes to the same bytes.
        let mut out = Vec::new();
        assert!(decoded.encode(&mut out).is_ok());
        assert_eq!(out, data);
    }
});
