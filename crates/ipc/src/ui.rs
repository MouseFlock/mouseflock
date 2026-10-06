//! The UI link's messages (IPC version 1): commands from a UI to the
//! service, and the service's status and notices.
//!
//! Keys are 32-byte Ed25519 public keys. Names are 1 to 63 bytes of UTF-8,
//! codes 1 to 32 bytes of printable ASCII, and a `Status` lists at most 256
//! members. Codes are sent as the user typed them; the service parses them.

use model::Side;

use crate::message::{Direction, Fields, check, exactly};
use crate::{EncodeError, ProtocolError, frame};

pub const OPEN_PAIRING: u16 = 0x0030;
pub const CLOSE_PAIRING: u16 = 0x0031;
pub const JOIN: u16 = 0x0032;
pub const REMOVE_PEER: u16 = 0x0033;
pub const UNDO: u16 = 0x0034;
pub const FORGET_LAYOUT: u16 = 0x0035;

pub const STATUS: u16 = 0x0038;
pub const PAIRED: u16 = 0x0039;
pub const JOIN_FAILED: u16 = 0x003A;
pub const PAIRING_CLOSED: u16 = 0x003B;
pub const PAIRING_NOT_OPENED: u16 = 0x003C;
pub const LEARNED: u16 = 0x003D;

/// Longest machine name, in bytes of UTF-8.
pub const MAX_NAME_LEN: usize = 63;
/// Longest pairing code as typed, in bytes.
pub const MAX_CODE_LEN: usize = 32;
/// Most members a `Status` lists.
pub const MAX_MEMBERS: usize = 256;

/// A machine's identity key.
pub type PeerKey = [u8; 32];

// ---- UI → service ------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiCommand {
    OpenPairing,
    ClosePairing,
    /// Pair with the machine showing this code, as typed.
    Join(String),
    RemovePeer(PeerKey),
    Undo,
    ForgetLayout,
}

impl UiCommand {
    pub const fn ty(&self) -> u16 {
        match self {
            Self::OpenPairing => OPEN_PAIRING,
            Self::ClosePairing => CLOSE_PAIRING,
            Self::Join(_) => JOIN,
            Self::RemovePeer(_) => REMOVE_PEER,
            Self::Undo => UNDO,
            Self::ForgetLayout => FORGET_LAYOUT,
        }
    }

    pub fn payload(&self) -> Result<Vec<u8>, EncodeError> {
        let mut p = Vec::new();
        match self {
            Self::Join(code) => put_code(&mut p, code)?,
            Self::RemovePeer(key) => p.extend_from_slice(key),
            Self::OpenPairing | Self::ClosePairing | Self::Undo | Self::ForgetLayout => {}
        }
        Ok(p)
    }

    pub fn encode_frame(&self, out: &mut Vec<u8>) -> Result<(), EncodeError> {
        frame::encode(self.ty(), &self.payload()?, out)
    }

    /// Decodes a command the service received from a UI, after `Hello`.
    pub fn decode(ty: u16, payload: &[u8], version: u16) -> Result<Self, ProtocolError> {
        check(ty, Direction::UiToService, version)?;
        let mut f = Fields::new(ty, payload);
        let command = match ty {
            crate::message::HELLO => return Err(ProtocolError::DuplicateHello),
            OPEN_PAIRING | CLOSE_PAIRING | UNDO | FORGET_LAYOUT => {
                exactly(ty, payload, 0)?;
                match ty {
                    OPEN_PAIRING => Self::OpenPairing,
                    CLOSE_PAIRING => Self::ClosePairing,
                    UNDO => Self::Undo,
                    _ => Self::ForgetLayout,
                }
            }
            JOIN => Self::Join(get_code(&mut f)?),
            REMOVE_PEER => {
                exactly(ty, payload, 32)?;
                Self::RemovePeer(f.array()?)
            }
            _ => return Err(ProtocolError::UnknownType { ty }),
        };
        f.end()?;
        Ok(command)
    }
}

// ---- service → UI ------------------------------------------------------------

/// A member of this machine's desk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub key: PeerKey,
    pub name: String,
    pub up: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pairing {
    Closed,
    /// Open with this code, for `left_ms` more milliseconds.
    Open {
        code: String,
        left_ms: u32,
    },
}

/// Where this machine's focus is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusOf {
    Local,
    /// This machine's input goes to this member.
    On(PeerKey),
    /// This member's input is injected here.
    ControlledBy(PeerKey),
}

