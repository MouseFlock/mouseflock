//! Sessions, the user tokens that belong to them, and the winlogon agent's
//! restricted SYSTEM token.

use std::fmt;
use std::io;

use windows::Win32::Foundation::{ERROR_NO_TOKEN, HANDLE, LUID};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{
    CreateRestrictedToken, DISABLE_MAX_PRIVILEGE, GetTokenInformation, LUID_AND_ATTRIBUTES,
    LookupPrivilegeNameW, SetTokenInformation, TOKEN_ACCESS_MASK, TOKEN_ADJUST_DEFAULT,
    TOKEN_ADJUST_SESSIONID, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_INFORMATION_CLASS,
    TOKEN_PRIVILEGES, TOKEN_QUERY, TOKEN_USER, TokenPrivileges, TokenSessionId, TokenUser,
};
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
    open_own_token(TOKEN_QUERY)
}

fn open_own_token(access: TOKEN_ACCESS_MASK) -> io::Result<Token> {
    let mut token = HANDLE::default();
    // SAFETY: the pseudo-handle from `GetCurrentProcess` needs no closing;
    // `token` is a valid out-pointer whose handle this function then owns.
    unsafe { OpenProcessToken(GetCurrentProcess(), access, &mut token) }?;
    Ok(Token(Owned(token)))
}

/// A primary copy of this process's token with every privilege removed
/// except `SeChangeNotifyPrivilege`. Its handle may also set the session.
fn restricted_own_token() -> io::Result<Token> {
    let own = open_own_token(
        TOKEN_DUPLICATE
            | TOKEN_QUERY
            | TOKEN_ASSIGN_PRIMARY
            | TOKEN_ADJUST_DEFAULT
            | TOKEN_ADJUST_SESSIONID,
    )?;
    let mut token = HANDLE::default();
    // SAFETY: `own` is open for the call; no SIDs or privileges are passed
    // by list; `token` is a valid out-pointer whose handle this function
    // then owns. The new handle has the same access as `own`'s.
    unsafe {
        CreateRestrictedToken(
            own.handle(),
            DISABLE_MAX_PRIVILEGE,
            None,
            None,
            None,
            &mut token,
        )
    }?;
    Ok(Token(Owned(token)))
}

/// The winlogon agent's token: this process's (SYSTEM's) token with every
/// privilege removed except `SeChangeNotifyPrivilege`, moved to `session`.
/// It keeps System integrity, which injecting into the secure desktop
/// needs. Moving it needs `SeTcbPrivilege`, so SYSTEM.
pub fn system_token(session: SessionId) -> io::Result<Token> {
    let token = restricted_own_token()?;
    let id = session.0;
    // SAFETY: the handle is open with TOKEN_ADJUST_SESSIONID; the buffer is
    // the u32 session ID TokenSessionId expects, and outlives the call.
    unsafe {
        SetTokenInformation(
            token.handle(),
            TokenSessionId,
            (&raw const id).cast(),
            size_of::<u32>() as u32,
        )
    }?;
    Ok(token)
}

/// The user this process runs as.
pub fn own_sid() -> io::Result<Sid> {
    own_token()?.user_sid()
}

impl Token {
    pub(crate) fn handle(&self) -> HANDLE {
        self.0.0
    }

    /// The token information of `class`, in an 8-byte aligned buffer.
    fn information(&self, class: TOKEN_INFORMATION_CLASS) -> io::Result<Vec<u64>> {
        let mut len = 0u32;
        // SAFETY: a size query with no buffer; it fails with
        // ERROR_INSUFFICIENT_BUFFER and sets `len`.
        let _ = unsafe { GetTokenInformation(self.handle(), class, None, 0, &mut len) };
        // u64 elements keep the buffer aligned for the structures' pointers.
        let mut buf = vec![0u64; (len as usize).div_ceil(8).max(1)];
        // SAFETY: `buf` holds at least `len` bytes and is 8-byte aligned.
        unsafe {
            GetTokenInformation(
                self.handle(),
                class,
                Some(buf.as_mut_ptr().cast()),
                len,
                &mut len,
            )
        }?;
        Ok(buf)
    }

    /// The names of the privileges the token holds, enabled or not, such
    /// as `SeChangeNotifyPrivilege`.
    pub fn privileges(&self) -> io::Result<Vec<String>> {
        let buf = self.information(TokenPrivileges)?;
        // SAFETY: on success the buffer starts with a TOKEN_PRIVILEGES whose
        // `PrivilegeCount` entries follow its header inside the same buffer.
        let entries: &[LUID_AND_ATTRIBUTES] = unsafe {
            let header = buf.as_ptr().cast::<TOKEN_PRIVILEGES>();
            std::slice::from_raw_parts(
                (&raw const (*header).Privileges).cast(),
                (*header).PrivilegeCount as usize,
            )
        };
        entries.iter().map(|e| privilege_name(&e.Luid)).collect()
    }

    /// The user the token belongs to.
    pub fn user_sid(&self) -> io::Result<Sid> {
        let buf = self.information(TokenUser)?;
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

/// A privilege's name from its local ID.
fn privilege_name(luid: &LUID) -> io::Result<String> {
    let mut name = [0u16; 64];
    let mut len = name.len() as u32;
    // SAFETY: `luid` is valid; `name` holds `len` UTF-16 units and `len` is
    // a valid in/out pointer that receives the length written.
    unsafe { LookupPrivilegeNameW(None, luid, Some(PWSTR(name.as_mut_ptr())), &mut len) }?;
    Ok(String::from_utf16_lossy(
        &name[..(len as usize).min(name.len())],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_token_lists_change_notify() {
        let privileges = own_token().unwrap().privileges().unwrap();
        assert!(
            privileges.iter().any(|p| p == "SeChangeNotifyPrivilege"),
            "{privileges:?}"
        );
    }

    #[test]
    fn restricted_token_keeps_only_change_notify() {
        // Every token holds more than one privilege, a normal user's too
        // (SeShutdownPrivilege and others).
        assert!(own_token().unwrap().privileges().unwrap().len() > 1);
        let restricted = restricted_own_token().unwrap();
        assert_eq!(
            restricted.privileges().unwrap(),
            ["SeChangeNotifyPrivilege"]
        );
        assert_eq!(restricted.user_sid().unwrap(), own_sid().unwrap());
    }

    #[test]
    fn system_token_fails_cleanly_without_system() {
        // Moving a token to another session needs SeTcbPrivilege, which a
        // normal user lacks; as SYSTEM it succeeds. Either way, no panic.
        if let Ok(t) = system_token(active_console_session().unwrap()) {
            assert_eq!(t.privileges().unwrap(), ["SeChangeNotifyPrivilege"]);
        }
    }

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
