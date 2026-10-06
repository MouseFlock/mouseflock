//! Clipboard messages: the offer and request on the control stream, and the
//! frames of a clipboard stream (one transfer per unidirectional stream).
//!
//! [`TransferReader`] checks a clipboard stream's order and lengths and lends
//! each chunk's bytes back; like every decoder here it buffers nothing, so the
//! caller owns the text it collects and can bound it by [`MAX_TEXT_LEN`].

use crate::codec::fixed_payload;
use crate::{EncodeError, ProtocolError, frame, registry};

/// Most bytes of UTF-8 text one transfer may carry (4 MiB).
pub const MAX_TEXT_LEN: u32 = 4_194_304;

/// Bytes in a `ClipboardData` payload: id u64, len u32.
const DATA_LEN: usize = 12;

/// `ClipboardOffer` or `ClipboardRequest`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardMessage {
    /// The sender has a new shareable copy with this ID. No text.
    Offer { id: u64 },
    /// The sender asks for the receiver's copy with this ID.
    Request { id: u64 },
}

impl ClipboardMessage {
    pub const fn ty(&self) -> u16 {
        match self {
            Self::Offer { .. } => registry::CLIPBOARD_OFFER,
            Self::Request { .. } => registry::CLIPBOARD_REQUEST,
        }
    }

    /// Whether `ty` is one of these messages.
    pub const fn is_clipboard(ty: u16) -> bool {
        matches!(ty, registry::CLIPBOARD_OFFER | registry::CLIPBOARD_REQUEST)
    }

    /// Decodes a control-stream message already checked by the session.
    pub fn decode(ty: u16, payload: &[u8]) -> Result<Self, ProtocolError> {
        let id = u64::from_le_bytes(fixed_payload::<8>(ty, payload)?);
        match ty {
            registry::CLIPBOARD_OFFER => Ok(Self::Offer { id }),
            registry::CLIPBOARD_REQUEST => Ok(Self::Request { id }),
            _ => Err(ProtocolError::UnknownType { ty }),
        }
    }

    pub const fn payload(&self) -> [u8; 8] {
        match self {
            Self::Offer { id } | Self::Request { id } => id.to_le_bytes(),
        }
    }

    /// Appends this message as one frame.
    pub fn encode(&self, out: &mut Vec<u8>) -> Result<(), EncodeError> {
        frame::encode(self.ty(), &self.payload(), out)
    }
}

/// Appends a clipboard stream's first frame: `ClipboardData` with `id` and the
/// text's length.
pub fn encode_data(id: u64, len: u32, out: &mut Vec<u8>) -> Result<(), EncodeError> {
    let mut payload = [0u8; DATA_LEN];
    let (id_bytes, len_bytes) = payload.split_at_mut(8);
    id_bytes.copy_from_slice(&id.to_le_bytes());
    len_bytes.copy_from_slice(&len.to_le_bytes());
    frame::encode(registry::CLIPBOARD_DATA, &payload, out)
}

/// Appends one `ClipboardChunk` frame. `bytes` must be 1 to
/// [`frame::MAX_FRAME_PAYLOAD`] long.
pub fn encode_chunk(bytes: &[u8], out: &mut Vec<u8>) -> Result<(), EncodeError> {
    if bytes.is_empty() {
        return Err(EncodeError::PayloadTooLarge { len: 0 });
    }
    frame::encode(registry::CLIPBOARD_CHUNK, bytes, out)
}

/// What one clipboard-stream frame contributed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferStep<'a> {
    /// The transfer's header: which copy, and how many bytes follow.
    Started { id: u64, len: u32 },
    /// More of the text.
    Chunk(&'a [u8]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    AwaitingData,
    Body { remaining: u32 },
    Failed,
}

/// Checks the frames of one clipboard stream: exactly one `ClipboardData`
/// first, then non-empty chunks totalling exactly its length, at most
/// [`MAX_TEXT_LEN`].
#[derive(Debug, Clone)]
pub struct TransferReader {
    state: State,
}

impl Default for TransferReader {
    fn default() -> Self {
        Self::new()
    }
}

impl TransferReader {
    pub const fn new() -> Self {
        Self {
            state: State::AwaitingData,
        }
    }