/// Everything a UI shows about this machine. Each one is complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub key: PeerKey,
    pub name: String,
    pub agent_linked: bool,
    pub pairing: Pairing,
    pub focus: FocusOf,
    pub members: Vec<Member>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinFailedReason {
    WrongCode,
    NotReady,
    ProtocolError,
    DeskFull,
    ConnectionLost,
    Unreachable,
    /// The check digit does not match: a typo.
    MistypedCode,
    /// Not a code at all.
    MalformedCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingClosedReason {
    Joined,
    Expired,
    ByUser,
    TooManyFailures,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotOpenedReason {
    NoNetwork,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEvent {
    Status(Status),
    Paired {
        key: PeerKey,
        name: String,
    },
    JoinFailed(JoinFailedReason),
    PairingClosed(PairingClosedReason),
    PairingNotOpened(NotOpenedReason),
    /// A side was learned toward this member; Undo works for `undo_ms`.
    Learned {
        key: PeerKey,
        side: Side,
        undo_ms: u32,
    },
}

impl UiEvent {
    pub const fn ty(&self) -> u16 {
        match self {
            Self::Status(_) => STATUS,
            Self::Paired { .. } => PAIRED,
            Self::JoinFailed(_) => JOIN_FAILED,
            Self::PairingClosed(_) => PAIRING_CLOSED,
            Self::PairingNotOpened(_) => PAIRING_NOT_OPENED,
            Self::Learned { .. } => LEARNED,
        }
    }

    pub fn payload(&self) -> Result<Vec<u8>, EncodeError> {
        let mut p = Vec::new();
        match self {
            Self::Status(s) => {
                p.extend_from_slice(&s.key);
                put_name(&mut p, &s.name)?;
                p.push(u8::from(s.agent_linked));
                match &s.pairing {
                    Pairing::Closed => p.push(0),
                    Pairing::Open { code, left_ms } => {
                        p.push(1);
                        put_code(&mut p, code)?;
                        p.extend_from_slice(&left_ms.to_le_bytes());
                    }
                }
                match s.focus {
                    FocusOf::Local => p.push(0),
                    FocusOf::On(key) => {
                        p.push(1);
                        p.extend_from_slice(&key);
                    }
                    FocusOf::ControlledBy(key) => {
                        p.push(2);
                        p.extend_from_slice(&key);
                    }
                }
                let n = u16::try_from(s.members.len())
                    .ok()
                    .filter(|n| usize::from(*n) <= MAX_MEMBERS)
                    .ok_or(EncodeError::InvalidField)?;
                p.extend_from_slice(&n.to_le_bytes());
                for m in &s.members {
                    p.extend_from_slice(&m.key);
                    put_name(&mut p, &m.name)?;
                    p.push(u8::from(m.up));
                }
            }
            Self::Paired { key, name } => {
                p.extend_from_slice(key);
                put_name(&mut p, name)?;
            }
            Self::JoinFailed(r) => p.push(join_failed_code(*r)),
            Self::PairingClosed(r) => p.push(pairing_closed_code(*r)),
            Self::PairingNotOpened(NotOpenedReason::NoNetwork) => p.push(0),
            Self::Learned { key, side, undo_ms } => {
                p.extend_from_slice(key);
                p.push(side_code(*side));
                p.extend_from_slice(&undo_ms.to_le_bytes());
            }
        }
        Ok(p)
    }

    pub fn encode_frame(&self, out: &mut Vec<u8>) -> Result<(), EncodeError> {
        frame::encode(self.ty(), &self.payload()?, out)
    }

    /// Decodes a message a UI received from the service, after `Hello`.
    pub fn decode(ty: u16, payload: &[u8], version: u16) -> Result<Self, ProtocolError> {
        check(ty, Direction::ServiceToUi, version)?;
        let mut f = Fields::new(ty, payload);
        let event = match ty {
            crate::message::HELLO => return Err(ProtocolError::DuplicateHello),
            STATUS => {
                let key = f.array()?;
                let name = get_name(&mut f)?;
                let agent_linked = get_bool(&mut f)?;
                let pairing = match f.u8()? {
                    0 => Pairing::Closed,
                    1 => Pairing::Open {
                        code: get_code(&mut f)?,
                        left_ms: f.u32()?,
                    },
                    value => return Err(ProtocolError::InvalidCode { ty, value }),
                };
                let focus = match f.u8()? {
                    0 => FocusOf::Local,
                    1 => FocusOf::On(f.array()?),
                    2 => FocusOf::ControlledBy(f.array()?),
                    value => return Err(ProtocolError::InvalidCode { ty, value }),
                };
                let n = usize::from(f.u16()?);
                if n > MAX_MEMBERS {
                    return Err(ProtocolError::InvalidField { ty });
                }
                let mut members = Vec::with_capacity(n);
                for _ in 0..n {
                    members.push(Member {
                        key: f.array()?,
                        name: get_name(&mut f)?,
                        up: get_bool(&mut f)?,
                    });
                }
                Self::Status(Status {
                    key,
                    name,
                    agent_linked,
                    pairing,
                    focus,
                    members,
                })
            }
            PAIRED => Self::Paired {
                key: f.array()?,
                name: get_name(&mut f)?,
            },
            JOIN_FAILED => {
                exactly(ty, payload, 1)?;
                Self::JoinFailed(join_failed_from(ty, f.u8()?)?)
            }
            PAIRING_CLOSED => {
                exactly(ty, payload, 1)?;
                Self::PairingClosed(pairing_closed_from(ty, f.u8()?)?)
            }
            PAIRING_NOT_OPENED => {
                exactly(ty, payload, 1)?;
                match f.u8()? {
                    0 => Self::PairingNotOpened(NotOpenedReason::NoNetwork),
                    value => return Err(ProtocolError::InvalidCode { ty, value }),
                }
            }
            LEARNED => {
                exactly(ty, payload, 37)?;
                Self::Learned {
                    key: f.array()?,
                    side: side_from(ty, f.u8()?)?,
                    undo_ms: f.u32()?,
                }
            }
            _ => return Err(ProtocolError::UnknownType { ty }),
        };
        f.end()?;
        Ok(event)
    }
}

// ---- fields and codes ----------------------------------------------------------

fn put_name(p: &mut Vec<u8>, name: &str) -> Result<(), EncodeError> {
    let len = u8::try_from(name.len())
        .ok()
        .filter(|n| (1..=MAX_NAME_LEN).contains(&usize::from(*n)))
        .ok_or(EncodeError::InvalidField)?;
    p.push(len);
    p.extend_from_slice(name.as_bytes());
    Ok(())
}

fn get_name(f: &mut Fields<'_>) -> Result<String, ProtocolError> {
    let ty = f.ty();
    let len = usize::from(f.u8()?);
    if !(1..=MAX_NAME_LEN).contains(&len) {
        return Err(ProtocolError::InvalidField { ty });
    }
    let bytes = f.bytes(len)?;
    core::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| ProtocolError::InvalidField { ty })
}

fn code_ok(code: &[u8]) -> bool {
    (1..=MAX_CODE_LEN).contains(&code.len()) && code.iter().all(|b| (0x21..=0x7e).contains(b))
}

fn put_code(p: &mut Vec<u8>, code: &str) -> Result<(), EncodeError> {
    let bytes = code.as_bytes();
    if !code_ok(bytes) {
        return Err(EncodeError::InvalidField);
    }
    let len = u8::try_from(bytes.len()).map_err(|_| EncodeError::InvalidField)?;
    p.push(len);
    p.extend_from_slice(bytes);
    Ok(())
}

fn get_code(f: &mut Fields<'_>) -> Result<String, ProtocolError> {
    let ty = f.ty();
    let len = usize::from(f.u8()?);
    if !(1..=MAX_CODE_LEN).contains(&len) {
        return Err(ProtocolError::InvalidField { ty });
    }
    let bytes = f.bytes(len)?;
    if !code_ok(bytes) {
        return Err(ProtocolError::InvalidField { ty });
    }
    // Printable ASCII is UTF-8.
    core::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| ProtocolError::InvalidField { ty })
}

fn get_bool(f: &mut Fields<'_>) -> Result<bool, ProtocolError> {
    let ty = f.ty();
    match f.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        value => Err(ProtocolError::InvalidCode { ty, value }),
    }
}

