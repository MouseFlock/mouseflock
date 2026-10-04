//! Decoding and encoding errors.

use std::fmt;

/// Input from a peer that breaks the wire protocol. The connection must be
/// closed with [`crate::CloseReason::ProtocolError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolError {
    /// A stream frame header claims a payload over the frame limit.
    FrameTooLarge { len: u32 },
    /// A datagram is shorter than its 2-byte type.
    DatagramTooShort { len: usize },
    /// A datagram is over the datagram limit.
    DatagramTooLarge { len: usize },
    /// Message type `0x0000`.
    ReservedType,
    /// A message type the negotiated version does not define.
    UnknownType { ty: u16 },
    /// A message type sent on a channel it may not use.
    WrongChannel { ty: u16 },
    /// A payload that does not match its message type's layout.
    PayloadLength {
        ty: u16,
        expected: usize,
        actual: usize,
    },
    /// The first frame on the control stream is not `Hello`.
    ExpectedHello { ty: u16 },
    /// A second `Hello` on the control stream.
    DuplicateHello,
    /// A `Hello` on a stream other than the control stream.
    HelloOutsideControlStream,
    /// A `Hello` payload shorter than its version range.
    HelloTooShort { len: usize },
    /// A `Hello` whose lowest version is 0 or above its highest.
    InvalidVersionRange { min: u16, max: u16 },
    /// A machine name length outside its range for the message.
    InvalidNameLength { len: usize },
    /// A machine name that is not UTF-8.
    NameNotUtf8,
    /// A `MemberRecord` kind other than addition or removal.
    InvalidRecordKind { kind: u8 },
    /// A side, button, or pressed-or-released code the layout does not define.
    InvalidCode { ty: u16, value: u8 },
    /// Input after the session already failed.
    SessionFailed,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FrameTooLarge { len } => write!(f, "frame payload of {len} bytes over limit"),
            Self::DatagramTooShort { len } => write!(f, "datagram of {len} bytes too short"),
            Self::DatagramTooLarge { len } => write!(f, "datagram of {len} bytes over limit"),
            Self::ReservedType => write!(f, "reserved message type 0x0000"),
            Self::UnknownType { ty } => write!(f, "unknown message type {ty:#06x}"),
            Self::WrongChannel { ty } => write!(f, "message type {ty:#06x} on wrong channel"),
            Self::PayloadLength {
                ty,
                expected,
                actual,
            } => write!(
                f,
                "message type {ty:#06x} payload is {actual} bytes, expected {expected}"
            ),
            Self::ExpectedHello { ty } => {
                write!(f, "expected Hello first, got message type {ty:#06x}")
            }
            Self::DuplicateHello => write!(f, "second Hello on control stream"),
            Self::HelloOutsideControlStream => write!(f, "Hello outside the control stream"),
            Self::HelloTooShort { len } => write!(f, "Hello payload of {len} bytes too short"),
            Self::InvalidVersionRange { min, max } => {
                write!(f, "invalid version range {min}..={max}")
            }
            Self::InvalidNameLength { len } => {
                write!(f, "machine name of {len} bytes out of range")
            }
            Self::NameNotUtf8 => write!(f, "machine name is not UTF-8"),
            Self::InvalidRecordKind { kind } => write!(f, "unknown membership record kind {kind}"),
            Self::InvalidCode { ty, value } => {
                write!(f, "message type {ty:#06x} has undefined code {value}")
            }
            Self::SessionFailed => write!(f, "session already failed"),
        }
    }
}

impl std::error::Error for ProtocolError {}

/// A message this side tried to send that cannot be encoded. Always a bug in
/// the caller, never caused by a peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError {
    /// Payload over the frame or datagram limit.
    PayloadTooLarge { len: usize },
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLarge { len } => write!(f, "payload of {len} bytes over limit"),
        }
    }
}

impl std::error::Error for EncodeError {}
