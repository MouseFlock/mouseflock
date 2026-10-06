//! Clipboard text on the agent link (add-text-clipboard design D4).
//!
//! Text travels as a run: `ClipboardText` or `SetClipboard` announcing its
//! length, then chunks carrying exactly that many bytes. Other messages may
//! come between the chunks. [`RunAssembler`] enforces the run rules at the
//! receiving end and hands back the text once it is complete.

use crate::ProtocolError;
use crate::frame::MAX_PAYLOAD;

/// Most bytes of UTF-8 text one run may carry (4 MiB).
pub const MAX_TEXT_LEN: u32 = 4_194_304;

/// Why the agent is not sharing the newest copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotShared {
    /// Marked by the copying app to stay out of clipboard monitoring,
    /// history or cloud sync.
    Private,
    /// No text format on the clipboard.
    NoText,
    /// Text over [`MAX_TEXT_LEN`] bytes of UTF-8.
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
    NotShared(NotShared),
}

/// `text` split into chunk payloads of at most one frame each. Empty text
/// has no chunks.
pub fn chunks(text: &str) -> std::slice::Chunks<'_, u8> {
    text.as_bytes().chunks(MAX_PAYLOAD)
}

/// Collects one run at a time at the receiving end.
#[derive(Debug, Default)]
pub struct RunAssembler {
    /// Bytes still to come, and the text so far.
    run: Option<(u32, Vec<u8>)>,
}

impl RunAssembler {
    /// Whether a run has started and not yet completed.
    pub const fn in_run(&self) -> bool {
        self.run.is_some()
    }

    /// Starts a run of `len` bytes, announced by a message of type `ty`.
    /// Empty text completes at once.
    pub fn start(&mut self, ty: u16, len: u32) -> Result<Option<String>, ProtocolError> {
        if self.in_run() {
            return Err(ProtocolError::ClipboardRun { ty });
        }
        if len > MAX_TEXT_LEN {
            return Err(ProtocolError::InvalidField { ty });
        }
        if len == 0 {
            return Ok(Some(String::new()));
        }
        self.run = Some((len, Vec::new()));
        Ok(None)
    }

    /// Adds a chunk carried by a message of type `ty`, and returns the text
    /// if this chunk completed it.
    pub fn chunk(&mut self, ty: u16, bytes: &[u8]) -> Result<Option<String>, ProtocolError> {
        let Some((remaining, text)) = self.run.as_mut() else {
            return Err(ProtocolError::ClipboardRun { ty });
        };
        let n = u32::try_from(bytes.len()).map_err(|_| ProtocolError::ClipboardRun { ty })?;
        if n == 0 {
            return Err(ProtocolError::InvalidField { ty });
        }
        *remaining = remaining
            .checked_sub(n)
            .ok_or(ProtocolError::ClipboardRun { ty })?;
        text.extend_from_slice(bytes);
        if *remaining > 0 {
            return Ok(None);
        }
        let Some((_, text)) = self.run.take() else {
            return Err(ProtocolError::ClipboardRun { ty });
        };
        String::from_utf8(text)
            .map(Some)
            .map_err(|_| ProtocolError::InvalidField { ty })
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

    fn run(a: &mut RunAssembler, text: &str) -> Result<Option<String>, ProtocolError> {
        let mut done = a.start(START, u32::try_from(text.len()).unwrap())?;
        for c in chunks(text) {
            done = a.chunk(CHUNK, c)?;
        }
        Ok(done)
    }

    #[test]
    fn runs_complete_with_their_text() {
        let mut a = RunAssembler::default();
        assert_eq!(run(&mut a, "héllo"), Ok(Some("héllo".into())));
        assert_eq!(run(&mut a, ""), Ok(Some(String::new())));
        let max = "x".repeat(MAX_TEXT_LEN as usize);
        assert_eq!(run(&mut a, &max), Ok(Some(max)));
        assert!(!a.in_run());
    }

    #[test]
    fn breaches_are_errors() {
        let mut a = RunAssembler::default();
        assert_eq!(
            a.chunk(CHUNK, b"x"),
            Err(ProtocolError::ClipboardRun { ty: CHUNK })
        );
        assert_eq!(
            a.start(START, MAX_TEXT_LEN + 1),
            Err(ProtocolError::InvalidField { ty: START })
        );
        assert_eq!(a.start(START, 3), Ok(None));
        assert_eq!(
            a.chunk(CHUNK, b""),
            Err(ProtocolError::InvalidField { ty: CHUNK })
        );
        assert_eq!(
            a.start(START, 3),
            Err(ProtocolError::ClipboardRun { ty: START })
        );
        assert_eq!(
            a.not_in_run(0x0018),
            Err(ProtocolError::ClipboardRun { ty: 0x0018 })
        );

        let mut b = RunAssembler::default();
        b.start(START, 2).unwrap();
        assert_eq!(
            b.chunk(CHUNK, b"abc"),
            Err(ProtocolError::ClipboardRun { ty: CHUNK })
        );

        let mut c = RunAssembler::default();
        c.start(START, 2).unwrap();
        assert_eq!(
            c.chunk(CHUNK, &[0xff, 0xfe]),
            Err(ProtocolError::InvalidField { ty: CHUNK })
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
