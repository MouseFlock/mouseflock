//! Security identifiers in their string form.

use std::fmt;

/// The local system account.
pub const SYSTEM: &str = "S-1-5-18";
/// The local Administrators group.
pub const ADMINISTRATORS: &str = "S-1-5-32-544";
/// The local Guests group (tests).
pub const GUESTS: &str = "S-1-5-32-546";

/// A security identifier as Windows writes it (`S-1-5-…`). Only `S`, digits
/// and dashes can appear, so a `Sid` is safe to splice into SDDL.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Sid(String);

impl Sid {
    /// Accepts `S-1-` followed by dash-separated decimal numbers.
    pub fn parse(s: &str) -> Option<Self> {
        let rest = s.strip_prefix("S-1-")?;
        let ok = !rest.is_empty()
            && rest.split('-').all(|part| {
                !part.is_empty() && part.len() <= 10 && part.bytes().all(|b| b.is_ascii_digit())
            });
        ok.then(|| Self(s.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Sid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Owners whose data folder the service keeps: SYSTEM, and the
/// Administrators group, which owns what an elevated installer creates.
pub fn is_trusted_owner(owner: &Sid) -> bool {
    owner.as_str() == SYSTEM || owner.as_str() == ADMINISTRATORS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_windows_sids() {
        for s in [
            SYSTEM,
            ADMINISTRATORS,
            GUESTS,
            "S-1-5-21-1004336348-1177238915-682003330-1001",
        ] {
            assert_eq!(Sid::parse(s).unwrap().as_str(), s);
        }
    }

    #[test]
    fn rejects_anything_that_could_escape_sddl() {
        for s in [
            "",
            "S-1-",
            "S-1-5-",
            "S-1-5--18",
            "SY",
            "S-1-5-18)(A;;GA;;;WD",
            "s-1-5-18",
            "S-1-5-18 ",
            "S-1-5-99999999999",
        ] {
            assert_eq!(Sid::parse(s), None, "{s:?}");
        }
    }

    #[test]
    fn trusted_owners() {
        assert!(is_trusted_owner(&Sid::parse(SYSTEM).unwrap()));
        assert!(is_trusted_owner(&Sid::parse(ADMINISTRATORS).unwrap()));
        assert!(!is_trusted_owner(
            &Sid::parse("S-1-5-21-1-2-3-1001").unwrap()
        ));
        assert!(!is_trusted_owner(&Sid::parse(GUESTS).unwrap()));
    }
}
