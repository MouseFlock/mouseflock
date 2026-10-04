//! What one machine's engine tells another's: focus messages, forwarded keys,
//! buttons and wheel on the control stream, and motion as running totals in
//! datagrams.
//!
//! Every payload has a fixed length. Keys, buttons and wheel carry a
//! [`Mark`]: the sender's motion totals at that moment, so the receiver can
//! apply movement it has not seen before acting on them. This module only
//! checks layouts; the motion rules live in `transport`.

use crate::codec::fixed_payload;
use crate::{EncodeError, ProtocolError, datagram, frame, registry};

/// Bytes in a peer identity.
pub const KEY_LEN: usize = 32;

/// Bytes in a [`Mark`], and in a `Motion` payload.
pub const MARK_LEN: usize = 12;

const FOCUS_ENTER_LEN: usize = 1 + 2 + KEY_LEN;
const EDGE_EXIT_LEN: usize = KEY_LEN + 1 + 2;
const KEY_INPUT_LEN: usize = MARK_LEN + 2 + 1;
const BUTTON_INPUT_LEN: usize = MARK_LEN + 1 + 1;
const WHEEL_INPUT_LEN: usize = MARK_LEN + 4 + 4;

/// A side of a machine's screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    const fn code(self) -> u8 {
        match self {
            Self::Left => 0,
            Self::Right => 1,
            Self::Top => 2,
            Self::Bottom => 3,
        }
    }

    const fn from_code(ty: u16, value: u8) -> Result<Self, ProtocolError> {
        match value {
            0 => Ok(Self::Left),
            1 => Ok(Self::Right),
            2 => Ok(Self::Top),
            3 => Ok(Self::Bottom),
            _ => Err(ProtocolError::InvalidCode { ty, value }),
        }
    }
}

/// A mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

impl Button {
    const fn code(self) -> u8 {
        match self {
            Self::Left => 0,
            Self::Right => 1,
            Self::Middle => 2,
            Self::X1 => 3,
            Self::X2 => 4,
        }
    }

    const fn from_code(ty: u16, value: u8) -> Result<Self, ProtocolError> {
        match value {
            0 => Ok(Self::Left),
            1 => Ok(Self::Right),
            2 => Ok(Self::Middle),
            3 => Ok(Self::X1),
            4 => Ok(Self::X2),
            _ => Err(ProtocolError::InvalidCode { ty, value }),
        }
    }
}

const fn down_code(down: bool) -> u8 {
    if down { 1 } else { 0 }
}

const fn down_from_code(ty: u16, value: u8) -> Result<bool, ProtocolError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(ProtocolError::InvalidCode { ty, value }),
    }
}

/// A sequence number and the running totals of forwarded motion. The
/// payload of `Motion`, and the start of every key, button and wheel
/// message. Totals and sequence numbers wrap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mark {
    pub seq: u32,
    pub dx: i32,
    pub dy: i32,
}

impl Mark {
    fn put(self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.extend_from_slice(&self.dx.to_le_bytes());
        out.extend_from_slice(&self.dy.to_le_bytes());
    }

    fn take(f: &mut Fields<'_>) -> Result<Self, ProtocolError> {
        Ok(Self {
            seq: u32::from_le_bytes(f.array()?),
            dx: i32::from_le_bytes(f.array()?),
            dy: i32::from_le_bytes(f.array()?),
        })
    }

    /// `Motion` as a datagram.
    pub fn encode_datagram(&self, out: &mut Vec<u8>) -> Result<(), EncodeError> {
        let mut payload = Vec::with_capacity(MARK_LEN);
        self.put(&mut payload);
        datagram::encode(registry::MOTION, &payload, out)
    }
}

/// `FocusEnter`: focus moves to the receiver through its `side` at
/// `fraction`; `from` is the machine whose edge the cursor left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FocusEnter {
    pub side: Side,
    pub fraction: u16,
    pub from: [u8; KEY_LEN],
}

/// `EdgeExit`: the cursor left the sender's `side` at `fraction` toward
/// `target`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeExit {
    pub target: [u8; KEY_LEN],
    pub side: Side,
    pub fraction: u16,
}

