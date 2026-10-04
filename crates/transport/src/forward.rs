//! Mapping between the engine's `model::PeerMessage` and `proto::input`
//! (design D2). `proto` does not depend on `model`, so the mapping lives
//! here.

use model::{Button, EdgeFraction, InputAction, Key, PeerId, PeerMessage, Side};
use proto::input::{
    self, ButtonInput, EdgeExit, FocusEnter, InputMessage, KeyInput, Mark, WheelInput,
};

/// How to send one engine message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outbound {
    /// Movement for the motion sender.
    Motion(i32, i32),
    /// A message with no motion mark.
    Plain(InputMessage),
    /// A key, button or wheel message, to be completed with the mark.
    Marked(Marked),
}

/// A key, button or wheel message waiting for its mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Marked {
    Key(Key, bool),
    Button(Button, bool),
    Wheel(i32, i32),
}

impl Marked {
    pub(crate) fn with(self, mark: Mark) -> InputMessage {
        match self {
            Self::Key(key, down) => InputMessage::Key(KeyInput {
                mark,
                usage: key.0,
                down,
            }),
            Self::Button(button, down) => InputMessage::Button(ButtonInput {
                mark,
                button: wire_button(button),
                down,
            }),
            Self::Wheel(dx, dy) => InputMessage::Wheel(WheelInput { mark, dx, dy }),
        }
    }
}

/// What a received engine message or datagram is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Inbound {
    /// A message with no motion mark.
    Plain(PeerMessage),
    /// A key, button or wheel message: apply the mark first.
    Marked(Mark, PeerMessage),
    /// A `Motion` datagram.
    Motion(Mark),
}

pub(crate) fn outbound(msg: PeerMessage) -> Outbound {
    let plain = Outbound::Plain;
    match msg {
        PeerMessage::FocusEnter {
            side,
            fraction,
            from,
        } => plain(InputMessage::FocusEnter(FocusEnter {
            side: wire_side(side),
            fraction: fraction.0,
            from: from.0,
        })),
        PeerMessage::FocusRefused => plain(InputMessage::FocusRefused),
        PeerMessage::EdgeExit {
            target,
            side,
            fraction,
        } => plain(InputMessage::EdgeExit(EdgeExit {
            target: target.0,
            side: wire_side(side),
            fraction: fraction.0,
        })),
        PeerMessage::TakenBack => plain(InputMessage::TakenBack),
        PeerMessage::FocusWithdrawn => plain(InputMessage::FocusWithdrawn),
        PeerMessage::ForgetSide { side } => plain(InputMessage::ForgetSide(wire_side(side))),
        PeerMessage::SecureAttention => plain(InputMessage::SecureAttention),
        PeerMessage::Input(action) => match action {
            InputAction::Motion { dx, dy } => Outbound::Motion(dx, dy),
            InputAction::Key { key, down } => Outbound::Marked(Marked::Key(key, down)),
            InputAction::Button { button, down } => Outbound::Marked(Marked::Button(button, down)),
            InputAction::Wheel { dx, dy } => Outbound::Marked(Marked::Wheel(dx, dy)),
        },
    }
}

pub(crate) fn inbound(msg: InputMessage) -> Inbound {
    let plain = Inbound::Plain;
    let input = |mark, action| Inbound::Marked(mark, PeerMessage::Input(action));
    match msg {
        InputMessage::FocusEnter(m) => plain(PeerMessage::FocusEnter {
            side: model_side(m.side),
            fraction: EdgeFraction(m.fraction),
            from: PeerId(m.from),
        }),
        InputMessage::FocusRefused => plain(PeerMessage::FocusRefused),
        InputMessage::EdgeExit(m) => plain(PeerMessage::EdgeExit {
            target: PeerId(m.target),
            side: model_side(m.side),
            fraction: EdgeFraction(m.fraction),
        }),
        InputMessage::TakenBack => plain(PeerMessage::TakenBack),
        InputMessage::FocusWithdrawn => plain(PeerMessage::FocusWithdrawn),
        InputMessage::ForgetSide(side) => plain(PeerMessage::ForgetSide {
            side: model_side(side),
        }),
        InputMessage::SecureAttention => plain(PeerMessage::SecureAttention),
        InputMessage::Key(m) => input(
            m.mark,
            InputAction::Key {
                key: Key(m.usage),
                down: m.down,
            },
        ),
        InputMessage::Button(m) => input(
            m.mark,
            InputAction::Button {
                button: model_button(m.button),
                down: m.down,
            },
        ),
        InputMessage::Wheel(m) => input(m.mark, InputAction::Wheel { dx: m.dx, dy: m.dy }),
        InputMessage::Motion(mark) => Inbound::Motion(mark),
    }
}

