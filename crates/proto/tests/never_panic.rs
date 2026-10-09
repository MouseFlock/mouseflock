//! Decoding is total: arbitrary input never panics, and every decoded frame
//! lies inside its input. Deterministic, so it runs in `cargo test` on stable;
//! `tools/fuzz` covers the same drivers with coverage guidance.

use proto::clipboard::{self, ClipboardMessage, TransferReader, TransferStep};
use proto::datagram::MAX_DATAGRAM_LEN;
use proto::files::{self, FileContentRequest, FileEntry, Manifest};
use proto::frame::{self, Decoded, HEADER_LEN};
use proto::hello::Hello;
use proto::input::{
    Button, ButtonInput, EdgeExit, FocusEnter, InputMessage, KeyInput, Mark, Side, WheelInput,
};
use proto::pairing::{Change, MemberRecord, Name, PairConfirm, PairPake, PairingMessage};
use proto::registry;
use proto::session::{ControlEvent, DatagramEvent, Message, Session, StreamEvent};

const ITERATIONS: usize = 300_000;

/// xorshift64: tiny, deterministic, good enough to shake a parser.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.next() as u8).collect()
    }

    fn bytes_below(&mut self, max_len: usize) -> Vec<u8> {
        let len = self.below(max_len);
        self.bytes(len)
    }
}

