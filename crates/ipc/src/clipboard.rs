//! Clipboard copies on the agent link (add-text-clipboard design D4,
//! add-clipboard-images D7).
//!
//! A copy travels as a run: a start message announcing its length
//! (`ClipboardText` or `SetClipboard` for text; from IPC version 2,
//! `ClipboardImage` or `SetClipboardImage` for an image as one PNG; from
//! version 3, `ClipboardFiles` or `SetClipboardFiles` for a file copy's
//! manifest, checked here only for its length), then
//! chunks carrying exactly that many bytes. Other messages may come between
//! the chunks. [`RunAssembler`] enforces the run rules at the receiving end
//! and hands back the copy once it is complete. An image's bytes are checked
//! only for their length and the PNG signature, never decoded.

use crate::ProtocolError;
use crate::files::MAX_MANIFEST_LEN;
use crate::frame::MAX_PAYLOAD;

/// Most bytes of UTF-8 text one run may carry (4 MiB).
pub const MAX_TEXT_LEN: u32 = 4_194_304;

/// Most bytes of PNG one image run may carry (32 MiB).
pub const MAX_IMAGE_LEN: u32 = 33_554_432;

/// The first eight bytes of every PNG file.
pub const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// What a run carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    /// One PNG file. IPC version 2.
    Image,
    /// A file copy's manifest. IPC version 3.
    Files,
}

impl Kind {
    pub const fn max_len(self) -> u32 {
        match self {
            Self::Text => MAX_TEXT_LEN,
            Self::Image => MAX_IMAGE_LEN,
            Self::Files => MAX_MANIFEST_LEN,
        }
    }
}

/// Why the agent is not sharing the newest copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotShared {
    /// Marked by the copying app to stay out of clipboard monitoring,
    /// history or cloud sync.
    Private,
    /// Neither text, an image nor files the agent can share; at IPC version
    /// 1, any copy without text, and at version 2 any without text or an
    /// image.
    NoText,
    /// Text over [`MAX_TEXT_LEN`] bytes of UTF-8, an image over the image
    /// limits, or files over the file copy limits.
    TooLarge,
}

impl NotShared {
    pub const fn code(self) -> u8 {
        match self {
            Self::Private => 0,
            Self::NoText => 1,
            Self::TooLarge => 2,
        }
    }

    pub const fn from_code(ty: u16, code: u8) -> Result<Self, ProtocolError> {
        match code {
            0 => Ok(Self::Private),
            1 => Ok(Self::NoText),
            2 => Ok(Self::TooLarge),
            value => Err(ProtocolError::InvalidCode { ty, value }),
        }
    }
}

/// What the agent reports about a copy, once its run is complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Copied {
    Text(String),
    /// One PNG file, unparsed.
    Image(Vec<u8>),
    NotShared(NotShared),
}

/// A complete run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Run {
    Text(String),
    /// One PNG file, checked only for its signature and length.
    Image(Vec<u8>),
    /// A file copy's manifest, checked only for its length.
    Files(Vec<u8>),
}

/// `bytes` split into chunk payloads of at most one frame each. Nothing has
/// no chunks.
pub fn chunks(bytes: &[u8]) -> std::slice::Chunks<'_, u8> {
    bytes.chunks(MAX_PAYLOAD)
}

/// A run in progress.
#[derive(Debug)]
struct Partial {
    kind: Kind,
    remaining: u32,
    bytes: Vec<u8>,
}

/// Collects one run at a time at the receiving end.
#[derive(Debug, Default)]
pub struct RunAssembler {
    run: Option<Partial>,
}

impl RunAssembler {
    /// Whether a run has started and not yet completed.
    pub const fn in_run(&self) -> bool {
        self.run.is_some()
    }

