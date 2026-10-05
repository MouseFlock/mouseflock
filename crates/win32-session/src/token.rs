//! Sessions and the user tokens that belong to them.

use std::fmt;
use std::io;

use windows::Win32::Foundation::{ERROR_NO_TOKEN, HANDLE};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::PWSTR;

use crate::handle::{Local, Owned};
use crate::sid::Sid;

/// A Windows logon session's ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(pub u32);

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The session attached to the physical console, if any. There is none
/// briefly while the console switches between sessions.
pub fn active_console_session() -> Option<SessionId> {
    // SAFETY: no arguments; returns 0xFFFFFFFF when no session is attached.
    let id = unsafe { WTSGetActiveConsoleSessionId() };
    (id != u32::MAX).then_some(SessionId(id))
}

/// An access token, closed on drop.
#[derive(Debug)]
pub struct Token(Owned);

#[derive(Debug)]
pub enum TokenError {
    NoUser,
    /// Anything else, such as not running as SYSTEM.
    Os(io::Error),
}

impl fmt::Display for TokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoUser => f.write_str("no user is logged on to the session"),
            Self::Os(e) => write!(f, "cannot get the session user's token: {e}"),
        }
    }
}

impl std::error::Error for TokenError {}

/// The primary token of the user logged on to `session`. Needs SYSTEM
/// (`SeTcbPrivilege`).
pub fn user_token(session: SessionId) -> Result<Token, TokenError> {
    let mut token = HANDLE::default();
    // SAFETY: `token` is a valid out-pointer; on success it receives a
    // handle this function then owns.
    match unsafe { WTSQueryUserToken(session.0, &mut token) } {
        Ok(()) => Ok(Token(Owned(token))),
        Err(e) if e.code() == ERROR_NO_TOKEN.to_hresult() => Err(TokenError::NoUser),
        Err(e) => Err(TokenError::Os(e.into())),
    }
}

/// This process's own token.
pub(crate) fn own_token() -> io::Result<Token> {
    let mut token = HANDLE::default();
    // SAFETY: the pseudo-handle from `GetCurrentProcess` needs no closing;
    // `token` is a valid out-pointer whose handle this function then owns.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }?;
    Ok(Token(Owned(token)))
}

/// The user this process runs as.
pub fn own_sid() -> io::Result<Sid> {
    own_token()?.user_sid()
}

impl Token {
    pub(crate) fn handle(&self) -> HANDLE {
        self.0.0
    }

    /// The user the token belongs to.
    pub fn user_sid(&self) -> io::Result<Sid> {
        let mut len = 0u32;
        // SAFETY: a size query with no buffer; it fails with
        // ERROR_INSUFFICIENT_BUFFER and sets `len`.
        let _ = unsafe { GetTokenInformation(self.handle(), TokenUser, None, 0, &mut len) };
        // u64 elements keep the buffer aligned for TOKEN_USER's pointer.
        let mut buf = vec![0u64; (len as usize).div_ceil(8).max(1)];
        // SAFETY: `buf` holds at least `len` bytes and is 8-byte aligned.
        unsafe {
            GetTokenInformation(
                self.handle(),
                TokenUser,
                Some(buf.as_mut_ptr().cast()),
                len,
                &mut len,
            )
        }?;
        // SAFETY: on success the buffer starts with a TOKEN_USER whose SID
        // points into the same buffer, which outlives this use.
        let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let mut text = PWSTR::null();
        // SAFETY: `sid` is valid as above; `text` receives LocalAlloc memory
        // that `Local` frees.
        unsafe { ConvertSidToStringSidW(sid, &mut text) }?;
        let _free = Local(text.0.cast());
        // SAFETY: on success `text` is a NUL-terminated UTF-16 string.
        let s = unsafe { text.to_string() }.map_err(|_| io::Error::other("SID is not UTF-16"))?;
        Sid::parse(&s).ok_or_else(|| io::Error::other(format!("unexpected SID form {s:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_desktop_has_a_console_session() {
        // Tests run in a logged-on session, which is the console unless they
        // run over RDP; either way, a console session exists.
        assert!(active_console_session().is_some());
    }

    #[test]
    fn own_sid_is_a_user_sid() {
        let sid = own_sid().unwrap();
        assert!(sid.as_str().starts_with("S-1-5-"), "{sid}");
    }

    #[test]
    fn user_token_fails_cleanly_without_system() {
        // As a normal user this is access denied; as SYSTEM it may succeed.
        // Either way it returns rather than panicking.
        let _ = user_token(active_console_session().unwrap());
    }
}
