//! The clipboard's rules without the clipboard: which copies are private,
//! and converting `CF_UNICODETEXT` to and from UTF-8 within the limit.

/// Most bytes of UTF-8 text shared (4 MiB), as on the agent link.
pub const MAX_TEXT_LEN: usize = 4_194_304;

/// Text over [`MAX_TEXT_LEN`] bytes of UTF-8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooLarge;

/// What the clipboard says about privacy, as the copying app set it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Markers {
    /// `ExcludeClipboardContentFromMonitorProcessing` is present.
    pub exclude: bool,
    /// `CanIncludeInClipboardHistory`'s DWORD, if present.
    pub history: Option<u32>,
    /// `CanUploadToCloudClipboard`'s DWORD, if present.
    pub cloud: Option<u32>,
}

impl Markers {
    /// Whether the copy must stay on this machine: excluded from monitoring,
    /// or barred from history or cloud sync.
    pub const fn private(self) -> bool {
        self.exclude || matches!(self.history, Some(0)) || matches!(self.cloud, Some(0))
    }
}

/// UTF-16 clipboard text up to its first NUL, as UTF-8, if it fits.
/// Unpaired surrogates become U+FFFD, as Windows apps may leave them.
pub fn to_utf8(units: &[u16]) -> Result<String, TooLarge> {
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    let units = units.get(..end).unwrap_or(units);
    // Every unit is at least one byte of UTF-8, so this bound is safe to
    // check before converting anything.
    if units.len() > MAX_TEXT_LEN {
        return Err(TooLarge);
    }
    let text: String = char::decode_utf16(units.iter().copied())
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect();
    if text.len() > MAX_TEXT_LEN {
        return Err(TooLarge);
    }
    Ok(text)
}

/// UTF-8 text as NUL-terminated UTF-16, ready for `CF_UNICODETEXT`.
pub fn to_utf16z(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privacy_markers() {
        assert!(!Markers::default().private());
        let excluded = Markers {
            exclude: true,
            ..Markers::default()
        };
        assert!(excluded.private());
        for (history, cloud, private) in [
            (Some(0), None, true),
            (None, Some(0), true),
            (Some(1), None, false),
            (None, Some(1), false),
            (Some(1), Some(1), false),
            (Some(1), Some(0), true),
        ] {
            let m = Markers {
                exclude: false,
                history,
                cloud,
            };
            assert_eq!(m.private(), private, "{m:?}");
        }
    }

    #[test]
    fn utf16_stops_at_nul() {
        let units = to_utf16z("héllo\r\n");
        assert_eq!(to_utf8(&units), Ok("héllo\r\n".into()));
        let mut embedded: Vec<u16> = "ab".encode_utf16().collect();
        embedded.push(0);
        embedded.extend("cd".encode_utf16());
        assert_eq!(to_utf8(&embedded), Ok("ab".into()));
        assert_eq!(to_utf8(&[]), Ok(String::new()));
    }

    #[test]
    fn surrogate_pairs_and_strays() {
        let text = "a😀b";
        assert_eq!(to_utf8(&to_utf16z(text)), Ok(text.into()));
        assert_eq!(to_utf8(&[0x61, 0xD800, 0x62]), Ok("a\u{FFFD}b".into()));
    }

    #[test]
    fn the_limit_is_utf8_bytes() {
        let max = "x".repeat(MAX_TEXT_LEN);
        assert_eq!(to_utf8(&to_utf16z(&max)).map(|t| t.len()), Ok(MAX_TEXT_LEN));
        let over = "x".repeat(MAX_TEXT_LEN + 1);
        assert_eq!(to_utf8(&to_utf16z(&over)), Err(TooLarge));
        // Fewer units than the limit, but more bytes once in UTF-8.
        let wide = "é".repeat(MAX_TEXT_LEN / 2 + 1);
        assert!(wide.encode_utf16().count() < MAX_TEXT_LEN);
        assert_eq!(to_utf8(&to_utf16z(&wide)), Err(TooLarge));
    }

    #[test]
    fn utf16z_ends_in_nul() {
        assert_eq!(to_utf16z(""), [0]);
        assert_eq!(to_utf16z("A"), [0x41, 0]);
    }
}
