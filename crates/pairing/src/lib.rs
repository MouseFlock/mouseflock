//! Identity keys, PAKE pairing, desk membership, trust store.
//!
//! Sans-I/O, like `proto`: the state machines take decoded messages and the
//! caller's monotonic clock, and return frames to send and connections to
//! close. `transport` runs them on QUIC; the service stores the files. See ADR
//! 0004, Amendment 2.

#![forbid(unsafe_code)]

mod code;
mod confirm;
mod desk;
mod exchange;
mod identity;
mod record;

pub use code::{Code, CodeError};
pub use confirm::{Binding, EXPORTER_LABEL, EXPORTER_LEN};
pub use desk::{AddError, Applied, Desk, MAX_SUBJECTS, Machine, UnixTime};
pub use exchange::{
    ATTEMPT_MS, ConnId, JoinFailure, Joiner, JoinerOutput, MAX_FAILURES, ModeClosed, ModeOutput,
    Now, PairingMode, RecordInbox, Violation, WINDOW_MS,
};
pub use identity::{IDENTITY_FILE_LEN, Identity, verify};
pub use record::{Change, MachineName, SignedRecord};

use std::fmt;

/// Why an identity or trust store file could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileError {
    /// Too short, too long, or a length field that does not match.
    Length,
    Magic,
    Version,
    /// A record that does not decode or whose signature does not verify.
    Record,
}

impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Length => "file has the wrong length",
            Self::Magic => "file is not a MouseFlock file of this kind",
            Self::Version => "file format version is not supported",
            Self::Record => "file contains an invalid record",
        })
    }
}

impl std::error::Error for FileError {}

/// A deterministic generator for tests. Not secure; never use outside tests.
#[cfg(test)]
pub(crate) fn test_rng(seed: u64) -> TestRng {
    TestRng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
}

#[cfg(test)]
pub(crate) struct TestRng(u64);

#[cfg(test)]
impl rand_core::RngCore for TestRng {
    fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        rand_core::impls::fill_bytes_via_next(self, dest);
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}

#[cfg(test)]
impl rand_core::CryptoRng for TestRng {}