/// Feeds `input` as one stream: decodes frames until it needs more input or
/// fails, handing each to the session as a control or other-stream frame.
fn drive_stream(session: &mut Session, input: &[u8], rng: &mut Rng) {
    let mut rest = input;
    let mut transfer = TransferReader::new(proto::PROTOCOL_VERSION);
    loop {
        match frame::decode(rest) {
            Ok(Decoded::NeedMore) | Err(_) => return,
            Ok(Decoded::Frame { frame, consumed }) => {
                assert!(consumed >= HEADER_LEN && consumed <= rest.len());
                assert_eq!(consumed - HEADER_LEN, frame.payload.len());
                let message = if rng.below(2) == 0 {
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
}

/// Decodes a message's payload by its type; a decoded message re-encodes to
/// the same payload.
fn decode_payload(m: Message<'_>) {
    if m.info.ty == registry::HELLO {
        return;
    }
    if let Ok(decoded) = PairingMessage::decode(m.info.ty, m.payload) {
        let mut out = Vec::new();
        decoded.encode_frame(&mut out).unwrap();
        assert_eq!(&out[HEADER_LEN..], m.payload);
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

/// Parses `bytes` as a manifest; one that parses re-encodes to the same
/// bytes, and its entries all pass the path check.
fn parse_manifest(bytes: &[u8]) {
    if let Ok(m) = Manifest::parse(bytes) {
        let entries: Vec<FileEntry<'_>> = m.entries().collect();
        assert_eq!(entries.len() as u32, m.count());
        assert!(entries.iter().all(|e| files::check_path(e.path).is_ok()));
        let mut out = Vec::new();
        files::encode_manifest(entries, &mut out).unwrap();
        assert_eq!(out, bytes);
    }
}

/// Feeds a clipboard-stream message to the reader: a chunk is exactly the
/// frame's payload, and finishing at any point never panics.
fn read_transfer(reader: &mut TransferReader, m: Message<'_>) {
    if let Ok(TransferStep::Chunk(c)) = reader.on_message(m.info.ty, m.payload) {
        assert_eq!(c, m.payload);
        let _ = clipboard::text(c);
        parse_manifest(c);
    }
    let _ = reader.finish();
}

fn agreed_session() -> Session {
    let mut session = Session::new(Hello::this_release());
    let payload = Hello::this_release().payload();
    session
        .on_control_frame(frame::Frame {
            ty: registry::HELLO,
            payload: &payload,
        })
        .unwrap();
    session
}

/// A few valid frames back to back: a `Hello`, then assorted types, half of
/// them well-formed pairing messages.
fn valid_frames(rng: &mut Rng) -> Vec<u8> {
    let mut out = Vec::new();
    Hello::this_release().encode_frame(&mut out).unwrap();
    for _ in 0..rng.below(4) {
        let pick = rng.below(7);
        if pick == 0 {
            pairing_message(rng).encode_frame(&mut out).unwrap();
        } else if pick == 1 {
            let m = input_message(rng);
            frame::encode(m.ty(), &m.payload(), &mut out).unwrap();
        } else if pick == 2 {
            let id = rng.next();
            let m = if rng.below(2) == 0 {
                ClipboardMessage::Offer { id }
            } else {
                ClipboardMessage::Request { id }
            };
            m.encode(&mut out).unwrap();
        } else if pick == 3 {
            let text = rng.bytes_below(24);
            clipboard::encode_data(rng.next(), text.len() as u32, &mut out).unwrap();
            for chunk in text.chunks(1 + rng.below(8)) {
                clipboard::encode_chunk(chunk, &mut out).unwrap();
            }
        } else if pick == 4 {
            let mut manifest = Vec::new();
            files::encode_manifest(manifest_entries(rng), &mut manifest).unwrap();
            clipboard::encode_files_data(rng.next(), manifest.len() as u32, &mut out).unwrap();
            for chunk in manifest.chunks(1 + rng.below(16)) {
                clipboard::encode_chunk(chunk, &mut out).unwrap();
            }
        } else if pick == 5 {
            if rng.below(2) == 0 {
                let r = FileContentRequest {
                    id: rng.next(),
                    index: rng.next() as u32,
                };
                r.encode(&mut out).unwrap();
            } else {
                let body = rng.bytes_below(24);
                files::encode_content_data(
                    rng.next(),
                    rng.next() as u32,
                    body.len() as u64,
                    &mut out,
                )
                .unwrap();
                for chunk in body.chunks(1 + rng.below(8)) {
                    clipboard::encode_chunk(chunk, &mut out).unwrap();
                }
            }
        } else {
            let ty = rng.below(0x19) as u16;
            let payload = rng.bytes_below(12);
            frame::encode(ty, &payload, &mut out).unwrap();
        }
    }
    out
}

const NAMES: [&str; 3] = ["A", "DESK-B", "Living room PC"];

const PATHS: [&str; 5] = ["a", "a/b.txt", "Photos", "Photos/sub/x.png", "名前.txt"];

fn manifest_entries(rng: &mut Rng) -> Vec<FileEntry<'static>> {
    (0..=rng.below(4))
        .map(|_| {
            let is_dir = rng.below(2) == 0;
            FileEntry {
                path: PATHS[rng.below(PATHS.len())],
                is_dir,
                size: if is_dir { 0 } else { rng.next() >> 40 },
            }
        })
        .collect()
}

fn array<const N: usize>(rng: &mut Rng) -> [u8; N] {
    rng.bytes(N).try_into().unwrap()
}

fn pairing_message(rng: &mut Rng) -> PairingMessage<'static> {
    let name = Name::new(NAMES[rng.below(NAMES.len())]).unwrap();
    match rng.below(4) {
        0 => PairingMessage::Pake(PairPake(array(rng))),
        1 => PairingMessage::Confirm(PairConfirm {
            tag: array(rng),
            name,
        }),
        2 => PairingMessage::Record(MemberRecord {
            subject: array(rng),
            epoch: rng.next(),
            signer: array(rng),
            change: if rng.below(2) == 0 {
                Change::Add(name)
            } else {
                Change::Remove
            },
            signature: array(rng),
        }),
        _ => PairingMessage::RecordsDone,
    }
}

fn mark(rng: &mut Rng) -> Mark {
    Mark {
        seq: rng.next() as u32,
        dx: rng.next() as i32,
        dy: rng.next() as i32,
    }
}

/// Any engine message, `Motion` included: its frame or datagram encoding
/// is chosen by the caller.
fn input_message(rng: &mut Rng) -> InputMessage {
    const SIDES: [Side; 4] = [Side::Left, Side::Right, Side::Top, Side::Bottom];
    const BUTTONS: [Button; 5] = [
        Button::Left,
        Button::Right,
        Button::Middle,
        Button::X1,
        Button::X2,
    ];
    let side = SIDES[rng.below(4)];
    match rng.below(11) {
        0 => InputMessage::FocusEnter(FocusEnter {
            side,
            fraction: rng.next() as u16,
            from: array(rng),
        }),
        1 => InputMessage::FocusRefused,
        2 => InputMessage::EdgeExit(EdgeExit {
            target: array(rng),
            side,
            fraction: rng.next() as u16,
        }),
        3 => InputMessage::TakenBack,
        4 => InputMessage::FocusWithdrawn,
        5 => InputMessage::ForgetSide(side),
        6 => InputMessage::Key(KeyInput {
            mark: mark(rng),
            usage: rng.next() as u16,
            down: rng.below(2) == 0,
        }),
        7 => InputMessage::Button(ButtonInput {
            mark: mark(rng),
            button: BUTTONS[rng.below(5)],
            down: rng.below(2) == 0,
        }),
        8 => InputMessage::Wheel(WheelInput {
            mark: mark(rng),
            dx: rng.next() as i32,
            dy: rng.next() as i32,
        }),
        9 => InputMessage::SecureAttention,
        _ => InputMessage::Motion(mark(rng)),
    }
}

/// Decodes a datagram through both sessions; a decoded `Motion` re-encodes
/// to the same bytes.
fn drive_datagram(fresh: &Session, agreed: &Session, input: &[u8]) {
    let _ = fresh.on_datagram(input);
    if let Ok(DatagramEvent::Message(m)) = agreed.on_datagram(input)
        && let Ok(decoded) = InputMessage::decode(m.info.ty, m.payload)
    {
        let mut out = Vec::new();
        decoded.encode(&mut out).unwrap();
        assert_eq!(out, input);
    }
}

fn mutate(input: &mut Vec<u8>, rng: &mut Rng) {
    for _ in 0..=rng.below(4) {
        if input.is_empty() {
            input.push(rng.next() as u8);
            continue;
        }
        let at = rng.below(input.len());
        match rng.below(4) {
            0 => input[at] ^= 1 << rng.below(8),
            1 => input[at] = rng.next() as u8,
            2 => input.truncate(at),
            _ => input.insert(at, rng.next() as u8),
        }
    }
}

#[test]
fn random_stream_bytes_never_panic() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for _ in 0..ITERATIONS {
        let input = rng.bytes_below(64);
        drive_stream(&mut Session::new(Hello::this_release()), &input, &mut rng);
        drive_stream(&mut agreed_session(), &input, &mut rng);
    }
}

#[test]
fn mutated_frames_never_panic() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    for _ in 0..ITERATIONS {
        let mut input = valid_frames(&mut rng);
        mutate(&mut input, &mut rng);
        drive_stream(&mut Session::new(Hello::this_release()), &input, &mut rng);
        drive_stream(&mut agreed_session(), &input, &mut rng);
    }
}

#[test]
fn mutated_manifests_never_panic() {
    let mut rng = Rng(0x3c6e_f372_fe94_f82b);
    for _ in 0..ITERATIONS {
        let mut input = Vec::new();
        files::encode_manifest(manifest_entries(&mut rng), &mut input).unwrap();
        mutate(&mut input, &mut rng);
        parse_manifest(&input);
        let random = rng.bytes_below(64);
        parse_manifest(&random);
    }
}

#[test]
fn random_datagrams_never_panic() {
    let mut rng = Rng(0xd1b5_4a32_d192_ed03);
    let (fresh, agreed) = (Session::new(Hello::this_release()), agreed_session());
    for _ in 0..ITERATIONS {
        let len = if rng.below(100) == 0 {
            rng.below(MAX_DATAGRAM_LEN + 100)
        } else {
            rng.below(16)
        };
        let input = rng.bytes(len);
        drive_datagram(&fresh, &agreed, &input);
    }
}

#[test]
fn mutated_motion_datagrams_never_panic() {
    let mut rng = Rng(0x6a09_e667_f3bc_c908);
    let (fresh, agreed) = (Session::new(Hello::this_release()), agreed_session());
    for _ in 0..ITERATIONS {
        let mut input = Vec::new();
        InputMessage::Motion(mark(&mut rng))
            .encode(&mut input)
            .unwrap();
        if rng.below(2) == 0 {
            mutate(&mut input, &mut rng);
        }
        drive_datagram(&fresh, &agreed, &input);
    }
}
