//! Whether this thread's desktop is the input desktop (add-winlogon-agent
//! design D6). The winlogon agent runs on the secure desktop and reports
//! when it gains or loses the input; the screen thread polls this.

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS, GetThreadDesktop,
    GetUserObjectInformationW, HDESK, OpenInputDesktop, UOI_NAME,
};

use crate::capture::thread_id;

/// How often the screen thread compares the desktops, in milliseconds.
pub(crate) const POLL_MS: u32 = 50;

/// Whether the calling thread's desktop is the input desktop. `false` when
/// this process may not open the input desktop, which is how a user's
/// process sees the secure desktop.
pub(crate) fn own_is_input() -> bool {
    is_input(own_name().as_deref(), input_name().as_deref())
}

/// Desktop names compare without case, as Windows does.
pub(crate) fn is_input(own: Option<&str>, input: Option<&str>) -> bool {
    matches!((own, input), (Some(own), Some(input)) if own.eq_ignore_ascii_case(input))
}

/// Passes on a reading only when it differs from the last one passed.
#[derive(Debug, Default)]
pub(crate) struct Changes {
    last: Option<bool>,
}

impl Changes {
    pub(crate) fn offer(&mut self, now: bool) -> Option<bool> {
        (self.last != Some(now)).then(|| {
            self.last = Some(now);
            now
        })
    }
}

fn own_name() -> Option<String> {
    // SAFETY: no pointers; the handle it returns belongs to the thread and
    // must not be closed.
    let desk = unsafe { GetThreadDesktop(thread_id()) }.ok()?;
    name(desk)
}

fn input_name() -> Option<String> {
    // SAFETY: no pointers; the handle it returns is closed below.
    let desk =
        unsafe { OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS) }.ok()?;
    let out = name(desk);
    // SAFETY: `desk` came from OpenInputDesktop and is closed once.
    let _ = unsafe { CloseDesktop(desk) };
    out
}

fn name(desk: HDESK) -> Option<String> {
    let mut buf = [0u16; 256];
    let mut needed = 0u32;
    // SAFETY: `desk` is open for the call; `buf` holds the byte length
    // passed, and `needed` is a valid out-pointer.
    unsafe {
        GetUserObjectInformationW(
            HANDLE(desk.0),
            UOI_NAME,
            Some(buf.as_mut_ptr().cast()),
            size_of_val(&buf) as u32,
            Some(&mut needed),
        )
    }
    .ok()?;
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_desktop_any_case() {
        assert!(is_input(Some("Winlogon"), Some("Winlogon")));
        assert!(is_input(Some("Default"), Some("default")));
    }

    #[test]
    fn another_desktop_has_the_input() {
        assert!(!is_input(Some("Winlogon"), Some("Default")));
        assert!(!is_input(Some("Default"), Some("Winlogon")));
    }

    #[test]
    fn input_desktop_cannot_be_opened() {
        assert!(!is_input(Some("Default"), None));
        assert!(!is_input(None, Some("Default")));
        assert!(!is_input(None, None));
    }

    #[test]
    fn only_changes_pass() {
        let mut c = Changes::default();
        assert_eq!(c.offer(true), Some(true));
        assert_eq!(c.offer(true), None);
        assert_eq!(c.offer(false), Some(false));
        assert_eq!(c.offer(false), None);
        assert_eq!(c.offer(true), Some(true));
    }

    #[test]
    fn this_thread_names_its_desktop() {
        // Tests run on a desktop, logged on or not; whichever it is, it has
        // a name. Whether it is the input desktop depends on the session.
        assert!(own_name().is_some_and(|n| !n.is_empty()));
    }
}