    /// Starts a run of `len` bytes of `kind`, announced by a message of type
    /// `ty`. Empty text completes at once; an image too short to hold the
    /// PNG signature is a breach.
    pub fn start(&mut self, ty: u16, kind: Kind, len: u32) -> Result<Option<Run>, ProtocolError> {
        if self.in_run() {
            return Err(ProtocolError::ClipboardRun { ty });
        }
        if len > kind.max_len() {
            return Err(ProtocolError::InvalidField { ty });
        }
        match kind {
            Kind::Text if len == 0 => return Ok(Some(Run::Text(String::new()))),
            Kind::Image if (len as usize) < PNG_SIGNATURE.len() => {
                return Err(ProtocolError::InvalidField { ty });
            }
            _ => {}
        }
        self.run = Some(Partial {
            kind,
            remaining: len,
            bytes: Vec::new(),
        });
        Ok(None)
    }

    /// Adds a chunk carried by a message of type `ty`, and returns the copy
    /// if this chunk completed it.
    pub fn chunk(&mut self, ty: u16, bytes: &[u8]) -> Result<Option<Run>, ProtocolError> {
        let Some(run) = self.run.as_mut() else {
            return Err(ProtocolError::ClipboardRun { ty });
        };
        let n = u32::try_from(bytes.len()).map_err(|_| ProtocolError::ClipboardRun { ty })?;
        if n == 0 {
            return Err(ProtocolError::InvalidField { ty });
        }
        run.remaining = run
            .remaining
            .checked_sub(n)
            .ok_or(ProtocolError::ClipboardRun { ty })?;
        run.bytes.extend_from_slice(bytes);
        // The signature is checked as soon as its bytes are in, wherever the
        // chunks split it.
        if run.kind == Kind::Image {
            let have = run.bytes.len().min(PNG_SIGNATURE.len());
            if run.bytes.get(..have) != PNG_SIGNATURE.get(..have) {
                return Err(ProtocolError::InvalidField { ty });
            }
        }
        if run.remaining > 0 {
            return Ok(None);
        }
        let Some(done) = self.run.take() else {
            return Err(ProtocolError::ClipboardRun { ty });
        };
        match done.kind {
            Kind::Text => String::from_utf8(done.bytes)
                .map(|t| Some(Run::Text(t)))
                .map_err(|_| ProtocolError::InvalidField { ty }),
            Kind::Image => Ok(Some(Run::Image(done.bytes))),
            Kind::Files => Ok(Some(Run::Files(done.bytes))),
        }
    }