/// `KeyInput`: a key, as a USB HID keyboard-page usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyInput {
    pub mark: Mark,
    pub usage: u16,
    pub down: bool,
}

/// `ButtonInput`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ButtonInput {
    pub mark: Mark,
    pub button: Button,
    pub down: bool,
}

/// `WheelInput`, in 1/120 of a notch: positive `dy` away from the user,
/// positive `dx` to the right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WheelInput {
    pub mark: Mark,
    pub dx: i32,
    pub dy: i32,
}

/// Any engine message, or `Motion`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMessage {
    FocusEnter(FocusEnter),
    FocusRefused,
    EdgeExit(EdgeExit),
    TakenBack,
    FocusWithdrawn,
    ForgetSide(Side),
    Key(KeyInput),
    Button(ButtonInput),
    Wheel(WheelInput),
    SecureAttention,
    /// Datagram only.
    Motion(Mark),
}

impl InputMessage {
    /// Decodes a message of type `ty`. Types this module does not define are
    /// [`ProtocolError::UnknownType`].
    pub fn decode(ty: u16, payload: &[u8]) -> Result<Self, ProtocolError> {
        Ok(match ty {
            registry::FOCUS_ENTER => {
                let p = fixed_payload::<FOCUS_ENTER_LEN>(ty, payload)?;
                let mut f = Fields::new(ty, &p);
                let [side] = f.array()?;
                Self::FocusEnter(FocusEnter {
                    side: Side::from_code(ty, side)?,
                    fraction: u16::from_le_bytes(f.array()?),
                    from: f.array()?,
                })
            }
            registry::FOCUS_REFUSED => empty(ty, payload, Self::FocusRefused)?,
            registry::EDGE_EXIT => {
                let p = fixed_payload::<EDGE_EXIT_LEN>(ty, payload)?;
                let mut f = Fields::new(ty, &p);
                let target = f.array()?;
                let [side] = f.array()?;
                Self::EdgeExit(EdgeExit {
                    target,
                    side: Side::from_code(ty, side)?,
                    fraction: u16::from_le_bytes(f.array()?),
                })
            }
            registry::TAKEN_BACK => empty(ty, payload, Self::TakenBack)?,
            registry::FOCUS_WITHDRAWN => empty(ty, payload, Self::FocusWithdrawn)?,
            registry::FORGET_SIDE => {
                let [side] = fixed_payload::<1>(ty, payload)?;
                Self::ForgetSide(Side::from_code(ty, side)?)
            }
            registry::KEY_INPUT => {
                let p = fixed_payload::<KEY_INPUT_LEN>(ty, payload)?;
                let mut f = Fields::new(ty, &p);
                let mark = Mark::take(&mut f)?;
                let usage = u16::from_le_bytes(f.array()?);
                let [down] = f.array()?;
                Self::Key(KeyInput {
                    mark,
                    usage,
                    down: down_from_code(ty, down)?,
                })
            }
            registry::BUTTON_INPUT => {
                let p = fixed_payload::<BUTTON_INPUT_LEN>(ty, payload)?;
                let mut f = Fields::new(ty, &p);
                let mark = Mark::take(&mut f)?;
                let [button, down] = f.array()?;
                Self::Button(ButtonInput {
                    mark,
                    button: Button::from_code(ty, button)?,
                    down: down_from_code(ty, down)?,
                })
            }
            registry::WHEEL_INPUT => {
                let p = fixed_payload::<WHEEL_INPUT_LEN>(ty, payload)?;
                let mut f = Fields::new(ty, &p);
                Self::Wheel(WheelInput {
                    mark: Mark::take(&mut f)?,
                    dx: i32::from_le_bytes(f.array()?),
                    dy: i32::from_le_bytes(f.array()?),
                })
            }
            registry::SECURE_ATTENTION => empty(ty, payload, Self::SecureAttention)?,
            registry::MOTION => {
                let p = fixed_payload::<MARK_LEN>(ty, payload)?;
                Self::Motion(Mark::take(&mut Fields::new(ty, &p))?)
            }
            _ => return Err(ProtocolError::UnknownType { ty }),
        })
    }

