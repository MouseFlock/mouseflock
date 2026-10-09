//! File runs on the agent link (IPC version 3, add-clipboard-files D5).
//!
//! A file copy's manifest travels as a clipboard run ([`crate::clipboard`]);
//! a file's contents travel as a file run of their own: a start announcing
//! the file's index and `u64` length, then file chunks carrying exactly that
//! many bytes, or a `FileAbandoned`/`AbandonFile` from the side sending the
//! bytes. A file run is separate from the clipboard run, so a copy can be
//! reported or set while a file streams. One file run at a time in each
//! direction.
//!
//! The link's queues never wait, so the bytes are windowed: the side sending
//! a file's bytes keeps at most [`FILE_WINDOW`] of them unacknowledged, and
//! the side receiving them acknowledges every byte it receives, passed on or
//! discarded. The counts are cumulative across runs, so an abandoned run
//! needs no reset. [`Window`] enforces it at the receiving end; [`Credit`]
//! keeps the sending side within it.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::ProtocolError;

/// Most bytes of a file the side sending them may have unacknowledged.
pub const FILE_WINDOW: u64 = 1_048_576;

/// Most bytes one manifest run may carry: the peer protocol's bound, from its
/// entry-count and path-length limits.
pub const MAX_MANIFEST_LEN: u32 = 51_642_372;

/// One direction's file run, at the end receiving its bytes.
#[derive(Debug, Default)]
pub struct FileRun {
    active: Option<Active>,
}

#[derive(Debug, Clone, Copy)]
struct Active {
    index: u32,
    remaining: u64,
}

impl FileRun {
    /// The run in progress: its file's index.
    pub fn active(&self) -> Option<u32> {
        self.active.map(|a| a.index)
    }

    /// Starts a run of `len` bytes for file `index`, announced by a message
    /// of type `ty`. `true` if it is complete at once (an empty file).
    pub fn start(&mut self, ty: u16, index: u32, len: u64) -> Result<bool, ProtocolError> {
        if self.active.is_some() {
            return Err(ProtocolError::ClipboardRun { ty });
        }
        if len == 0 {
            return Ok(true);
        }
        self.active = Some(Active {
            index,
            remaining: len,
        });
        Ok(false)
    }

    /// Counts a chunk of `n` bytes carried by a message of type `ty`; the
    /// file's index if this chunk completed it.
    pub fn chunk(&mut self, ty: u16, n: usize) -> Result<Option<u32>, ProtocolError> {
        let Some(active) = self.active.as_mut() else {
            return Err(ProtocolError::ClipboardRun { ty });
        };
        let n = u64::try_from(n).map_err(|_| ProtocolError::ClipboardRun { ty })?;
        if n == 0 {
            return Err(ProtocolError::InvalidField { ty });
        }
        active.remaining = active
            .remaining
            .checked_sub(n)
            .ok_or(ProtocolError::ClipboardRun { ty })?;
        if active.remaining > 0 {
            return Ok(None);
        }
        let index = active.index;
        self.active = None;
        Ok(Some(index))
    }

    /// Ends the run for file `index`, if it is the one in progress, because
    /// the side sending its bytes abandoned it. Whether a run ended.
    pub fn abandon(&mut self, index: u32) -> bool {
        if self.active.is_some_and(|a| a.index == index) {
            self.active = None;
            true
        } else {
            false
        }
    }
}

/// Bytes of file runs received and not yet acknowledged, shared between a
/// link's reader, which counts them in, and its handle, which counts the
/// acknowledgements out.
#[derive(Debug, Default)]
pub struct Window(AtomicU64);

impl Window {
    /// Counts `n` bytes received in a message of type `ty`; a protocol error
    /// if more than [`FILE_WINDOW`] are now unacknowledged.
    pub fn receive(&self, ty: u16, n: usize) -> Result<(), ProtocolError> {
        let n = u64::try_from(n).map_err(|_| ProtocolError::FileWindow { ty })?;
        let before = self.0.fetch_add(n, Ordering::AcqRel);
        if before.saturating_add(n) > FILE_WINDOW {
            return Err(ProtocolError::FileWindow { ty });
        }
        Ok(())
    }