const fn wire_side(side: Side) -> input::Side {
    match side {
        Side::Left => input::Side::Left,
        Side::Right => input::Side::Right,
        Side::Top => input::Side::Top,
        Side::Bottom => input::Side::Bottom,
    }
}

const fn model_side(side: input::Side) -> Side {
    match side {
        input::Side::Left => Side::Left,
        input::Side::Right => Side::Right,
        input::Side::Top => Side::Top,
        input::Side::Bottom => Side::Bottom,
    }
}

const fn wire_button(button: Button) -> input::Button {
    match button {
        Button::Left => input::Button::Left,
        Button::Right => input::Button::Right,
        Button::Middle => input::Button::Middle,
        Button::X1 => input::Button::X1,
        Button::X2 => input::Button::X2,
    }
}

const fn model_button(button: input::Button) -> Button {
    match button {
        input::Button::Left => Button::Left,
        input::Button::Right => Button::Right,
        input::Button::Middle => Button::Middle,
        input::Button::X1 => Button::X1,
        input::Button::X2 => Button::X2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every engine message except motion, with each side and button.
    fn every_message() -> Vec<PeerMessage> {
        let mut all = vec![
            PeerMessage::FocusRefused,
            PeerMessage::TakenBack,
            PeerMessage::FocusWithdrawn,
            PeerMessage::SecureAttention,
            PeerMessage::Input(InputAction::Key {
                key: Key(0xe4),
                down: true,
            }),
            PeerMessage::Input(InputAction::Wheel { dx: -120, dy: 240 }),
        ];
        for side in [Side::Left, Side::Right, Side::Top, Side::Bottom] {
            all.push(PeerMessage::FocusEnter {
                side,
                fraction: EdgeFraction(1234),
                from: PeerId([1; 32]),
            });
            all.push(PeerMessage::EdgeExit {
                target: PeerId([2; 32]),
                side,
                fraction: EdgeFraction(u16::MAX),
            });
            all.push(PeerMessage::ForgetSide { side });
        }
        for button in [
            Button::Left,
            Button::Right,
            Button::Middle,
            Button::X1,
            Button::X2,
        ] {
            all.push(PeerMessage::Input(InputAction::Button {
                button,
                down: false,
            }));
        }
        all
    }

    #[test]
    fn every_message_round_trips_through_its_frame() {
        let mark = Mark {
            seq: 3,
            dx: 4,
            dy: 5,
        };
        for msg in every_message() {
            let wire = match outbound(msg) {
                Outbound::Plain(m) => m,
                Outbound::Marked(m) => m.with(mark),
                Outbound::Motion(..) => panic!("{msg:?} is not motion"),
            };
            let decoded = InputMessage::decode(wire.ty(), &wire.payload()).unwrap();
            let back = match inbound(decoded) {
                Inbound::Plain(m) => m,
                Inbound::Marked(got, m) => {
                    assert_eq!(got, mark);
                    m
                }
                Inbound::Motion(_) => panic!("{msg:?} came back as motion"),
            };
            assert_eq!(back, msg);
        }
    }

    #[test]
    fn motion_goes_to_the_sender() {
        assert_eq!(
            outbound(PeerMessage::Input(InputAction::Motion { dx: 3, dy: -2 })),
            Outbound::Motion(3, -2)
        );
        let mark = Mark {
            seq: 9,
            dx: 1,
            dy: 1,
        };
        assert_eq!(inbound(InputMessage::Motion(mark)), Inbound::Motion(mark));
    }
}
