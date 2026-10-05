//! Owned kernel handles and `LocalAlloc`ed memory.

use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};

/// A kernel handle closed on drop.
#[derive(Debug)]
pub(crate) struct Owned(pub(crate) HANDLE);

// SAFETY: a kernel handle is a process-wide index, usable from any thread;
// every call made through it here is thread-safe.
unsafe impl Send for Owned {}
// SAFETY: as above; shared use only waits on or queries the handle.
unsafe impl Sync for Owned {}

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: this value owns the handle and closes it exactly once.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

/// Memory Windows allocated with `LocalAlloc`, freed on drop.
pub(crate) struct Local(pub(crate) *mut core::ffi::c_void);

impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from a Windows call documented to
            // return `LocalAlloc` memory, and is freed exactly once.
            unsafe { LocalFree(Some(HLOCAL(self.0))) };
        }
    }
}

/// `s` as a NUL-terminated UTF-16 string.
pub(crate) fn wide(s: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.as_ref().encode_wide().chain(Some(0)).collect()
}
