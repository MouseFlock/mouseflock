//! `Hello`: `min: u16le | max: u16le`, then fields from later versions.
//!
//! Decoded before any version is agreed, so its first four bytes never change
//! and trailing bytes are ignored.

use crate::{EncodeError, MIN_PROTOCOL_VERSION, PROTOCOL_VERSION, ProtocolError, frame, registry};

/// Bytes of `Hello` that every version understands.
pub const HELLO_LEN: usize = 4;

/// The range of protocol versions a peer speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hello {
    pub min: u16,
    pub max: u16,
}

impl Hello {
    /// The range this release speaks.
    pub const fn this_release() -> Self {
        Self {
            min: MIN_PROTOCOL_VERSION,
            max: PROTOCOL_VERSION,
        }
    }

    /// Decodes a `Hello` payload, ignoring bytes after the version range.
    pub fn decode(payload: &[u8]) -> Result<Self, ProtocolError> {
        let Some(&[a, b, c, d]) = payload.first_chunk::<HELLO_LEN>() else {
            return Err(ProtocolError::HelloTooShort { len: payload.len() });
        };
        let (min, max) = (u16::from_le_bytes([a, b]), u16::from_le_bytes([c, d]));
        if min == 0 || min > max {
            return Err(ProtocolError::InvalidVersionRange { min, max });
        }
        Ok(Self { min, max })
    }

    /// The `Hello` payload.
    pub fn payload(self) -> [u8; HELLO_LEN] {
        let [a, b] = self.min.to_le_bytes();
        let [c, d] = self.max.to_le_bytes();
        [a, b, c, d]
    }

    /// Appends this `Hello` as a complete stream frame to `out`.
    pub fn encode_frame(self, out: &mut Vec<u8>) -> Result<(), EncodeError> {
        frame::encode(registry::HELLO, &self.payload(), out)
    }
}

/// Which side of a failed negotiation runs the older release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Older {
    ThisPeer,
    OtherPeer,
}

/// Two version ranges that do not overlap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VersionMismatch {
    pub local: Hello,
    pub remote: Hello,
    pub older: Older,
}

/// The highest version both ranges contain. Gives the same answer whichever
/// side calls it.
pub fn negotiate(local: Hello, remote: Hello) -> Result<u16, VersionMismatch> {
    let highest = local.max.min(remote.max);
    let lowest = local.min.max(remote.min);
    if highest >= lowest {
        return Ok(highest);
    }
    let older = if remote.max < local.min {
        Older::OtherPeer
    } else {
        Older::ThisPeer
    };
    Err(VersionMismatch {
        local,
        remote,
        older,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn range(min: u16, max: u16) -> Hello {
        Hello { min, max }
    }

    #[test]
    fn this_release_payload_matches_spec() {
        assert_eq!(Hello::this_release().payload(), [0x01, 0x00, 0x02, 0x00]);
    }

    #[test]
    fn this_release_frame_matches_spec() {
        let mut out = Vec::new();
        Hello::this_release().encode_frame(&mut out).unwrap();
        assert_eq!(
            out,
            [0x04, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x02, 0x00]
        );
    }

    #[test]
    fn peer_running_0_1_0_agrees_version_1() {
        assert_eq!(negotiate(Hello::this_release(), range(1, 1)), Ok(1));
        assert_eq!(negotiate(range(1, 1), Hello::this_release()), Ok(1));
    }

    #[test]
    fn trailing_fields_are_ignored() {
        let payload = [0x01, 0x00, 0x03, 0x00, 9, 9, 9, 9, 9, 9];
        assert_eq!(Hello::decode(&payload), Ok(range(1, 3)));
    }

    #[test]
    fn short_payload_is_rejected() {
        assert_eq!(
            Hello::decode(&[0x01, 0x00, 0x01]),
            Err(ProtocolError::HelloTooShort { len: 3 })
        );
    }

    #[test]
    fn lowest_version_zero_is_rejected() {
        assert_eq!(
            Hello::decode(&range(0, 1).payload()),
            Err(ProtocolError::InvalidVersionRange { min: 0, max: 1 })
        );
    }

    #[test]
    fn inverted_range_is_rejected() {
        assert_eq!(
            Hello::decode(&range(3, 2).payload()),
            Err(ProtocolError::InvalidVersionRange { min: 3, max: 2 })
        );
    }

    #[test]
    fn overlapping_ranges_use_highest_common() {
        assert_eq!(negotiate(range(1, 3), range(2, 5)), Ok(3));
    }

    #[test]
    fn negotiation_is_symmetric() {
        let cases = [(1, 3, 2, 5), (1, 1, 1, 1), (2, 9, 4, 4), (1, 2, 4, 5)];
        for (a, b, c, d) in cases {
            let (x, y) = (range(a, b), range(c, d));
            assert_eq!(negotiate(x, y).ok(), negotiate(y, x).ok());
        }
    }

    #[test]
    fn other_peer_older() {
        let result = negotiate(range(4, 5), range(1, 2));
        assert_eq!(result.map_err(|m| m.older), Err(Older::OtherPeer));
    }

    #[test]
    fn this_peer_older() {
        let result = negotiate(range(1, 2), range(4, 5));
        assert_eq!(result.map_err(|m| m.older), Err(Older::ThisPeer));
    }
}