const fn side_code(side: Side) -> u8 {
    match side {
        Side::Left => 0,
        Side::Right => 1,
        Side::Top => 2,
        Side::Bottom => 3,
    }
}

const fn side_from(ty: u16, value: u8) -> Result<Side, ProtocolError> {
    match value {
        0 => Ok(Side::Left),
        1 => Ok(Side::Right),
        2 => Ok(Side::Top),
        3 => Ok(Side::Bottom),
        _ => Err(ProtocolError::InvalidCode { ty, value }),
    }
}

const fn join_failed_code(r: JoinFailedReason) -> u8 {
    match r {
        JoinFailedReason::WrongCode => 0,
        JoinFailedReason::NotReady => 1,
        JoinFailedReason::ProtocolError => 2,
        JoinFailedReason::DeskFull => 3,
        JoinFailedReason::ConnectionLost => 4,
        JoinFailedReason::Unreachable => 5,
        JoinFailedReason::MistypedCode => 6,
        JoinFailedReason::MalformedCode => 7,
    }
}

const fn join_failed_from(ty: u16, value: u8) -> Result<JoinFailedReason, ProtocolError> {
    Ok(match value {
        0 => JoinFailedReason::WrongCode,
        1 => JoinFailedReason::NotReady,
        2 => JoinFailedReason::ProtocolError,
        3 => JoinFailedReason::DeskFull,
        4 => JoinFailedReason::ConnectionLost,
        5 => JoinFailedReason::Unreachable,
        6 => JoinFailedReason::MistypedCode,
        7 => JoinFailedReason::MalformedCode,
        _ => return Err(ProtocolError::InvalidCode { ty, value }),
    })
}