    /// The message type.
    pub const fn ty(&self) -> u16 {
        match self {
            Self::FocusEnter(_) => registry::FOCUS_ENTER,
            Self::FocusRefused => registry::FOCUS_REFUSED,
            Self::EdgeExit(_) => registry::EDGE_EXIT,
            Self::TakenBack => registry::TAKEN_BACK,
            Self::FocusWithdrawn => registry::FOCUS_WITHDRAWN,
            Self::ForgetSide(_) => registry::FORGET_SIDE,
            Self::Key(_) => registry::KEY_INPUT,
            Self::Button(_) => registry::BUTTON_INPUT,
            Self::Wheel(_) => registry::WHEEL_INPUT,
            Self::SecureAttention => registry::SECURE_ATTENTION,
            Self::Motion(_) => registry::MOTION,
        }
    }

    /// The payload bytes.
    pub fn payload(&self) -> Vec<u8> {
        let mut p = Vec::new();
        match self {
            Self::FocusEnter(m) => {
                p.push(m.side.code());
                p.extend_from_slice(&m.fraction.to_le_bytes());
                p.extend_from_slice(&m.from);
            }
            Self::EdgeExit(m) => {
                p.extend_from_slice(&m.target);
                p.push(m.side.code());
                p.extend_from_slice(&m.fraction.to_le_bytes());
            }
            Self::ForgetSide(side) => p.push(side.code()),
            Self::Key(m) => {
                m.mark.put(&mut p);
                p.extend_from_slice(&m.usage.to_le_bytes());
                p.push(down_code(m.down));
            }
            Self::Button(m) => {
                m.mark.put(&mut p);
                p.push(m.button.code());
                p.push(down_code(m.down));
            }
            Self::Wheel(m) => {
                m.mark.put(&mut p);
                p.extend_from_slice(&m.dx.to_le_bytes());
                p.extend_from_slice(&m.dy.to_le_bytes());
            }
            Self::Motion(m) => m.put(&mut p),
            Self::FocusRefused | Self::TakenBack | Self::FocusWithdrawn | Self::SecureAttention => {
            }
        }
        p
    }

    /// Appends this message as a stream frame, or for `Motion` as a
    /// datagram, to `out`.
    pub fn encode(&self, out: &mut Vec<u8>) -> Result<(), EncodeError> {
        match self {
            Self::Motion(m) => m.encode_datagram(out),
            _ => frame::encode(self.ty(), &self.payload(), out),
        }
    }
}

/// An empty-payload message, or a payload-length error.
fn empty(ty: u16, payload: &[u8], message: InputMessage) -> Result<InputMessage, ProtocolError> {
    fixed_payload::<0>(ty, payload).map(|_| message)
}

/// Reads fields front to back from a payload whose length is already
/// checked; a shortfall still reports a length error rather than panicking.
struct Fields<'a> {
    ty: u16,
    rest: &'a [u8],
    len: usize,
}

