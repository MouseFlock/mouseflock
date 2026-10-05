//! Frames: `len: u32le | type: u16le | payload`.
//!
//! The same shape as the peer protocol's frames, with its own registry and
//! version. The header never changes in any IPC version, because it is read
//! before a version is agreed.

use crate::{EncodeError, ProtocolError};

/// Bytes in a frame header.
pub const HEADER_LEN: usize = 6;

/// Largest payload a frame may carry.
pub const MAX_PAYLOAD: usize = 65_536;

/// One decoded frame, borrowing its payload from the caller's buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame<'a> {
    pub ty: u16,
    pub payload: &'a [u8],
}

/// The result of one decoding step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decoded<'a> {
    /// A complete frame, and how many bytes of the input it used.
    Frame { frame: Frame<'a>, consumed: usize },
    /// Not enough input for a complete frame. Nothing was consumed.
    NeedMore,
}

/// Decodes the first frame in `buf`. An oversized length is rejected from
/// the header alone, so a reader never buffers more than [`HEADER_LEN`] +
/// [`MAX_PAYLOAD`] bytes.
pub fn decode(buf: &[u8]) -> Result<Decoded<'_>, ProtocolError> {
    let Some(&[l0, l1, l2, l3, t0, t1]) = buf.first_chunk::<HEADER_LEN>() else {
        return Ok(Decoded::NeedMore);
    };
    let claimed = u32::from_le_bytes([l0, l1, l2, l3]);
    let too_large = ProtocolError::FrameTooLarge { len: claimed };
    let len = usize::try_from(claimed).map_err(|_| too_large)?;
    if len > MAX_PAYLOAD {
        return Err(too_large);
    }
    let end = HEADER_LEN.checked_add(len).ok_or(too_large)?;
    let Some(payload) = buf.get(HEADER_LEN..end) else {
        return Ok(Decoded::NeedMore);
    };
    Ok(Decoded::Frame {
        frame: Frame {
            ty: u16::from_le_bytes([t0, t1]),
            payload,
        },
        consumed: end,
    })
}

/// Appends one frame to `out`.
pub fn encode(ty: u16, payload: &[u8], out: &mut Vec<u8>) -> Result<(), EncodeError> {
    let too_large = EncodeError::PayloadTooLarge { len: payload.len() };
    if payload.len() > MAX_PAYLOAD {
        return Err(too_large);
    }
    let len = u32::try_from(payload.len()).map_err(|_| too_large)?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&ty.to_le_bytes());
    out.extend_from_slice(payload);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_frame_bytes() {
        let mut out = Vec::new();
        encode(0x0003, &[0x01], &mut out).unwrap();
        assert_eq!(out, [0x01, 0, 0, 0, 0x03, 0x00, 0x01]);
        assert_eq!(
            decode(&out),
            Ok(Decoded::Frame {
                frame: Frame {
                    ty: 3,
                    payload: &[1]
                },
                consumed: 7
            })
        );
    }

    #[test]
    fn oversized_header_is_rejected_from_six_bytes() {
        let header = [0x01, 0x00, 0x01, 0x00, 0x10, 0x00];
        assert_eq!(
            decode(&header),
            Err(ProtocolError::FrameTooLarge { len: 65_537 })
        );
        let mut at_limit = Vec::new();
        encode(0x10, &vec![0; MAX_PAYLOAD], &mut at_limit).unwrap();
        assert!(matches!(decode(&at_limit), Ok(Decoded::Frame { .. })));
        assert!(encode(0x10, &vec![0; MAX_PAYLOAD + 1], &mut Vec::new()).is_err());
    }

    #[test]
    fn need_more_at_every_split() {
        let mut out = Vec::new();
        encode(0x0026, &[1, 2, 3, 4, 5, 6, 7, 8], &mut out).unwrap();
        for cut in 0..out.len() {
            assert_eq!(decode(&out[..cut]), Ok(Decoded::NeedMore), "cut {cut}");
        }
    }
}
