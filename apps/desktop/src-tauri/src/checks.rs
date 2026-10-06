//! What the app checks before it talks to the service or sends a code
//! (design D2, D4). Code rules beyond what the UI link can carry stay with
//! the service.

use ipc::ui::MAX_CODE_LEN;

/// Development builds also accept a console-mode service in their own
/// session. Release builds never do.
pub const DEV: bool = cfg!(debug_assertions);

/// Whether a pipe server in session `server` may be the service, for an app
/// in session `own`. Only services run in session 0.
pub const fn trust(server: u32, own: u32, dev: bool) -> bool {
    server == 0 || (dev && server == own)
}

/// Why typed text was not sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidCode;

/// The typed code with all whitespace removed, if the UI link can carry it:
/// 1 to 32 bytes of printable ASCII.
pub fn normalize_code(text: &str) -> Result<String, InvalidCode> {
    let code: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let printable = code.bytes().all(|b| (0x21..=0x7E).contains(&b));
    if code.is_empty() || code.len() > MAX_CODE_LEN || !printable {
        return Err(InvalidCode);
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trust_rules() {
        assert!(trust(0, 1, false));
        assert!(trust(0, 1, true));
        assert!(!trust(1, 1, false));
        assert!(trust(1, 1, true));
        assert!(!trust(2, 1, false));
        assert!(!trust(2, 1, true));
    }

    #[test]
    fn codes_lose_whitespace() {
        assert_eq!(normalize_code("137 - 482 9153").unwrap(), "137-4829153");
        assert_eq!(normalize_code("\t137-4829153\n").unwrap(), "137-4829153");
    }

    #[test]
    fn codes_must_fit_the_link() {
        assert!(normalize_code(&"1".repeat(32)).is_ok());
        assert_eq!(normalize_code(&"1".repeat(33)), Err(InvalidCode));
        assert_eq!(normalize_code(""), Err(InvalidCode));
        assert_eq!(normalize_code("   "), Err(InvalidCode));
        assert_eq!(normalize_code("137-48é"), Err(InvalidCode));
        assert_eq!(normalize_code("137\u{7f}"), Err(InvalidCode));
    }
}
