//! Decoding is total: arbitrary input never panics, and every decoded
//! message re-encodes to the same bytes. Deterministic, so it runs in
//! `cargo test`; `tools/fuzz` covers the same driver with coverage guidance.

use ipc::IPC_VERSION;
use ipc::clipboard::{Kind, NotShared, PNG_SIGNATURE, RunAssembler};
use ipc::files::{FileRun, Window};
use ipc::frame::{self, Decoded, HEADER_LEN};
use ipc::message::{
    ClipboardToAgent, Hello, Role, ToAgent, ToAgentAny, ToService, check_agent_role,
};
use ipc::ui::{FocusOf, Member, Pairing, Status, UiCommand, UiEvent};
use model::{
    Button, CaptureMode, InputAction, InputEvent, Key, Millis, Monitor, MonitorId, Origin, Point,
    Rect, Screen,
};

const ITERATIONS: usize = 200_000;

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
}

/// Feeds a decoded clipboard message to the run at the end that receives it;
/// errors are fine, panics are not.
/// One end's clipboard run, file run and window.
#[derive(Default)]
struct Runs {
    clipboard: RunAssembler,
    file: FileRun,
    window: Window,
}

fn run_service(runs: &mut Runs, m: &ToService) {
    let _ = match m {
        ToService::ClipboardText(len) => runs.clipboard.start(0x16, Kind::Text, *len).map(|_| ()),
        ToService::ClipboardImage(len) => runs.clipboard.start(0x1a, Kind::Image, *len).map(|_| ()),
        ToService::ClipboardFiles(len) => runs.clipboard.start(0x40, Kind::Files, *len).map(|_| ()),
        ToService::ClipboardChunk(bytes) => runs.clipboard.chunk(0x17, bytes).map(|_| ()),
        ToService::ClipboardNotShared(_) => runs.clipboard.not_in_run(0x18),
        ToService::FileContent { index, len } => runs.file.start(0x42, *index, *len).map(|_| ()),
        ToService::FileChunk(bytes) => runs
            .window
            .receive(0x43, bytes.len())
            .and_then(|()| runs.file.chunk(0x43, bytes.len()).map(|_| ())),
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

fn run_agent(runs: &mut Runs, m: &ClipboardToAgent) {
    let _ = match m {
        ClipboardToAgent::Set(len) => runs.clipboard.start(0x27, Kind::Text, *len).map(|_| ()),
        ClipboardToAgent::SetImage(len) => {
            runs.clipboard.start(0x29, Kind::Image, *len).map(|_| ())
        }
        ClipboardToAgent::SetFiles(len) => {
            runs.clipboard.start(0x48, Kind::Files, *len).map(|_| ())
        }
        ClipboardToAgent::Chunk(bytes) => runs.clipboard.chunk(0x28, bytes).map(|_| ()),
        ClipboardToAgent::SetFileContent { index, len } => {
            runs.file.start(0x4a, *index, *len).map(|_| ())
        }
        ClipboardToAgent::SetFileChunk(bytes) => runs
            .window
            .receive(0x4b, bytes.len())
            .and_then(|()| runs.file.chunk(0x4b, bytes.len()).map(|_| ())),
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

/// Decodes every frame in `input` as both ends of a link at this release's
/// version would; decoded messages re-encode to the same payload. A type
/// version 2 added never decodes at version 1.
fn drive(input: &[u8]) {
    const V: u16 = IPC_VERSION;
    let mut rest = input;
    let (mut service_runs, mut agent_runs) = (Runs::default(), Runs::default());
    loop {
        match frame::decode(rest) {
            Ok(Decoded::NeedMore) | Err(_) => return,
            Ok(Decoded::Frame { frame, consumed }) => {
                assert!(consumed >= HEADER_LEN && consumed <= rest.len());
                let _ = Hello::decode(frame.payload);
                if ToService::decode(frame.ty, frame.payload, 1).is_ok()
                    || ToAgentAny::decode(frame.ty, frame.payload, 1).is_ok()
                {
                    assert!(
                        frame.ty != 0x1a && frame.ty != 0x29,
                        "image type decoded at version 1"
                    );
                }
                if ToService::decode(frame.ty, frame.payload, 2).is_ok()
                    || ToAgentAny::decode(frame.ty, frame.payload, 2).is_ok()
                {
                    assert!(
                        !(0x40..=0x4f).contains(&frame.ty),
                        "file type decoded at version 2"
                    );
                }
                if let Ok(m) = ToService::decode(frame.ty, frame.payload, V) {
                    assert_eq!(m.payload().unwrap(), frame.payload);
                    for role in [Role::SessionAgent, Role::WinlogonAgent] {
                        let _ = check_agent_role(frame.ty, role);
                    }
                    run_service(&mut service_runs, &m);
                }
                if let Ok(m) = ToAgent::decode(frame.ty, frame.payload, V) {
                    assert_eq!(m.payload(), frame.payload);
                }
                if let Ok(ToAgentAny::Clipboard(m)) = ToAgentAny::decode(frame.ty, frame.payload, V)
                {
                    assert_eq!(m.payload().unwrap(), frame.payload);
                    run_agent(&mut agent_runs, &m);
                }
                if let Ok(m) = UiCommand::decode(frame.ty, frame.payload, V) {
                    assert_eq!(m.payload().unwrap(), frame.payload);
                }
                if let Ok(m) = UiEvent::decode(frame.ty, frame.payload, V) {
                    assert_eq!(m.payload().unwrap(), frame.payload);
                }
                rest = &rest[consumed..];
            }
        }
    }
}

fn event(rng: &mut Rng) -> InputEvent {
    let v = |rng: &mut Rng| rng.next() as i32;
    match rng.below(4) {
        0 => InputEvent::Key {
            key: Key(rng.next() as u16),
            down: rng.below(2) == 0,
        },
        1 => InputEvent::Button {
            button: [
                Button::Left,
                Button::Right,
                Button::Middle,
                Button::X1,
                Button::X2,
            ][rng.below(5)],
            down: rng.below(2) == 0,
        },
        2 => InputEvent::Wheel {
            dx: v(rng),
            dy: v(rng),
        },
        _ => InputEvent::Motion {
            dx: v(rng),
            dy: v(rng),
            cursor: Point::new(v(rng), v(rng)),
        },
    }
}

fn valid_frames(rng: &mut Rng) -> Vec<u8> {
    let mut out = Vec::new();
    Hello::this_release(Role::SessionAgent)
        .encode_frame(&mut out)
        .unwrap();
    for _ in 0..rng.below(4) {
        match rng.below(16) {
            14 => {
                let n = rng.below(30);
                let manifest = rng.bytes(n);
                ToService::ClipboardFiles(manifest.len() as u32)
                    .encode_frame(&mut out)
                    .unwrap();
                for c in manifest.chunks(1 + rng.below(6)) {
                    ToService::ClipboardChunk(c.to_vec())
                        .encode_frame(&mut out)
                        .unwrap();
                }
                let n = rng.below(30);
                let body = rng.bytes(n);
                let index = rng.next() as u32;
                ToService::FileContent {
                    index,
                    len: body.len() as u64,
                }
                .encode_frame(&mut out)
                .unwrap();
                for c in body.chunks(1 + rng.below(6)) {
                    ToService::FileChunk(c.to_vec())
                        .encode_frame(&mut out)
                        .unwrap();
                }
                for m in [
                    ToService::FileWanted {
                        set: rng.next() as u32,
                        index,
                    },
                    ToService::FileAbandoned { index },
                    ToService::FileUnwanted { index },
                    ToService::FileConsumed(rng.next() as u32),
                ] {
                    m.encode_frame(&mut out).unwrap();
                }
            }
            15 => {
                let n = rng.below(30);
                let manifest = rng.bytes(n);
                ClipboardToAgent::SetFiles(manifest.len() as u32)
                    .encode_frame(&mut out)
                    .unwrap();
                for c in manifest.chunks(1 + rng.below(6)) {
                    ClipboardToAgent::Chunk(c.to_vec())
                        .encode_frame(&mut out)
                        .unwrap();
                }
                let n = rng.below(30);
                let body = rng.bytes(n);
                let index = rng.next() as u32;
                ClipboardToAgent::SetFileContent {
                    index,
                    len: body.len() as u64,
                }
                .encode_frame(&mut out)
                .unwrap();
                for c in body.chunks(1 + rng.below(6)) {
                    ClipboardToAgent::SetFileChunk(c.to_vec())
                        .encode_frame(&mut out)
                        .unwrap();
                }
                for m in [
                    ClipboardToAgent::SendFile {
                        copy: rng.next() as u32,
                        index,
                    },
                    ClipboardToAgent::AbandonFile { index },
                    ClipboardToAgent::StopFile { index },
                    ClipboardToAgent::FileAcked(rng.next() as u32),
                ] {
                    m.encode_frame(&mut out).unwrap();
                }
            }
            12 => {
                let mut png = PNG_SIGNATURE.to_vec();
                let extra = rng.below(20);
                png.extend(rng.bytes(extra));
                ToService::ClipboardImage(png.len() as u32)
                    .encode_frame(&mut out)
                    .unwrap();
                for c in png.chunks(1 + rng.below(6)) {
                    ToService::ClipboardChunk(c.to_vec())
                        .encode_frame(&mut out)
                        .unwrap();
                }
            }
            13 => {
                let mut png = PNG_SIGNATURE.to_vec();
                let extra = rng.below(20);
                png.extend(rng.bytes(extra));
                ClipboardToAgent::SetImage(png.len() as u32)
                    .encode_frame(&mut out)
                    .unwrap();
                for c in png.chunks(1 + rng.below(6)) {
                    ClipboardToAgent::Chunk(c.to_vec())
                        .encode_frame(&mut out)
                        .unwrap();
                }
            }
            11 => ToService::InputDesktop(rng.below(2) == 0)
                .encode_frame(&mut out)
                .unwrap(),
            8 => {
                let len = rng.below(20);
                let text = rng.bytes(len);
                ToService::ClipboardText(text.len() as u32)
                    .encode_frame(&mut out)
                    .unwrap();
                for c in text.chunks(1 + rng.below(6)) {
                    ToService::ClipboardChunk(c.to_vec())
                        .encode_frame(&mut out)
                        .unwrap();
                }
            }
            9 => {
                let len = rng.below(20);
                let text = rng.bytes(len);
                ClipboardToAgent::Set(text.len() as u32)
                    .encode_frame(&mut out)
                    .unwrap();
                for c in text.chunks(1 + rng.below(6)) {
                    ClipboardToAgent::Chunk(c.to_vec())
                        .encode_frame(&mut out)
                        .unwrap();
                }
            }
            10 => ToService::ClipboardNotShared(
                [NotShared::Private, NotShared::NoText, NotShared::TooLarge][rng.below(3)],
            )
            .encode_frame(&mut out)
            .unwrap(),
            5 => ui_command(rng).encode_frame(&mut out).unwrap(),
            6 => UiEvent::Status(status(rng)).encode_frame(&mut out).unwrap(),
            7 => UiEvent::Paired {
                key: [rng.next() as u8; 32],
                name: "B".repeat(1 + rng.below(63)),
            }
            .encode_frame(&mut out)
            .unwrap(),
            0 => ToService::Input {
                at: Millis(rng.next()),
                input: event(rng),
                origin: if rng.below(2) == 0 {
                    Origin::Physical
                } else {
                    Origin::Injected
                },
            }
            .encode_frame(&mut out)
            .unwrap(),
            1 => {
                let n = rng.below(3);
                let monitors = (0..n)
                    .map(|i| Monitor {
                        id: MonitorId(format!("m{i}")),
                        rect: Rect::new(i as i32 * 100, 0, 100, 100),
                        dpi: 96,
                    })
                    .collect();
                ToService::Screen(Screen::new(monitors))
                    .encode_frame(&mut out)
                    .unwrap();
            }
            2 => ToService::Pong {
                service: Millis(rng.next()),
                agent: Millis(rng.next()),
            }
            .encode_frame(&mut out)
            .unwrap(),
            3 => ToAgent::SetCapture(
                [
                    CaptureMode::PassAll,
                    CaptureMode::WithholdAll,
                    CaptureMode::WithholdMouse,
                ][rng.below(3)],
            )
            .encode_frame(&mut out)
            .unwrap(),
            _ => ToAgent::Inject(InputAction::Motion {
                dx: rng.next() as i32,
                dy: rng.next() as i32,
            })
            .encode_frame(&mut out)
            .unwrap(),
        }
    }
    out
}

fn ui_command(rng: &mut Rng) -> UiCommand {
    match rng.below(6) {
        0 => UiCommand::OpenPairing,
        1 => UiCommand::ClosePairing,
        2 => UiCommand::Join("12-3456789".into()),
        3 => UiCommand::RemovePeer([rng.next() as u8; 32]),
        4 => UiCommand::Undo,
        _ => UiCommand::ForgetLayout,
    }
}

fn status(rng: &mut Rng) -> Status {
    let key = [rng.next() as u8; 32];
    Status {
        key,
        name: "A".into(),
        agent_linked: rng.below(2) == 0,
        pairing: if rng.below(2) == 0 {
            Pairing::Closed
        } else {
            Pairing::Open {
                code: "12-3456789".into(),
                left_ms: rng.next() as u32,
            }
        },
        focus: [FocusOf::Local, FocusOf::On(key), FocusOf::ControlledBy(key)][rng.below(3)],
        members: (0..rng.below(4))
            .map(|i| Member {
                key: [i as u8; 32],
                name: format!("M{i}"),
                up: rng.below(2) == 0,
            })
            .collect(),
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
fn random_bytes_never_panic() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for _ in 0..ITERATIONS {
        let len = rng.below(64);
        drive(&rng.bytes(len));
    }
}

#[test]
fn mutated_frames_never_panic() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    for _ in 0..ITERATIONS {
        let mut input = valid_frames(&mut rng);
        if rng.below(4) != 0 {
            mutate(&mut input, &mut rng);
        }
        drive(&input);
    }
}