    /// Fails if a run is in progress: another clipboard message of type `ty`
    /// would cut it short.
    pub const fn not_in_run(&self, ty: u16) -> Result<(), ProtocolError> {
        if self.in_run() {
            Err(ProtocolError::ClipboardRun { ty })
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: u16 = 0x0016;
    const CHUNK: u16 = 0x0017;
    const IMAGE: u16 = 0x001a;

    fn run(a: &mut RunAssembler, text: &str) -> Result<Option<Run>, ProtocolError> {
        let mut done = a.start(START, Kind::Text, u32::try_from(text.len()).unwrap())?;
        for c in chunks(text.as_bytes()) {
            done = a.chunk(CHUNK, c)?;
        }
        Ok(done)
    }

    fn image_run(
        a: &mut RunAssembler,
        png: &[u8],
        split: usize,
    ) -> Result<Option<Run>, ProtocolError> {
        let mut done = a.start(IMAGE, Kind::Image, u32::try_from(png.len()).unwrap())?;
        for c in png.chunks(split) {
            done = a.chunk(CHUNK, c)?;
        }
        Ok(done)
    }

    fn png_like(len: usize) -> Vec<u8> {
        let mut bytes = vec![0x42; len];
        bytes[..8].copy_from_slice(&PNG_SIGNATURE);
        bytes
    }

    #[test]
    fn runs_complete_with_their_text() {
        let mut a = RunAssembler::default();
        assert_eq!(run(&mut a, "héllo"), Ok(Some(Run::Text("héllo".into()))));
        assert_eq!(run(&mut a, ""), Ok(Some(Run::Text(String::new()))));
        let max = "x".repeat(MAX_TEXT_LEN as usize);
        assert_eq!(run(&mut a, &max), Ok(Some(Run::Text(max))));
        assert!(!a.in_run());
    }

    #[test]
    fn image_runs_complete_with_their_bytes() {
        let mut a = RunAssembler::default();
        let png = png_like(200_000);
        assert_eq!(
            image_run(&mut a, &png, MAX_PAYLOAD),
            Ok(Some(Run::Image(png)))
        );
        let max = png_like(MAX_IMAGE_LEN as usize);
        assert_eq!(
            image_run(&mut a, &max, MAX_PAYLOAD),
            Ok(Some(Run::Image(max)))
        );
        assert!(!a.in_run());
    }

    #[test]
    fn image_breaches_are_errors() {
        let mut a = RunAssembler::default();
        assert_eq!(
            a.start(IMAGE, Kind::Image, MAX_IMAGE_LEN + 1),
            Err(ProtocolError::InvalidField { ty: IMAGE })
        );
        assert_eq!(
            a.start(IMAGE, Kind::Image, 7),
            Err(ProtocolError::InvalidField { ty: IMAGE })
        );
        assert_eq!(
            a.start(IMAGE, Kind::Image, 0),
            Err(ProtocolError::InvalidField { ty: IMAGE })
        );
        // A wrong signature, whole or split 3 + 5.
        for split in [16, 3] {
            let mut bad = png_like(16);
            bad[5] = 0;
            let mut b = RunAssembler::default();
            assert_eq!(
                image_run(&mut b, &bad, split),
                Err(ProtocolError::InvalidField { ty: CHUNK }),
                "split {split}"
            );
        }
        // Text is never checked against the signature.
        let mut c = RunAssembler::default();
        assert!(run(&mut c, "\u{0}abcdefgh").is_ok());
    }

    #[test]
    fn breaches_are_errors() {
        let mut a = RunAssembler::default();
        assert_eq!(
            a.chunk(CHUNK, b"x"),
            Err(ProtocolError::ClipboardRun { ty: CHUNK })
        );
        assert_eq!(
            a.start(START, Kind::Text, MAX_TEXT_LEN + 1),
            Err(ProtocolError::InvalidField { ty: START })
        );
        assert_eq!(a.start(START, Kind::Text, 3), Ok(None));
        assert_eq!(
            a.chunk(CHUNK, b""),
            Err(ProtocolError::InvalidField { ty: CHUNK })
        );
        assert_eq!(
            a.start(START, Kind::Text, 3),
            Err(ProtocolError::ClipboardRun { ty: START })
        );
        assert_eq!(
            a.not_in_run(0x0018),
            Err(ProtocolError::ClipboardRun { ty: 0x0018 })
        );

        let mut b = RunAssembler::default();
        b.start(START, Kind::Text, 2).unwrap();
        assert_eq!(
            b.chunk(CHUNK, b"abc"),
            Err(ProtocolError::ClipboardRun { ty: CHUNK })
        );

        let mut c = RunAssembler::default();
        c.start(START, Kind::Text, 2).unwrap();
        assert_eq!(
            c.chunk(CHUNK, &[0xff, 0xfe]),
            Err(ProtocolError::InvalidField { ty: CHUNK })
        );
    }

    #[test]
    fn manifest_runs_are_bounded_by_length_only() {
        let mut a = RunAssembler::default();
        assert_eq!(
            a.start(0x0040, Kind::Files, MAX_MANIFEST_LEN + 1),
            Err(ProtocolError::InvalidField { ty: 0x0040 })
        );
        assert_eq!(a.start(0x0040, Kind::Files, 3), Ok(None));
        assert_eq!(
            a.chunk(CHUNK, b"\xff\0x"),
            Ok(Some(Run::Files(b"\xff\0x".to_vec())))
        );
    }

    #[test]
    fn reasons_round_trip() {
        for r in [NotShared::Private, NotShared::NoText, NotShared::TooLarge] {
            assert_eq!(NotShared::from_code(0x18, r.code()), Ok(r));
        }
        assert_eq!(
            NotShared::from_code(0x18, 3),
            Err(ProtocolError::InvalidCode { ty: 0x18, value: 3 })
        );
    }
}