impl<'a> Fields<'a> {
    const fn new(ty: u16, payload: &'a [u8]) -> Self {
        Self {
            ty,
            rest: payload,
            len: payload.len(),
        }
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], ProtocolError> {
        let (head, rest) =
            self.rest
                .split_first_chunk::<N>()
                .ok_or(ProtocolError::PayloadLength {
                    ty: self.ty,
                    expected: N,
                    actual: self.len,
                })?;
        self.rest = rest;
        Ok(*head)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MARK: Mark = Mark {
        seq: 0x0102_0304,
        dx: -5,
        dy: 70_000,
    };

    fn every_message() -> Vec<InputMessage> {
        vec![
            InputMessage::FocusEnter(FocusEnter {
                side: Side::Left,
                fraction: 0x8000,
                from: [7; KEY_LEN],
            }),
            InputMessage::FocusRefused,
            InputMessage::EdgeExit(EdgeExit {
                target: [9; KEY_LEN],
                side: Side::Bottom,
                fraction: u16::MAX,
            }),
            InputMessage::TakenBack,
            InputMessage::FocusWithdrawn,
            InputMessage::ForgetSide(Side::Top),
            InputMessage::Key(KeyInput {
                mark: MARK,
                usage: 0x00e4,
                down: true,
            }),
            InputMessage::Button(ButtonInput {
                mark: MARK,
                button: Button::X2,
                down: false,
            }),
            InputMessage::Wheel(WheelInput {
                mark: MARK,
                dx: 120,
                dy: -240,
            }),
            InputMessage::SecureAttention,
            InputMessage::Motion(MARK),
        ]
    }

    #[test]
    fn every_message_round_trips() {
        for m in every_message() {
            assert_eq!(InputMessage::decode(m.ty(), &m.payload()), Ok(m), "{m:?}");
        }
    }

    #[test]
    fn payload_lengths_match_the_layouts() {
        let lengths: Vec<usize> = every_message().iter().map(|m| m.payload().len()).collect();
        assert_eq!(lengths, [35, 0, 35, 0, 0, 1, 15, 14, 20, 0, 12]);
    }

    #[test]
    fn one_byte_short_or_long_is_rejected() {
        for m in every_message() {
            let p = m.payload();
            let mut long = p.clone();
            long.push(0);
            assert!(
                matches!(
                    InputMessage::decode(m.ty(), &long),
                    Err(ProtocolError::PayloadLength { .. })
                ),
                "{m:?} long"
            );
            if let Some((_, short)) = p.split_last() {
                assert!(
                    matches!(
                        InputMessage::decode(m.ty(), short),
                        Err(ProtocolError::PayloadLength { .. })
                    ),
                    "{m:?} short"
                );
            }
        }
    }

    #[test]
    fn undefined_codes_are_rejected() {
        let ty = registry::FORGET_SIDE;
        for v in 4..=u8::MAX {
            assert_eq!(
                InputMessage::decode(ty, &[v]),
                Err(ProtocolError::InvalidCode { ty, value: v })
            );
        }

        let mut enter = InputMessage::FocusEnter(FocusEnter {
            side: Side::Left,
            fraction: 0,
            from: [0; KEY_LEN],
        })
        .payload();
        enter[0] = 4;
        assert!(InputMessage::decode(registry::FOCUS_ENTER, &enter).is_err());

        let mut exit = InputMessage::EdgeExit(EdgeExit {
            target: [0; KEY_LEN],
            side: Side::Left,
            fraction: 0,
        })
        .payload();
        exit[KEY_LEN] = 9;
        assert!(InputMessage::decode(registry::EDGE_EXIT, &exit).is_err());

        let button = |b: u8, d: u8| {
            let mut p = Vec::new();
            MARK.put(&mut p);
            p.extend([b, d]);
            InputMessage::decode(registry::BUTTON_INPUT, &p)
        };
        let ty = registry::BUTTON_INPUT;
        assert_eq!(
            button(5, 1),
            Err(ProtocolError::InvalidCode { ty, value: 5 })
        );
        assert_eq!(
            button(9, 1),
            Err(ProtocolError::InvalidCode { ty, value: 9 })
        );
        assert_eq!(
            button(0, 2),
            Err(ProtocolError::InvalidCode { ty, value: 2 })
        );

        let mut key = Vec::new();
        MARK.put(&mut key);
        key.extend([0x04, 0x00, 2]);
        assert_eq!(
            InputMessage::decode(registry::KEY_INPUT, &key),
            Err(ProtocolError::InvalidCode {
                ty: registry::KEY_INPUT,
                value: 2
            })
        );
    }

    #[test]
    fn other_types_are_unknown() {
        assert_eq!(
            InputMessage::decode(registry::HELLO, &[]),
            Err(ProtocolError::UnknownType {
                ty: registry::HELLO
            })
        );
    }

    #[test]
    fn stream_and_datagram_encodings() {
        let mut frame = Vec::new();
        InputMessage::TakenBack.encode(&mut frame).unwrap();
        assert_eq!(frame, [0, 0, 0, 0, 0x09, 0x00]);

        let mut datagram = Vec::new();
        InputMessage::Motion(Mark {
            seq: 1,
            dx: -1,
            dy: 2,
        })
        .encode(&mut datagram)
        .unwrap();
        assert_eq!(
            datagram,
            [0x10, 0x00, 1, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 2, 0, 0, 0]
        );
    }
}