const fn pairing_closed_code(r: PairingClosedReason) -> u8 {
    match r {
        PairingClosedReason::Joined => 0,
        PairingClosedReason::Expired => 1,
        PairingClosedReason::ByUser => 2,
        PairingClosedReason::TooManyFailures => 3,
    }
}

const fn pairing_closed_from(ty: u16, value: u8) -> Result<PairingClosedReason, ProtocolError> {
    Ok(match value {
        0 => PairingClosedReason::Joined,
        1 => PairingClosedReason::Expired,
        2 => PairingClosedReason::ByUser,
        3 => PairingClosedReason::TooManyFailures,
        _ => return Err(ProtocolError::InvalidCode { ty, value }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{KEY_EVENT, SET_CAPTURE, ToAgent, ToService};

    fn name(n: usize) -> String {
        "n".repeat(n)
    }

    fn status(members: usize, name_len: usize) -> Status {
        Status {
            key: [1; 32],
            name: name(name_len),
            agent_linked: true,
            pairing: Pairing::Open {
                code: "12-3456789".into(),
                left_ms: 599_000,
            },
            focus: FocusOf::ControlledBy([2; 32]),
            members: (0..members)
                .map(|i| Member {
                    key: [u8::try_from(i % 256).unwrap(); 32],
                    name: name(name_len),
                    up: i % 2 == 0,
                })
                .collect(),
        }
    }

    fn commands() -> Vec<UiCommand> {
        vec![
            UiCommand::OpenPairing,
            UiCommand::ClosePairing,
            UiCommand::Join("01-2345678".into()),
            UiCommand::Join("x".repeat(32)),
            UiCommand::RemovePeer([9; 32]),
            UiCommand::Undo,
            UiCommand::ForgetLayout,
        ]
    }

    fn events() -> Vec<UiEvent> {
        let mut closed = status(0, 1);
        closed.pairing = Pairing::Closed;
        closed.focus = FocusOf::Local;
        closed.agent_linked = false;
        let mut on = status(3, 10);
        on.focus = FocusOf::On([4; 32]);
        vec![
            UiEvent::Status(closed),
            UiEvent::Status(on),
            UiEvent::Status(status(2, 63)),
            UiEvent::Paired {
                key: [5; 32],
                name: "LAPTOP-7 é".into(),
            },
            UiEvent::JoinFailed(JoinFailedReason::MistypedCode),
            UiEvent::JoinFailed(JoinFailedReason::WrongCode),
            UiEvent::PairingClosed(PairingClosedReason::TooManyFailures),
            UiEvent::PairingNotOpened(NotOpenedReason::NoNetwork),
            UiEvent::Learned {
                key: [6; 32],
                side: Side::Bottom,
                undo_ms: 10_000,
            },
        ]
    }

    #[test]
    fn every_ui_message_round_trips() {
        for c in commands() {
            let p = c.payload().unwrap();
            assert_eq!(UiCommand::decode(c.ty(), &p, 1).unwrap(), c);
        }
        for e in events() {
            let p = e.payload().unwrap();
            assert_eq!(UiEvent::decode(e.ty(), &p, 1).unwrap(), e);
        }
    }

    #[test]
    fn one_byte_short_or_long_is_rejected() {
        for c in commands() {
            let p = c.payload().unwrap();
            let mut long = p.clone();
            long.push(0);
            assert!(UiCommand::decode(c.ty(), &long, 1).is_err(), "{c:?} long");
            if let Some((_, short)) = p.split_last() {
                assert!(UiCommand::decode(c.ty(), short, 1).is_err(), "{c:?} short");
            }
        }
        for e in events() {
            let p = e.payload().unwrap();
            let mut long = p.clone();
            long.push(0);
            assert!(UiEvent::decode(e.ty(), &long, 1).is_err(), "{e:?} long");
            let (_, short) = p.split_last().unwrap();
            assert!(UiEvent::decode(e.ty(), short, 1).is_err(), "{e:?} short");
        }
    }

    #[test]
    fn limits_are_enforced_both_ways() {
        // Encoding refuses what decoding would reject.
        for bad in [name(0), name(64)] {
            let e = UiEvent::Paired {
                key: [0; 32],
                name: bad,
            };
            assert_eq!(e.payload(), Err(EncodeError::InvalidField));
        }
        for bad in ["", "has space", &"x".repeat(33), "é"] {
            assert_eq!(
                UiCommand::Join(bad.into()).payload(),
                Err(EncodeError::InvalidField),
                "{bad:?}"
            );
        }
        assert_eq!(
            UiEvent::Status(status(257, 1)).payload(),
            Err(EncodeError::InvalidField)
        );

        // Hand-made payloads that break a limit.
        fn field<T>(ty: u16) -> Result<T, ProtocolError> {
            Err(ProtocolError::InvalidField { ty })
        }
        let mut long_name = vec![0; 32];
        long_name.push(64);
        long_name.extend(std::iter::repeat_n(b'a', 64));
        assert_eq!(UiEvent::decode(PAIRED, &long_name, 1), field(PAIRED));
        let mut empty_name = vec![0; 32];
        empty_name.push(0);
        assert_eq!(UiEvent::decode(PAIRED, &empty_name, 1), field(PAIRED));
        let mut not_utf8 = vec![0; 32];
        not_utf8.extend([2, 0xc3, 0x28]);
        assert_eq!(UiEvent::decode(PAIRED, &not_utf8, 1), field(PAIRED));

        let mut long_code = vec![33];
        long_code.extend(std::iter::repeat_n(b'1', 33));
        assert_eq!(UiCommand::decode(JOIN, &long_code, 1), field(JOIN));
        assert_eq!(UiCommand::decode(JOIN, b"\x031 2", 1), field(JOIN));

        let mut many = UiEvent::Status(status(0, 1)).payload().unwrap();
        let n = many.len() - 2;
        many[n..].copy_from_slice(&257u16.to_le_bytes());
        assert_eq!(UiEvent::decode(STATUS, &many, 1), field(STATUS));
    }

    #[test]
    fn undefined_codes_are_rejected() {
        let code = |ty, value| Err(ProtocolError::InvalidCode { ty, value });
        assert_eq!(UiEvent::decode(JOIN_FAILED, &[8], 1), code(JOIN_FAILED, 8));
        assert_eq!(
            UiEvent::decode(PAIRING_CLOSED, &[4], 1),
            code(PAIRING_CLOSED, 4)
        );
        assert_eq!(
            UiEvent::decode(PAIRING_NOT_OPENED, &[1], 1),
            code(PAIRING_NOT_OPENED, 1)
        );
        let mut learned = vec![0; 32];
        learned.push(4);
        learned.extend(0u32.to_le_bytes());
        assert_eq!(UiEvent::decode(LEARNED, &learned, 1), code(LEARNED, 4));

        // `up` = 2 on the one member.
        let mut s = UiEvent::Status(status(1, 1)).payload().unwrap();
        *s.last_mut().unwrap() = 2;
        assert_eq!(UiEvent::decode(STATUS, &s, 1), code(STATUS, 2));
    }

    #[test]
    fn messages_stay_on_their_link() {
        fn wrong<T>(ty: u16) -> Result<T, ProtocolError> {
            Err(ProtocolError::WrongDirection { ty })
        }
        // A UI type on an agent link, both ways.
        assert_eq!(ToService::decode(OPEN_PAIRING, &[], 1), wrong(OPEN_PAIRING));
        assert_eq!(ToAgent::decode(STATUS, &[], 1), wrong(STATUS));
        // An agent type on a UI link, both ways.
        assert_eq!(UiCommand::decode(KEY_EVENT, &[0; 12], 1), wrong(KEY_EVENT));
        assert_eq!(UiEvent::decode(SET_CAPTURE, &[0], 1), wrong(SET_CAPTURE));
        // A UI command sent back to a UI, and a UI event sent to the service.
        assert_eq!(UiEvent::decode(UNDO, &[], 1), wrong(UNDO));
        assert_eq!(UiCommand::decode(LEARNED, &[0; 37], 1), wrong(LEARNED));
    }

    #[test]
    fn largest_status_fits_a_frame() {
        let s = UiEvent::Status(status(256, 63));
        let mut out = Vec::new();
        s.encode_frame(&mut out).unwrap();
        assert!(out.len() <= 65_536 + 6, "{}", out.len());
        let p = s.payload().unwrap();
        assert_eq!(UiEvent::decode(STATUS, &p, 1).unwrap(), s);
    }
}
