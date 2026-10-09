//! Clipboard messages: the offer and request on the control stream, and the
//! frames of a clipboard stream (one transfer per unidirectional stream).
//!
//! [`TransferReader`] checks a clipboard stream's order and lengths and lends
//! each chunk's bytes back; like every decoder here it buffers nothing, so the
//! caller owns the bytes it collects and can bound them by the kind's limit.
//!
//! A stream carries text (`ClipboardData`, version 1), an image as one PNG
//! (`ClipboardImageData`, version 2), a file copy's manifest
//! (`ClipboardFilesData`, version 3) or one file's contents
//! (`FileContentData`, version 3, a u64 length with no small cap). An image's
//! bytes are never decoded here: only its length and its first eight bytes,
//! the PNG signature, are checked. A manifest is checked by
//! [`crate::files::Manifest::parse`] once collected.

use crate::codec::fixed_payload;
use crate::files::{self, MAX_MANIFEST_LEN};
use crate::{EncodeError, ProtocolError, frame, registry};

/// Most bytes of UTF-8 text one transfer may carry (4 MiB).
pub const MAX_TEXT_LEN: u32 = 4_194_304;

/// Most bytes of PNG one image transfer may carry (32 MiB).
pub const MAX_IMAGE_LEN: u32 = 33_554_432;

/// The first eight bytes of every PNG file.
pub const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// What a clipboard transfer carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClipKind {
    /// UTF-8 text, at most [`MAX_TEXT_LEN`] bytes.
    Text,
    /// One PNG file, at most [`MAX_IMAGE_LEN`] bytes. Protocol version 2.
    Image,
    /// A file copy, carried as its manifest, at most [`MAX_MANIFEST_LEN`]
    /// bytes. The files' contents travel on streams of their own, one per
    /// file, when asked for. Protocol version 3.
    Files,
}

impl ClipKind {
    /// The most bytes a transfer of this kind may carry.
    pub const fn max_len(self) -> u32 {
        match self {
            Self::Text => MAX_TEXT_LEN,
            Self::Image => MAX_IMAGE_LEN,
            Self::Files => MAX_MANIFEST_LEN,
        }
    }

    /// The protocol version that introduced this kind.
    pub const fn since(self) -> u16 {
        match self {
            Self::Text => 1,
            Self::Image => 2,
            Self::Files => 3,
        }
    }
}

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

/// Appends a text clipboard stream's first frame: `ClipboardData` with `id`
/// and the text's length.
pub fn encode_data(id: u64, len: u32, out: &mut Vec<u8>) -> Result<(), EncodeError> {
    encode_header(registry::CLIPBOARD_DATA, id, len, out)
}

/// Appends an image clipboard stream's first frame: `ClipboardImageData`
/// with `id` and the PNG's length. Only for a connection at version 2 or
/// later.
pub fn encode_image_data(id: u64, len: u32, out: &mut Vec<u8>) -> Result<(), EncodeError> {
    encode_header(registry::CLIPBOARD_IMAGE_DATA, id, len, out)
}

/// Appends a manifest clipboard stream's first frame: `ClipboardFilesData`
/// with `id` and the manifest's length. Only for a connection at version 3
/// or later.
pub fn encode_files_data(id: u64, len: u32, out: &mut Vec<u8>) -> Result<(), EncodeError> {
    encode_header(registry::CLIPBOARD_FILES_DATA, id, len, out)
}

/// Appends the header frame for a transfer of `kind`.
pub fn encode_header_for(
    kind: ClipKind,
    id: u64,
    len: u32,
    out: &mut Vec<u8>,
) -> Result<(), EncodeError> {
    match kind {
        ClipKind::Text => encode_data(id, len, out),
        ClipKind::Image => encode_image_data(id, len, out),
        ClipKind::Files => encode_files_data(id, len, out),
    }
}

