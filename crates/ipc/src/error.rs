//! Decoding and encoding errors.

use std::fmt;

/// Input from the other end that breaks the IPC protocol. The connection
/// must be closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolError {
    /// A frame header claims a payload over the frame limit.
    FrameTooLarge { len: u32 },
    /// Message type `0x0000`.
    ReservedType,
    /// A message type the agreed version does not define.
    UnknownType { ty: u16 },
    /// A message type travelling in a direction it may not.
    WrongDirection { ty: u16 },
    /// A payload that does not match its message type's layout.
    PayloadLength {
        ty: u16,
        expected: usize,
        actual: usize,
    },
    /// A code the layout does not define.
    InvalidCode { ty: u16, value: u8 },
    /// The first frame is not `Hello`.
    ExpectedHello { ty: u16 },
    /// A second `Hello`.
    DuplicateHello,
    /// A `Hello` whose lowest version is 0 or above its highest.
    InvalidVersionRange { min: u16, max: u16 },
    /// A screen report that breaks a limit: too many monitors, a bad
    /// identity, an empty or overflowing rectangle, or an implausible DPI.
    InvalidScreen,
    /// A name, code or count outside its limits, or text that is not
    /// UTF-8 or printable ASCII as its layout requires.
    InvalidField { ty: u16 },
    /// A clipboard message that breaks a run: a chunk outside a run, too
    /// many bytes for it, or another clipboard message cutting it short.
    ClipboardRun { ty: u16 },
    /// An agent message its role may not send: clipboard from the winlogon
    /// agent, or `InputDesktop` from the session agent.
    WrongRole { ty: u16, role: u8 },
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FrameTooLarge { len } => write!(f, "frame payload of {len} bytes over limit"),
            Self::ReservedType => write!(f, "reserved message type 0x0000"),
            Self::UnknownType { ty } => write!(f, "unknown message type {ty:#06x}"),
            Self::WrongDirection { ty } => {
                write!(f, "message type {ty:#06x} sent in the wrong direction")
            }
            Self::PayloadLength {
                ty,
                expected,
                actual,
            } => write!(
                f,
                "message type {ty:#06x} payload is {actual} bytes, expected {expected}"
            ),
            Self::InvalidCode { ty, value } => {
                write!(f, "message type {ty:#06x} has undefined code {value}")
            }
            Self::ExpectedHello { ty } => {
                write!(f, "expected Hello first, got message type {ty:#06x}")
            }
            Self::DuplicateHello => write!(f, "second Hello"),
            Self::InvalidVersionRange { min, max } => {
                write!(f, "invalid version range {min}..={max}")
            }
            Self::InvalidScreen => write!(f, "screen report breaks a limit"),
            Self::InvalidField { ty } => {
                write!(f, "message type {ty:#06x} has a field outside its limits")
            }
            Self::ClipboardRun { ty } => {
                write!(f, "message type {ty:#06x} breaks a clipboard run")
            }
            Self::WrongRole { ty, role } => {
                write!(f, "message type {ty:#06x} may not come from role {role}")
            }
        }
    }
}

impl std::error::Error for ProtocolError {}

/// A message this side tried to send that cannot be encoded. Always a bug in
/// the caller or an input outside the protocol's limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError {
    /// Payload over the frame limit.
    PayloadTooLarge { len: usize },
    /// A screen outside the limits a receiver enforces.
    InvalidScreen,
    /// A name, code or member list outside the limits a receiver enforces.
    InvalidField,
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLarge { len } => write!(f, "payload of {len} bytes over limit"),
            Self::InvalidScreen => write!(f, "screen outside the protocol's limits"),
            Self::InvalidField => {
                write!(f, "name, code or member list outside the protocol's limits")
            }
        }
    }
}

impl std::error::Error for EncodeError {}