    /// Handles one message from the stream, already checked by the session.
    pub fn on_message<'a>(
        &mut self,
        ty: u16,
        payload: &'a [u8],
    ) -> Result<TransferStep<'a>, ProtocolError> {
        let result = self.step(ty, payload);
        if result.is_err() {
            self.state = State::Failed;
        }
        result
    }

    fn step<'a>(&mut self, ty: u16, payload: &'a [u8]) -> Result<TransferStep<'a>, ProtocolError> {
        match (self.state, ty) {
            (State::Failed, _) => Err(ProtocolError::SessionFailed),
            (State::AwaitingData, registry::CLIPBOARD_DATA) => {
                let p = fixed_payload::<DATA_LEN>(ty, payload)?;
                let (Some(id), Some(len)) = (p.first_chunk::<8>(), p.last_chunk::<4>()) else {
                    return Err(short(ty, payload));
                };
                let (id, len) = (u64::from_le_bytes(*id), u32::from_le_bytes(*len));
                if len > MAX_TEXT_LEN {
                    return Err(ProtocolError::TextTooLong { len });
                }
                self.state = State::Body { remaining: len };
                Ok(TransferStep::Started { id, len })
            }
            (State::Body { remaining }, registry::CLIPBOARD_CHUNK) => {
                if payload.is_empty() {
                    return Err(ProtocolError::EmptyChunk);
                }
                let n = u32::try_from(payload.len()).map_err(|_| ProtocolError::TextOverrun)?;
                let remaining = remaining.checked_sub(n).ok_or(ProtocolError::TextOverrun)?;
                self.state = State::Body { remaining };
                Ok(TransferStep::Chunk(payload))
            }
            _ => Err(ProtocolError::TransferOrder { ty }),
        }
    }

    /// Called when the stream ends: the text must be complete.
    pub fn finish(&self) -> Result<(), ProtocolError> {
        match self.state {
            State::Body { remaining: 0 } => Ok(()),
            State::Body { remaining } => Err(ProtocolError::TextIncomplete { remaining }),
            State::AwaitingData => Err(ProtocolError::TextIncomplete { remaining: 0 }),
            State::Failed => Err(ProtocolError::SessionFailed),
        }
    }
}

fn short(ty: u16, payload: &[u8]) -> ProtocolError {
    ProtocolError::PayloadLength {
        ty,
        expected: DATA_LEN,
        actual: payload.len(),
    }
}