fn encode_header(ty: u16, id: u64, len: u32, out: &mut Vec<u8>) -> Result<(), EncodeError> {
    let mut payload = [0u8; DATA_LEN];
    let (id_bytes, len_bytes) = payload.split_at_mut(8);
    id_bytes.copy_from_slice(&id.to_le_bytes());
    len_bytes.copy_from_slice(&len.to_le_bytes());
    frame::encode(ty, &payload, out)
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
    /// The transfer's header: which copy, what it carries, and how many bytes
    /// follow.
    Started { id: u64, kind: ClipKind, len: u32 },
    /// A file content stream's header: which copy, which file of its
    /// manifest, and how many bytes follow. Version 3.
    FileStarted { id: u64, index: u32, len: u64 },
    /// More of the copy's bytes.
    Chunk(&'a [u8]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    AwaitingData,
    /// `signed` counts the leading bytes already checked against the PNG
    /// signature; text starts at 8, so nothing is checked.
    Body {
        remaining: u64,
        signed: usize,
    },
    Failed,
}

/// Checks the frames of one clipboard stream: exactly one header first
/// (`ClipboardData`; from version 2 `ClipboardImageData`; from version 3
/// `ClipboardFilesData` or `FileContentData`), then non-empty chunks
/// totalling exactly its length, within the kind's limit. An image's first
/// eight bytes must be the PNG signature, wherever the chunks split.
#[derive(Debug, Clone)]
pub struct TransferReader {
    state: State,
    version: u16,
}

impl TransferReader {
    /// A reader for a stream on a connection that agreed `version`.
    pub const fn new(version: u16) -> Self {
        Self {
            state: State::AwaitingData,
            version,
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
            (State::AwaitingData, registry::FILE_CONTENT_DATA) => {
                if self.version < ClipKind::Files.since() {
                    return Err(ProtocolError::UnknownType { ty });
                }
                let (id, index, len) = files::decode_content_data(payload)?;
                self.state = State::Body {
                    remaining: len,
                    signed: PNG_SIGNATURE.len(),
                };
                Ok(TransferStep::FileStarted { id, index, len })
            }
            (
                State::AwaitingData,
                registry::CLIPBOARD_DATA
                | registry::CLIPBOARD_IMAGE_DATA
                | registry::CLIPBOARD_FILES_DATA,
            ) => {
                let kind = match ty {
                    registry::CLIPBOARD_DATA => ClipKind::Text,
                    registry::CLIPBOARD_IMAGE_DATA => ClipKind::Image,
                    _ => ClipKind::Files,
                };
                if self.version < kind.since() {
                    return Err(ProtocolError::UnknownType { ty });
                }
                let p = fixed_payload::<DATA_LEN>(ty, payload)?;
                let (Some(id), Some(len)) = (p.first_chunk::<8>(), p.last_chunk::<4>()) else {
                    return Err(short(ty, payload));
                };
                let (id, len) = (u64::from_le_bytes(*id), u32::from_le_bytes(*len));
                let signed = match kind {
                    ClipKind::Text if len > MAX_TEXT_LEN => {
                        return Err(ProtocolError::TextTooLong { len });
                    }
                    ClipKind::Text => PNG_SIGNATURE.len(),
                    ClipKind::Image if len > MAX_IMAGE_LEN => {
                        return Err(ProtocolError::ImageTooLong { len });
                    }
                    // Too short to hold the signature, so it cannot be a PNG.
                    ClipKind::Image if (len as usize) < PNG_SIGNATURE.len() => {
                        return Err(ProtocolError::NotPng);
                    }
                    ClipKind::Image => 0,
                    ClipKind::Files if len > MAX_MANIFEST_LEN => {
                        return Err(ProtocolError::ManifestTooLong { len });
                    }
                    ClipKind::Files => PNG_SIGNATURE.len(),
                };
                self.state = State::Body {
                    remaining: u64::from(len),
                    signed,
                };
                Ok(TransferStep::Started { id, kind, len })
            }
            (State::Body { remaining, signed }, registry::CLIPBOARD_CHUNK) => {
                if payload.is_empty() {
                    return Err(ProtocolError::EmptyChunk);
                }
                let n = u64::try_from(payload.len()).map_err(|_| ProtocolError::TextOverrun)?;
                let remaining = remaining.checked_sub(n).ok_or(ProtocolError::TextOverrun)?;
                let signed = check_signature(signed, payload)?;
                self.state = State::Body { remaining, signed };
                Ok(TransferStep::Chunk(payload))
            }
            _ => Err(ProtocolError::TransferOrder { ty }),
        }
    }

    /// Called when the stream ends: the copy must be complete.
    pub fn finish(&self) -> Result<(), ProtocolError> {
        match self.state {
            State::Body { remaining: 0, .. } => Ok(()),
            State::Body { remaining, .. } => Err(ProtocolError::TextIncomplete { remaining }),
            State::AwaitingData => Err(ProtocolError::TextIncomplete { remaining: 0 }),
            State::Failed => Err(ProtocolError::SessionFailed),
        }
    }
}

/// Checks the part of `chunk` that falls within the PNG signature, given that
/// `signed` leading bytes were already checked; returns the new count.
fn check_signature(signed: usize, chunk: &[u8]) -> Result<usize, ProtocolError> {
    let Some(expected) = PNG_SIGNATURE.get(signed..) else {
        return Ok(signed);
    };
    let n = expected.len().min(chunk.len());
    if chunk.get(..n) != expected.get(..n) {
        return Err(ProtocolError::NotPng);
    }
    // `n` is at most the signature's remaining length, so this stays ≤ 8.
    Ok(signed.saturating_add(n))
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
        let mut reader = TransferReader::new(2);
        let mut rest = bytes;
        let mut id = None;
        let mut text = Vec::new();
        while let Decoded::Frame { frame, consumed } = decode(rest)? {
            match reader.on_message(frame.ty, frame.payload)? {
                TransferStep::Started { id: i, .. } | TransferStep::FileStarted { id: i, .. } => {
                    id = Some(i);
                }
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

    /// Like `read`, at a given version, also reporting the kind.
    fn read_at(version: u16, bytes: &[u8]) -> Result<(u64, ClipKind, Vec<u8>), ProtocolError> {
        let mut reader = TransferReader::new(version);
        let mut rest = bytes;
        let mut head = None;
        let mut body = Vec::new();
        while let Decoded::Frame { frame, consumed } = decode(rest)? {
            match reader.on_message(frame.ty, frame.payload)? {
                TransferStep::Started { id, kind, .. } => head = Some((id, kind)),
                TransferStep::FileStarted { .. } => panic!("a file content header"),
                TransferStep::Chunk(c) => body.extend_from_slice(c),
            }
            rest = &rest[consumed..];
        }
        reader.finish()?;
        let (id, kind) = head.expect("a header");
        Ok((id, kind, body))
    }

    #[test]
    fn manifest_round_trips_at_version_3() {
        use crate::files::{FileEntry, Manifest, encode_manifest};
        let entries = [
            FileEntry {
                path: "d",
                is_dir: true,
                size: 0,
            },
            FileEntry {
                path: "d/f.txt",
                is_dir: false,
                size: 9,
            },
        ];
        let mut manifest = Vec::new();
        encode_manifest(entries, &mut manifest).unwrap();
        let mut out = Vec::new();
        encode_files_data(4, u32::try_from(manifest.len()).unwrap(), &mut out).unwrap();
        for chunk in manifest.chunks(5) {
            encode_chunk(chunk, &mut out).unwrap();
        }
        let (id, kind, body) = read_at(3, &out).unwrap();
        assert_eq!((id, kind), (4, ClipKind::Files));
        let m = Manifest::parse(&body).unwrap();
        assert_eq!(m.entries().collect::<Vec<_>>(), entries);
    }

    #[test]
    fn file_headers_need_version_3() {
        for version in [1, 2] {
            let mut r = TransferReader::new(version);
            assert_eq!(
                r.on_message(registry::CLIPBOARD_FILES_DATA, &[0; 12]),
                Err(ProtocolError::UnknownType {
                    ty: registry::CLIPBOARD_FILES_DATA
                })
            );
            let mut r = TransferReader::new(version);
            assert_eq!(
                r.on_message(registry::FILE_CONTENT_DATA, &[0; 20]),
                Err(ProtocolError::UnknownType {
                    ty: registry::FILE_CONTENT_DATA
                })
            );
        }
    }

    #[test]
    fn manifest_over_its_bound_is_refused_at_the_header() {
        let mut out = Vec::new();
        encode_files_data(1, MAX_MANIFEST_LEN + 1, &mut out).unwrap();
        assert_eq!(
            read_at(3, &out),
            Err(ProtocolError::ManifestTooLong {
                len: MAX_MANIFEST_LEN + 1
            })
        );
        let mut r = TransferReader::new(3);
        let mut at_bound = Vec::new();
        encode_files_data(1, MAX_MANIFEST_LEN, &mut at_bound).unwrap();
        let frame = match decode(&at_bound).unwrap() {
            Decoded::Frame { frame, .. } => frame,
            Decoded::NeedMore => panic!("a whole frame"),
        };
        assert!(r.on_message(frame.ty, frame.payload).is_ok());
    }

    /// Feeds the frames in `bytes` to a version 3 reader.
    fn feed(reader: &mut TransferReader, bytes: &[u8]) -> Result<Vec<u8>, ProtocolError> {
        let mut rest = bytes;
        let mut body = Vec::new();
        while let Decoded::Frame { frame, consumed } = decode(rest)? {
            if let TransferStep::Chunk(c) = reader.on_message(frame.ty, frame.payload)? {
                body.extend_from_slice(c);
            }
            rest = &rest[consumed..];
        }
        Ok(body)
    }

    #[test]
    fn file_content_has_a_u64_length_and_no_small_cap() {
        let five_gib: u64 = 5 << 30;
        let mut out = Vec::new();
        files::encode_content_data(7, 2, five_gib, &mut out).unwrap();
        let mut r = TransferReader::new(3);
        let frame = match decode(&out).unwrap() {
            Decoded::Frame { frame, .. } => frame,
            Decoded::NeedMore => panic!("a whole frame"),
        };
        assert_eq!(
            r.on_message(frame.ty, frame.payload),
            Ok(TransferStep::FileStarted {
                id: 7,
                index: 2,
                len: five_gib
            })
        );
        // Bytes past 4 GiB of header length are counted in a u64: the
        // remainder after one chunk is still over u32::MAX.
        let chunk = [0x55; 1000];
        assert_eq!(
            r.on_message(registry::CLIPBOARD_CHUNK, &chunk),
            Ok(TransferStep::Chunk(&chunk))
        );
        assert_eq!(
            r.finish(),
            Err(ProtocolError::TextIncomplete {
                remaining: five_gib - 1000
            })
        );
    }

    #[test]
    fn file_content_must_total_exactly() {
        let mut exact = Vec::new();
        files::encode_content_data(1, 0, 5, &mut exact).unwrap();
        encode_chunk(b"hel", &mut exact).unwrap();
        encode_chunk(b"lo", &mut exact).unwrap();
        let mut r = TransferReader::new(3);
        assert_eq!(feed(&mut r, &exact), Ok(b"hello".to_vec()));
        assert_eq!(r.finish(), Ok(()));

        let mut over = exact.clone();
        encode_chunk(b"!", &mut over).unwrap();
        let mut r = TransferReader::new(3);
        assert_eq!(feed(&mut r, &over), Err(ProtocolError::TextOverrun));

        // A second header on the same stream is out of order.
        let mut twice = exact;
        files::encode_content_data(1, 1, 1, &mut twice).unwrap();
        let mut r = TransferReader::new(3);
        assert_eq!(
            feed(&mut r, &twice),
            Err(ProtocolError::TransferOrder {
                ty: registry::FILE_CONTENT_DATA
            })
        );
        // An empty file is a header and nothing else.
        let mut empty = Vec::new();
        files::encode_content_data(1, 0, 0, &mut empty).unwrap();
        let mut r = TransferReader::new(3);
        assert_eq!(feed(&mut r, &empty), Ok(Vec::new()));
        assert_eq!(r.finish(), Ok(()));
    }

    /// An image stream carrying `bytes`, split into chunks of `chunk` bytes.
    fn image_transfer(id: u64, bytes: &[u8], chunk: usize) -> Vec<u8> {
        let mut out = Vec::new();
        encode_image_data(id, u32::try_from(bytes.len()).unwrap(), &mut out).unwrap();
        for piece in bytes.chunks(chunk) {
            encode_chunk(piece, &mut out).unwrap();
        }
        out
    }

    /// `len` bytes that start with the PNG signature.
    fn png_like(len: usize) -> Vec<u8> {
        let mut bytes = vec![0x42; len];
        bytes[..8].copy_from_slice(&PNG_SIGNATURE);
        bytes
    }

    #[test]
    fn image_round_trips_at_version_2() {
        let png = png_like(100_000);
        assert_eq!(
            read_at(2, &image_transfer(9, &png, frame::MAX_FRAME_PAYLOAD)),
            Ok((9, ClipKind::Image, png))
        );
    }

    #[test]
    fn image_of_exactly_the_limit_is_accepted() {
        let png = png_like(MAX_IMAGE_LEN as usize);
        let (_, kind, body) =
            read_at(2, &image_transfer(1, &png, frame::MAX_FRAME_PAYLOAD)).unwrap();
        assert_eq!(kind, ClipKind::Image);
        assert_eq!(body.len(), MAX_IMAGE_LEN as usize);
    }

    #[test]
    fn image_over_the_limit_is_refused_at_the_header() {
        let mut out = Vec::new();
        encode_image_data(1, 40_000_000, &mut out).unwrap();
        assert_eq!(
            read_at(2, &out),
            Err(ProtocolError::ImageTooLong { len: 40_000_000 })
        );
    }

    #[test]
    fn image_header_at_version_1_is_an_unknown_type() {
        let png = png_like(16);
        assert_eq!(
            read_at(1, &image_transfer(1, &png, 16)),
            Err(ProtocolError::UnknownType {
                ty: registry::CLIPBOARD_IMAGE_DATA
            })
        );
    }

    #[test]
    fn image_must_start_with_the_png_signature() {
        let mut bad = png_like(16);
        bad[3] = b'X';
        assert_eq!(
            read_at(2, &image_transfer(1, &bad, 16)),
            Err(ProtocolError::NotPng)
        );
        // Split 3 + 5 across two chunks, then the rest: still checked.
        assert_eq!(
            read_at(2, &image_transfer(1, &bad, 3)),
            Err(ProtocolError::NotPng)
        );
        let mut late = png_like(16);
        late[6] = 0;
        assert_eq!(
            read_at(2, &image_transfer(1, &late, 3)),
            Err(ProtocolError::NotPng)
        );
        // A good signature split the same way is accepted.
        let good = png_like(16);
        assert_eq!(
            read_at(2, &image_transfer(4, &good, 3)),
            Ok((4, ClipKind::Image, good))
        );
    }

    #[test]
    fn image_too_short_for_a_signature_is_not_a_png() {
        let mut out = Vec::new();
        encode_image_data(1, 7, &mut out).unwrap();
        assert_eq!(read_at(2, &out), Err(ProtocolError::NotPng));
    }

    #[test]
    fn text_is_unchanged_at_either_version() {
        for version in [1, 2] {
            assert_eq!(
                read_at(version, &transfer(5, b"hello")),
                Ok((5, ClipKind::Text, b"hello".to_vec()))
            );
        }
        // Text bytes are never checked against the PNG signature.
        assert!(read_at(2, &transfer(5, &[0; 16])).is_ok());
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
            let mut r = TransferReader::new(2);
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
        let mut r = TransferReader::new(2);
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
        let mut r = TransferReader::new(2);
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
        let mut r = TransferReader::new(2);
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
