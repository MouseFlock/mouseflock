//! Wire messages, framing, and versioning. No I/O.
//!
//! The only crate that defines wire types. A change here bumps
//! [`PROTOCOL_VERSION`] and updates `docs/protocol.md` in the same PR.
//!
//! Everything here is sans-I/O: decoders take the bytes the caller has
//! buffered and never allocate, so `transport` owns all buffering and the
//! memory a peer can make us hold is bounded by [`frame::MAX_FRAME_PAYLOAD`].
//! Decoding must never panic on any input; the lints below enforce that.

#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::arithmetic_side_effects
    )
)]

pub mod codec;
pub mod datagram;
pub mod error;
pub mod frame;
pub mod hello;
pub mod input;
pub mod pairing;
pub mod registry;
pub mod session;

pub use error::{EncodeError, ProtocolError};

/// Highest peer-to-peer protocol version this release speaks.
pub const PROTOCOL_VERSION: u16 = 1;

/// Lowest peer-to-peer protocol version this release speaks.
pub const MIN_PROTOCOL_VERSION: u16 = 1;

/// Why a connection was closed, sent as the QUIC application error code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    Normal,
    ProtocolError,
    VersionMismatch,
    /// A key-confirmation tag was wrong: the codes differ, or something relayed
    /// the connection.
    WrongPairingCode,
    /// The machine is not in pairing mode, or another attempt is running.
    NotReadyToPair,
    /// The other machine does not count this one as a member of its desk.
    NotAMember,
}

impl CloseReason {
    /// The QUIC application error code for this reason.
    pub const fn code(self) -> u64 {
        match self {
            Self::Normal => 0,
            Self::ProtocolError => 1,
            Self::VersionMismatch => 2,
            Self::WrongPairingCode => 3,
            Self::NotReadyToPair => 4,
            Self::NotAMember => 5,
        }
    }

    /// The reason for a QUIC application error code, if it is one we define.
    pub const fn from_code(code: u64) -> Option<Self> {
        match code {
            0 => Some(Self::Normal),
            1 => Some(Self::ProtocolError),
            2 => Some(Self::VersionMismatch),
            3 => Some(Self::WrongPairingCode),
            4 => Some(Self::NotReadyToPair),
            5 => Some(Self::NotAMember),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_codes_round_trip() {
        for reason in [
            CloseReason::Normal,
            CloseReason::ProtocolError,
            CloseReason::VersionMismatch,
            CloseReason::WrongPairingCode,
            CloseReason::NotReadyToPair,
            CloseReason::NotAMember,
        ] {
            assert_eq!(CloseReason::from_code(reason.code()), Some(reason));
        }
        assert_eq!(CloseReason::from_code(6), None);
    }

    #[test]
    fn close_codes_match_spec() {
        assert_eq!(CloseReason::Normal.code(), 0);
        assert_eq!(CloseReason::ProtocolError.code(), 1);
        assert_eq!(CloseReason::VersionMismatch.code(), 2);
        assert_eq!(CloseReason::WrongPairingCode.code(), 3);
        assert_eq!(CloseReason::NotReadyToPair.code(), 4);
        assert_eq!(CloseReason::NotAMember.code(), 5);
    }
}