/// The collected text, if it is UTF-8.
pub fn text(bytes: &[u8]) -> Result<&str, ProtocolError> {
    std::str::from_utf8(bytes).map_err(|_| ProtocolError::TextNotUtf8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{Decoded, decode};

    /// Feeds encoded frames through a reader, collecting the text.
    fn read(bytes: &[u8]) -> Result<(u64, Vec<u8>), ProtocolError> {
        let mut reader = TransferReader::new();
        let mut rest = bytes;
        let mut id = None;
        let mut text = Vec::new();
        while let Decoded::Frame { frame, consumed } = decode(rest)? {
            match reader.on_message(frame.ty, frame.payload)? {
                TransferStep::Started { id: i, .. } => id = Some(i),
                TransferStep::Chunk(c) => text.extend_from_slice(c),
            }
            rest = &rest[consumed..];
        }
        reader.finish()?;
        Ok((id.unwrap_or_default(), text))
    }

    fn transfer(id: u64, text: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        encode_data(id, u32::try_from(text.len()).unwrap(), &mut out).unwrap();
        for chunk in text.chunks(frame::MAX_FRAME_PAYLOAD) {
            encode_chunk(chunk, &mut out).unwrap();
        }
        out
    }

    #[test]
    fn messages_round_trip() {
        for m in [
            ClipboardMessage::Offer { id: 7 },
            ClipboardMessage::Request { id: u64::MAX },
        ] {
            assert_eq!(ClipboardMessage::decode(m.ty(), &m.payload()), Ok(m));
            let mut out = Vec::new();
            m.encode(&mut out).unwrap();
            assert_eq!(out.len(), frame::HEADER_LEN + 8);
        }
    }

    #[test]
    fn fixed_layouts_are_exact() {
        for ty in [registry::CLIPBOARD_OFFER, registry::CLIPBOARD_REQUEST] {
            for len in [7, 9] {
                assert_eq!(
                    ClipboardMessage::decode(ty, &vec![0; len]),
                    Err(ProtocolError::PayloadLength {
                        ty,
                        expected: 8,
                        actual: len
                    })
                );
            }
        }
        for len in [11, 13] {
            let mut r = TransferReader::new();
            assert_eq!(
                r.on_message(registry::CLIPBOARD_DATA, &vec![0; len]),
                Err(ProtocolError::PayloadLength {
                    ty: registry::CLIPBOARD_DATA,
                    expected: 12,
                    actual: len
                })
            );
        }
    }

    #[test]
    fn transfers_round_trip() {
        assert_eq!(read(&transfer(1, b"hello")), Ok((1, b"hello".to_vec())));
        assert_eq!(read(&transfer(2, b"")), Ok((2, Vec::new())));
        let max = vec![b'x'; MAX_TEXT_LEN as usize];
        assert_eq!(read(&transfer(3, &max)), Ok((3, max)));
    }

    #[test]
    fn over_limit_is_rejected_at_the_header() {
        let mut out = Vec::new();
        encode_data(1, MAX_TEXT_LEN + 1, &mut out).unwrap();
        assert_eq!(
            read(&out),
            Err(ProtocolError::TextTooLong {
                len: MAX_TEXT_LEN + 1
            })
        );
    }

    #[test]
    fn chunks_must_total_exactly() {
        let mut extra = transfer(1, b"hi");
        encode_chunk(b"!", &mut extra).unwrap();
        assert_eq!(read(&extra), Err(ProtocolError::TextOverrun));

        let mut short = Vec::new();
        encode_data(1, 5, &mut short).unwrap();
        encode_chunk(b"hi", &mut short).unwrap();
        assert_eq!(
            read(&short),
            Err(ProtocolError::TextIncomplete { remaining: 3 })
        );

        assert_eq!(
            read(&[]),
            Err(ProtocolError::TextIncomplete { remaining: 0 })
        );
    }

    #[test]
    fn empty_chunks_and_bad_order_are_rejected() {
        let mut r = TransferReader::new();
        r.on_message(registry::CLIPBOARD_DATA, &[0; 12]).unwrap();
        assert_eq!(
            r.on_message(registry::CLIPBOARD_CHUNK, &[]),
            Err(ProtocolError::EmptyChunk)
        );
        assert!(encode_chunk(&[], &mut Vec::new()).is_err());

        let mut twice = transfer(1, b"hi");
        encode_data(1, 2, &mut twice).unwrap();
        assert_eq!(
            read(&twice),
            Err(ProtocolError::TransferOrder {
                ty: registry::CLIPBOARD_DATA
            })
        );

        let mut first_chunk = Vec::new();
        encode_chunk(b"hi", &mut first_chunk).unwrap();
        assert_eq!(
            read(&first_chunk),
            Err(ProtocolError::TransferOrder {
                ty: registry::CLIPBOARD_CHUNK
            })
        );
    }

    #[test]
    fn failed_reader_stays_failed() {
        let mut r = TransferReader::new();
        assert!(r.on_message(registry::CLIPBOARD_CHUNK, b"x").is_err());
        assert_eq!(
            r.on_message(registry::CLIPBOARD_DATA, &[0; 12]),
            Err(ProtocolError::SessionFailed)
        );
        assert_eq!(r.finish(), Err(ProtocolError::SessionFailed));
    }

    #[test]
    fn text_must_be_utf8() {
        assert_eq!(text("héllo".as_bytes()), Ok("héllo"));
        assert_eq!(text(&[0xff, 0xfe]), Err(ProtocolError::TextNotUtf8));
    }

    #[test]
    fn chunks_are_lent_not_copied() {
        let mut r = TransferReader::new();
        r.on_message(
            registry::CLIPBOARD_DATA,
            &[0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0],
        )
        .unwrap();
        let payload = *b"abc";
        let TransferStep::Chunk(c) = r.on_message(registry::CLIPBOARD_CHUNK, &payload).unwrap()
        else {
            panic!("not a chunk");
        };
        assert!(std::ptr::eq(c.as_ptr(), payload.as_ptr()));
    }
}