    /// Counts `n` bytes acknowledged.
    pub fn acknowledge(&self, n: u32) {
        let _ = self
            .0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
                Some(v.saturating_sub(u64::from(n)))
            });
    }

    /// Bytes received and not yet acknowledged.
    pub fn unacknowledged(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }
}

/// The side sending a file's bytes: how many more it may send now.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Credit {
    unacknowledged: u64,
}

impl Credit {
    /// Bytes that may be sent before an acknowledgement.
    pub const fn room(&self) -> u64 {
        FILE_WINDOW.saturating_sub(self.unacknowledged)
    }

    /// Counts `n` bytes sent.
    pub fn sent(&mut self, n: usize) {
        let n = u64::try_from(n).unwrap_or(u64::MAX);
        self.unacknowledged = self.unacknowledged.saturating_add(n);
    }

    /// Counts `n` bytes the other side acknowledged.
    pub fn acknowledged(&mut self, n: u32) {
        self.unacknowledged = self.unacknowledged.saturating_sub(u64::from(n));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: u16 = 0x0042;
    const CHUNK: u16 = 0x0043;

    #[test]
    fn a_run_completes_at_its_length() {
        let mut r = FileRun::default();
        assert_eq!(r.start(START, 3, 5 << 32), Ok(false));
        assert_eq!(r.active(), Some(3));
        assert_eq!(r.chunk(CHUNK, 65_536), Ok(None));
        let mut left = (5u64 << 32) - 65_536;
        while left > 0 {
            let n = left.min(1 << 30);
            left -= n;
            let done = r.chunk(CHUNK, n as usize).unwrap();
            assert_eq!(done, (left == 0).then_some(3));
        }
        assert_eq!(r.active(), None);
        assert_eq!(r.start(START, 0, 0), Ok(true), "an empty file");
        assert_eq!(r.active(), None);
    }

    #[test]
    fn breaches_are_errors() {
        let mut r = FileRun::default();
        assert_eq!(
            r.chunk(CHUNK, 1),
            Err(ProtocolError::ClipboardRun { ty: CHUNK })
        );
        r.start(START, 1, 10).unwrap();
        assert_eq!(
            r.start(START, 2, 10),
            Err(ProtocolError::ClipboardRun { ty: START })
        );
        assert_eq!(
            r.chunk(CHUNK, 0),
            Err(ProtocolError::InvalidField { ty: CHUNK })
        );
        assert_eq!(
            r.chunk(CHUNK, 11),
            Err(ProtocolError::ClipboardRun { ty: CHUNK })
        );
    }

    #[test]
    fn abandoning_ends_only_its_own_run() {
        let mut r = FileRun::default();
        r.start(START, 1, 10).unwrap();
        assert!(!r.abandon(2));
        assert_eq!(r.active(), Some(1));
        assert!(r.abandon(1));
        assert_eq!(r.active(), None);
        assert!(!r.abandon(1));
        assert_eq!(r.start(START, 1, 10), Ok(false), "a new run after it");
    }

    #[test]
    fn the_window_bounds_unacknowledged_bytes() {
        let w = Window::default();
        w.receive(CHUNK, 1_000_000).unwrap();
        w.receive(CHUNK, (FILE_WINDOW - 1_000_000) as usize)
            .unwrap();
        assert_eq!(w.unacknowledged(), FILE_WINDOW);
        assert_eq!(
            w.receive(CHUNK, 1),
            Err(ProtocolError::FileWindow { ty: CHUNK })
        );
        let w = Window::default();
        w.receive(CHUNK, 600_000).unwrap();
        w.acknowledge(600_000);
        w.receive(CHUNK, FILE_WINDOW as usize).unwrap();
        // Stale or excess acknowledgements never go below nothing.
        w.acknowledge(u32::MAX);
        assert_eq!(w.unacknowledged(), 0);
    }

    #[test]
    fn credit_keeps_the_sender_within_the_window() {
        let mut c = Credit::default();
        assert_eq!(c.room(), FILE_WINDOW);
        c.sent(65_536 * 16);
        assert_eq!(c.room(), 0);
        c.acknowledged(65_536);
        assert_eq!(c.room(), 65_536);
        c.acknowledged(u32::MAX);
        assert_eq!(c.room(), FILE_WINDOW);
    }
}
