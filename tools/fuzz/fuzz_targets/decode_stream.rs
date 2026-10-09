//! Stream decoding plus the session state machine, then each message's payload
//! decoded by its type. The first input byte picks whether the session has
//! already agreed a version and, bit by bit, whether each frame goes to the
//! control stream or another stream. The second byte picks the peer's highest
//! version, 1, 2 or 3, so clipboard streams are read as a version 1 peer (text
//! only), a version 2 peer (text and images) or a version 3 peer (text, images,
//! file manifests and file contents) would read them. Every chunk is also
//! parsed as a file manifest; one that parses re-encodes to the same bytes.

#![no_main]
#![forbid(unsafe_code)]

use libfuzzer_sys::fuzz_target;
use proto::clipboard::{self, ClipboardMessage, TransferReader, TransferStep};
use proto::files::{self, FileContentRequest, FileEntry, Manifest};
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
    if let Ok(decoded) = ClipboardMessage::decode(m.info.ty, m.payload) {
        assert_eq!(decoded.ty(), m.info.ty);
        assert_eq!(decoded.payload(), m.payload);
    }
    if m.info.ty == registry::FILE_CONTENT_REQUEST
        && let Ok(decoded) = FileContentRequest::decode(m.payload)
    {
        assert_eq!(decoded.payload(), m.payload);
    }
}

/// A manifest that parses lists only safe paths and re-encodes exactly.
fn parse_manifest(bytes: &[u8]) {
    if let Ok(m) = Manifest::parse(bytes) {
        let entries: Vec<FileEntry<'_>> = m.entries().collect();
        assert_eq!(entries.len(), m.count() as usize);
        assert!(entries.iter().all(|e| files::check_path(e.path).is_ok()));
        let mut out = Vec::new();
        assert!(files::encode_manifest(entries, &mut out).is_ok());
        assert_eq!(out, bytes);
    }
}

/// A clipboard-stream message through the reader: a chunk is exactly the
/// frame's payload, and finishing at any point is total.
fn read_transfer(reader: &mut TransferReader, m: Message<'_>) {
    if let Ok(TransferStep::Chunk(c)) = reader.on_message(m.info.ty, m.payload) {
        assert_eq!(c, m.payload);
        let _ = clipboard::text(c);
        parse_manifest(c);
    }
    let _ = reader.finish();
}

fuzz_target!(|data: &[u8]| {
    let Some((&routing, rest)) = data.split_first() else {
        return;
    };
    let Some((&peer, mut rest)) = rest.split_first() else {
        return;
    };
    let peer_max = 1 + u16::from(peer % 3);
    let version = peer_max.min(proto::PROTOCOL_VERSION);
    let mut session = Session::new(Hello::this_release());
    if routing & 1 == 1 {
        let payload = Hello {
            min: 1,
            max: peer_max,
        }
        .payload();
        let hello = Frame {
            ty: registry::HELLO,
            payload: &payload,
        };
        assert!(session.on_control_frame(hello).is_ok());
    }
    let mut bit = 1;
    let mut transfer = TransferReader::new(version);
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
                        Ok(StreamEvent::Message(m)) => {
                            read_transfer(&mut transfer, m);
                            Some(m)
                        }
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
