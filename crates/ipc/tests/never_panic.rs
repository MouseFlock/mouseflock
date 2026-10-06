//! Decoding is total: arbitrary input never panics, and every decoded
//! message re-encodes to the same bytes. Deterministic, so it runs in
//! `cargo test`; `tools/fuzz` covers the same driver with coverage guidance.

use ipc::frame::{self, Decoded, HEADER_LEN};
use ipc::message::{Hello, Role, ToAgent, ToService};
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

/// Decodes every frame in `input` as both ends would; decoded messages
/// re-encode to the same payload.
fn drive(input: &[u8]) {
    let mut rest = input;
    loop {
        match frame::decode(rest) {
            Ok(Decoded::NeedMore) | Err(_) => return,
            Ok(Decoded::Frame { frame, consumed }) => {
                assert!(consumed >= HEADER_LEN && consumed <= rest.len());
                let _ = Hello::decode(frame.payload);
                if let Ok(m) = ToService::decode(frame.ty, frame.payload, 1) {
                    assert_eq!(m.payload().unwrap(), frame.payload);
                }
                if let Ok(m) = ToAgent::decode(frame.ty, frame.payload, 1) {
                    assert_eq!(m.payload(), frame.payload);
                }
                if let Ok(m) = UiCommand::decode(frame.ty, frame.payload, 1) {
                    assert_eq!(m.payload().unwrap(), frame.payload);
                }
                if let Ok(m) = UiEvent::decode(frame.ty, frame.payload, 1) {
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
        match rng.below(8) {
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
