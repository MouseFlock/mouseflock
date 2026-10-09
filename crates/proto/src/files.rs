//! File copies (version 3): the manifest a `ClipboardFilesData` stream
//! carries, the `FileContentRequest` on the control stream, and the header of
//! a file content stream (`FileContentData`).
//!
//! A manifest lists a copy's files and directories by path relative to the
//! copy root, with each file's size; it never holds a file's bytes. It is
//! parsed in place, without allocating, and checked whole before any entry is
//! used: the entry count and total ceilings, every path, and that directories
//! have no size. The sender encodes with the same checks, so a copy it cannot
//! offer is found before anything is sent.

use crate::codec::fixed_payload;
use crate::{EncodeError, ProtocolError, frame, registry};

/// Most entries, files and directories, one manifest may list.
pub const MAX_MANIFEST_ENTRIES: u32 = 65_536;

/// Most bytes the files of one copy may total (64 GiB).
pub const MAX_FILES_TOTAL: u64 = 68_719_476_736;

/// Longest path, in UTF-16 code units, an entry may have: what a Windows
/// file descriptor holds, less its terminating NUL.
pub const MAX_PATH_UNITS: usize = 259;

/// Most bytes a manifest can take: its count, then every entry at its
/// longest (11 bytes before the path, and a path of [`MAX_PATH_UNITS`] units
/// at 3 UTF-8 bytes each). Checked against the other limits in the tests.
pub const MAX_MANIFEST_LEN: u32 = 51_642_372;

/// Bytes in a `FileContentRequest` payload: id u64, index u32.
const REQUEST_LEN: usize = 12;

/// Bytes in a `FileContentData` payload: id u64, index u32, len u64.
const CONTENT_DATA_LEN: usize = 20;

/// Why a manifest is refused, by its receiver or by the sender building it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestError {
    /// No entries.
    Empty,
    /// More than [`MAX_MANIFEST_ENTRIES`] entries.
    TooManyEntries,
    /// Files totalling more than [`MAX_FILES_TOTAL`] bytes.
    TooLarge,
    /// Ends inside an entry, or holds fewer entries than its count.
    Truncated,
    /// Bytes after the last entry.
    Trailing,
    /// An `is_dir` other than 0 or 1.
    DirFlag { value: u8 },
    /// A directory with a size other than 0.
    DirectorySize,
    /// A path that is not UTF-8.
    PathNotUtf8,
    /// A path longer than [`MAX_PATH_UNITS`] UTF-16 code units.
    PathTooLong,
    /// A path that is empty, absolute, names a drive, climbs with `..`, or
    /// holds a name Windows cannot create.
    UnsafePath,
}

/// One entry of a file copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileEntry<'a> {
    /// Relative to the copy root, `/` separated.
    pub path: &'a str,
    pub is_dir: bool,
    /// The file's length in bytes; 0 for a directory.
    pub size: u64,
}

/// A manifest checked whole, borrowing its bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Manifest<'a> {
    entries: &'a [u8],
    count: u32,
    total: u64,
}

impl<'a> Manifest<'a> {
    /// Checks a manifest's bytes: the count, then exactly that many entries,
    /// each well formed with a safe path, within the ceilings.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ManifestError> {
        let (count, entries) = bytes
            .split_first_chunk::<4>()
            .ok_or(ManifestError::Truncated)?;
        let count = u32::from_le_bytes(*count);
        if count == 0 {
            return Err(ManifestError::Empty);
        }
        if count > MAX_MANIFEST_ENTRIES {
            return Err(ManifestError::TooManyEntries);
        }
        let mut rest = entries;
        let mut total = 0u64;
        for _ in 0..count {
            let (entry, after) = entry(rest)?;
            total = add_size(total, entry.size)?;
            rest = after;
        }
        if !rest.is_empty() {
            return Err(ManifestError::Trailing);
        }
        Ok(Self {
            entries,
            count,
            total,
        })
    }

    /// How many entries, files and directories.
    pub const fn count(&self) -> u32 {
        self.count
    }

    /// The files' total size in bytes.
    pub const fn total(&self) -> u64 {
        self.total
    }

    /// The number of files, not counting directories.
    pub fn files(&self) -> u32 {
        let files = self.entries().filter(|e| !e.is_dir).count();
        u32::try_from(files).unwrap_or(u32::MAX)
    }

    /// The entries, in order. An entry's index here is the index a
    /// `FileContentRequest` names.
    pub const fn entries(&self) -> Entries<'a> {
        Entries {
            rest: self.entries,
            left: self.count,
        }
    }
}

/// The entries of a checked [`Manifest`].
#[derive(Debug, Clone)]
pub struct Entries<'a> {
    rest: &'a [u8],
    left: u32,
}

impl<'a> Iterator for Entries<'a> {
    type Item = FileEntry<'a>;

    fn next(&mut self) -> Option<FileEntry<'a>> {
        self.left = self.left.checked_sub(1)?;
        let (entry, rest) = entry(self.rest).ok()?;
        self.rest = rest;
        Some(entry)
    }
}

/// One entry from the front of `bytes`, and what follows it.
fn entry(bytes: &[u8]) -> Result<(FileEntry<'_>, &[u8]), ManifestError> {
    let (&flag, rest) = bytes.split_first().ok_or(ManifestError::Truncated)?;
    let (size, rest) = rest
        .split_first_chunk::<8>()
        .ok_or(ManifestError::Truncated)?;
    let (path_len, rest) = rest
        .split_first_chunk::<2>()
        .ok_or(ManifestError::Truncated)?;
    let (path, rest) = rest
        .split_at_checked(usize::from(u16::from_le_bytes(*path_len)))
        .ok_or(ManifestError::Truncated)?;
    let is_dir = match flag {
        0 => false,
        1 => true,
        value => return Err(ManifestError::DirFlag { value }),
    };
    let size = u64::from_le_bytes(*size);
    if is_dir && size != 0 {
        return Err(ManifestError::DirectorySize);
    }
    let path = std::str::from_utf8(path).map_err(|_| ManifestError::PathNotUtf8)?;
    check_path(path)?;
    Ok((FileEntry { path, is_dir, size }, rest))
}

fn add_size(total: u64, size: u64) -> Result<u64, ManifestError> {
    total
        .checked_add(size)
        .filter(|t| *t <= MAX_FILES_TOTAL)
        .ok_or(ManifestError::TooLarge)
}

/// Checks that `path` stays under the folder it is pasted into and can be
/// created there: relative, `/` separated, at most [`MAX_PATH_UNITS`] UTF-16
/// units, and every name in it one Windows can create (no `.` or `..`, no
/// drive, no reserved character or device name, no trailing dot or space).
pub fn check_path(path: &str) -> Result<(), ManifestError> {
    if path.encode_utf16().count() > MAX_PATH_UNITS {
        return Err(ManifestError::PathTooLong);
    }
    if path.split('/').all(name_ok) {
        Ok(())
    } else {
        Err(ManifestError::UnsafePath)
    }
}

/// Whether `name` is one path component Windows creates as written. An empty
/// name (a leading, doubled or trailing `/`) is refused, so an absolute or
/// UNC path never passes.
fn name_ok(name: &str) -> bool {
    const RESERVED: &[&str] = &["CON", "PRN", "AUX", "NUL"];
    const NUMBERED: &[&str] = &["COM", "LPT"];
    if name.is_empty() || name.ends_with(['.', ' ']) {
        return false;
    }
    if name
        .chars()
        .any(|c| c < ' ' || matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
    {
        return false;
    }
    // Device names are reserved whatever follows the first dot.
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    let device = RESERVED.iter().any(|r| stem.eq_ignore_ascii_case(r))
        || NUMBERED.iter().any(|r| {
            stem.len() == 4
                && stem.get(..3).is_some_and(|s| s.eq_ignore_ascii_case(r))
                && stem.get(3..).is_some_and(|d| {
                    matches!(d, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
                })
        });
    !device
}

/// Appends a manifest of `entries` to `out`, with the same checks a receiver
/// makes. On error `out` is left as it was.
pub fn encode_manifest<'e>(
    entries: impl IntoIterator<Item = FileEntry<'e>>,
    out: &mut Vec<u8>,
) -> Result<(), ManifestError> {
    let start = out.len();
    let result = encode_entries(entries, out);
    match result {
        Ok(count) => {
            if let Some(slot) = out.get_mut(start..).and_then(|s| s.first_chunk_mut::<4>()) {
                *slot = count.to_le_bytes();
            }
            Ok(())
        }
        Err(e) => {
            out.truncate(start);
            Err(e)
        }
    }
}

fn encode_entries<'e>(
    entries: impl IntoIterator<Item = FileEntry<'e>>,
    out: &mut Vec<u8>,
) -> Result<u32, ManifestError> {
    out.extend_from_slice(&[0; 4]);
    let mut count = 0u32;
    let mut total = 0u64;
    for e in entries {
        count = count
            .checked_add(1)
            .filter(|c| *c <= MAX_MANIFEST_ENTRIES)
            .ok_or(ManifestError::TooManyEntries)?;
        if e.is_dir && e.size != 0 {
            return Err(ManifestError::DirectorySize);
        }
        check_path(e.path)?;
        total = add_size(total, e.size)?;
        // At most 777 bytes once the path passed its check.
        let path_len = u16::try_from(e.path.len()).map_err(|_| ManifestError::PathTooLong)?;
        out.push(u8::from(e.is_dir));
        out.extend_from_slice(&e.size.to_le_bytes());
        out.extend_from_slice(&path_len.to_le_bytes());
        out.extend_from_slice(e.path.as_bytes());
    }
    if count == 0 {
        return Err(ManifestError::Empty);
    }
    Ok(count)
}

/// `FileContentRequest`: asks the receiver for one file of its file copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileContentRequest {
    /// The copy's ID, as offered.
    pub id: u64,
    /// The file's index in the copy's manifest.
    pub index: u32,
}

impl FileContentRequest {
    /// Decodes a control-stream message already checked by the session.
    pub fn decode(payload: &[u8]) -> Result<Self, ProtocolError> {
        let p = fixed_payload::<REQUEST_LEN>(registry::FILE_CONTENT_REQUEST, payload)?;
        let (id, index) = p.split_at(8);
        let (Ok(id), Ok(index)) = (<[u8; 8]>::try_from(id), <[u8; 4]>::try_from(index)) else {
            return Err(ProtocolError::PayloadLength {
                ty: registry::FILE_CONTENT_REQUEST,
                expected: REQUEST_LEN,
                actual: payload.len(),
            });
        };
        Ok(Self {
            id: u64::from_le_bytes(id),
            index: u32::from_le_bytes(index),
        })
    }

    pub fn payload(&self) -> [u8; REQUEST_LEN] {
        let mut p = [0u8; REQUEST_LEN];
        let (id, index) = p.split_at_mut(8);
        id.copy_from_slice(&self.id.to_le_bytes());
        index.copy_from_slice(&self.index.to_le_bytes());
        p
    }

    /// Appends this message as one frame.
    pub fn encode(&self, out: &mut Vec<u8>) -> Result<(), EncodeError> {
        frame::encode(registry::FILE_CONTENT_REQUEST, &self.payload(), out)
    }
}

/// Appends a file content stream's first frame: `FileContentData` with the
/// copy's `id`, the file's `index` in its manifest, and its length.
pub fn encode_content_data(
    id: u64,
    index: u32,
    len: u64,
    out: &mut Vec<u8>,
) -> Result<(), EncodeError> {
    frame::encode(
        registry::FILE_CONTENT_DATA,
        &content_data_payload(id, index, len),
        out,
    )
}

fn content_data_payload(id: u64, index: u32, len: u64) -> [u8; CONTENT_DATA_LEN] {
    let mut p = [0u8; CONTENT_DATA_LEN];
    let (id_bytes, rest) = p.split_at_mut(8);
    let (index_bytes, len_bytes) = rest.split_at_mut(4);
    id_bytes.copy_from_slice(&id.to_le_bytes());
    index_bytes.copy_from_slice(&index.to_le_bytes());
    len_bytes.copy_from_slice(&len.to_le_bytes());
    p
}

/// A `FileContentData` payload: the copy's ID, the file's index and length.
pub(crate) fn decode_content_data(payload: &[u8]) -> Result<(u64, u32, u64), ProtocolError> {
    let p = fixed_payload::<CONTENT_DATA_LEN>(registry::FILE_CONTENT_DATA, payload)?;
    let short = || ProtocolError::PayloadLength {
        ty: registry::FILE_CONTENT_DATA,
        expected: CONTENT_DATA_LEN,
        actual: payload.len(),
    };
    let (id, rest) = p.split_first_chunk::<8>().ok_or_else(short)?;
    let (index, len) = rest.split_first_chunk::<4>().ok_or_else(short)?;
    let len = <[u8; 8]>::try_from(len).map_err(|_| short())?;
    Ok((
        u64::from_le_bytes(*id),
        u32::from_le_bytes(*index),
        u64::from_le_bytes(len),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, size: u64) -> FileEntry<'_> {
        FileEntry {
            path,
            is_dir: false,
            size,
        }
    }

    fn dir(path: &str) -> FileEntry<'_> {
        FileEntry {
            path,
            is_dir: true,
            size: 0,
        }
    }

    fn encoded(entries: &[FileEntry<'_>]) -> Vec<u8> {
        let mut out = Vec::new();
        encode_manifest(entries.iter().copied(), &mut out).unwrap();
        out
    }

    /// Bytes before an entry's path: `is_dir` u8, `size` u64, `path_len` u16.
    const ENTRY_HEAD: usize = 11;

    /// Longest path in UTF-8 bytes: each UTF-16 unit is at most 3 bytes.
    const MAX_PATH_BYTES: usize = 777;

    #[test]
    fn manifest_len_bound_follows_from_the_other_limits() {
        assert_eq!(MAX_PATH_BYTES, MAX_PATH_UNITS * 3);
        let most = 4 + MAX_MANIFEST_ENTRIES as usize * (ENTRY_HEAD + MAX_PATH_BYTES);
        assert_eq!(MAX_MANIFEST_LEN as usize, most);
        // A path at the unit limit, all three-byte characters, is the longest.
        let longest = "€".repeat(MAX_PATH_UNITS);
        assert_eq!(longest.len(), MAX_PATH_BYTES);
        assert_eq!(check_path(&longest), Ok(()));
    }

    #[test]
    fn manifest_round_trips() {
        let entries = [
            dir("Photos"),
            file("Photos/a.jpg", 1234),
            dir("Photos/empty"),
            file("Photos/sub/big.bin", 5 << 32),
            file("notes é.txt", 0),
        ];
        let bytes = encoded(&entries);
        let m = Manifest::parse(&bytes).unwrap();
        assert_eq!(m.count(), 5);
        assert_eq!(m.files(), 3);
        assert_eq!(m.total(), 1234 + (5 << 32));
        assert_eq!(m.entries().collect::<Vec<_>>(), entries);
        // The layout: count, then is_dir, size, path_len, path.
        assert_eq!(&bytes[..4], &5u32.to_le_bytes());
        assert_eq!(bytes[4], 1);
        assert_eq!(&bytes[5..13], &0u64.to_le_bytes());
        assert_eq!(&bytes[13..15], &6u16.to_le_bytes());
        assert_eq!(&bytes[15..21], b"Photos");
    }

    #[test]
    fn counts_and_totals_are_bounded() {
        assert_eq!(
            encode_manifest([], &mut Vec::new()),
            Err(ManifestError::Empty)
        );
        assert_eq!(
            Manifest::parse(&0u32.to_le_bytes()),
            Err(ManifestError::Empty)
        );
        let mut over = (MAX_MANIFEST_ENTRIES + 1).to_le_bytes().to_vec();
        over.extend_from_slice(&encoded(&[file("a", 1)])[4..]);
        assert_eq!(Manifest::parse(&over), Err(ManifestError::TooManyEntries));
        let names: Vec<String> = (0..=MAX_MANIFEST_ENTRIES)
            .map(|i| format!("f{i}"))
            .collect();
        let mut out = vec![7];
        assert_eq!(
            encode_manifest(names.iter().map(|n| file(n, 0)), &mut out),
            Err(ManifestError::TooManyEntries)
        );
        assert_eq!(out, [7], "a failed encode leaves the buffer as it was");
        let exactly: Vec<_> = names[..MAX_MANIFEST_ENTRIES as usize]
            .iter()
            .map(|n| file(n, 0))
            .collect();
        assert_eq!(
            Manifest::parse(&encoded(&exactly)).map(|m| m.count()),
            Ok(MAX_MANIFEST_ENTRIES)
        );

        let at_limit = [file("a", MAX_FILES_TOTAL - 1), file("b", 1)];
        assert_eq!(
            Manifest::parse(&encoded(&at_limit)).map(|m| m.total()),
            Ok(MAX_FILES_TOTAL)
        );
        let too_large = [file("a", MAX_FILES_TOTAL), file("b", 1)];
        assert_eq!(
            encode_manifest(too_large, &mut Vec::new()),
            Err(ManifestError::TooLarge)
        );
        // A hostile total that would wrap a u64.
        let mut wrap = encoded(&[file("a", 1), file("b", 1)]);
        wrap[5..13].copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(Manifest::parse(&wrap), Err(ManifestError::TooLarge));
    }

    #[test]
    fn malformed_manifests_are_refused() {
        let good = encoded(&[dir("d"), file("d/f", 3)]);
        for cut in 0..good.len() {
            assert_eq!(
                Manifest::parse(&good[..cut]),
                Err(ManifestError::Truncated),
                "cut at {cut}"
            );
        }
        let mut trailing = good.clone();
        trailing.push(0);
        assert_eq!(Manifest::parse(&trailing), Err(ManifestError::Trailing));
        let mut flag = good.clone();
        flag[4] = 2;
        assert_eq!(
            Manifest::parse(&flag),
            Err(ManifestError::DirFlag { value: 2 })
        );
        let mut sized_dir = good.clone();
        sized_dir[5] = 1;
        assert_eq!(
            Manifest::parse(&sized_dir),
            Err(ManifestError::DirectorySize)
        );
        let mut not_utf8 = good;
        not_utf8[15] = 0xff;
        assert_eq!(Manifest::parse(&not_utf8), Err(ManifestError::PathNotUtf8));
    }

    /// A manifest of one file at `path`, written without the encoder's
    /// checks, as a hostile sender would.
    fn raw(path: &str) -> Vec<u8> {
        let mut out = 1u32.to_le_bytes().to_vec();
        out.push(0);
        out.extend_from_slice(&1u64.to_le_bytes());
        out.extend_from_slice(&(path.len() as u16).to_le_bytes());
        out.extend_from_slice(path.as_bytes());
        out
    }

    #[test]
    fn paths_stay_under_the_paste_target() {
        for bad in [
            "..",
            "../x",
            "a/../../x",
            "..\\..\\Windows\\System32\\evil.dll",
            "a\\b",
            "/abs",
            "//server/share/x",
            "C:\\Windows\\x.dll",
            "C:x",
            "a//b",
            "a/",
            "",
            ".",
            "a/./b",
            "x.txt:stream",
            "con",
            "NUL.txt",
            "a/Com1.log",
            "lpt9",
            "prn .txt",
            "trailing.",
            "trailing ",
            "a\u{1}b",
            "what?",
            "pipe|",
        ] {
            assert_eq!(check_path(bad), Err(ManifestError::UnsafePath), "{bad:?}");
            assert_eq!(
                Manifest::parse(&raw(bad)),
                Err(ManifestError::UnsafePath),
                "{bad:?}"
            );
            assert_eq!(
                encode_manifest([file(bad, 1)], &mut Vec::new()),
                Err(ManifestError::UnsafePath),
                "{bad:?}"
            );
        }
        for good in [
            "a",
            "a/b/c.txt",
            ".hidden",
            "..dots in front",
            "com",
            "COM10",
            "console.log",
            "nul-ish",
            "名前/ファイル.txt",
            "emoji 😀.png",
        ] {
            assert_eq!(check_path(good), Ok(()), "{good:?}");
        }
    }

    #[test]
    fn paths_over_259_units_are_refused() {
        let fits = "a/".repeat(129) + "b";
        assert_eq!(fits.encode_utf16().count(), 259);
        assert_eq!(check_path(&fits), Ok(()));
        let over = fits.clone() + "c";
        assert_eq!(check_path(&over), Err(ManifestError::PathTooLong));
        assert_eq!(
            Manifest::parse(&raw(&over)),
            Err(ManifestError::PathTooLong)
        );
        // Counted in UTF-16 units: an emoji is two.
        let emoji = "😀".repeat(130);
        assert_eq!(check_path(&emoji), Err(ManifestError::PathTooLong));
        assert_eq!(check_path(&"😀".repeat(129)), Ok(()));
    }

    #[test]
    fn content_request_round_trips() {
        let r = FileContentRequest {
            id: u64::MAX,
            index: 65_535,
        };
        assert_eq!(FileContentRequest::decode(&r.payload()), Ok(r));
        let mut out = Vec::new();
        r.encode(&mut out).unwrap();
        assert_eq!(out.len(), frame::HEADER_LEN + 12);
        for len in [11, 13] {
            assert_eq!(
                FileContentRequest::decode(&vec![0; len]),
                Err(ProtocolError::PayloadLength {
                    ty: registry::FILE_CONTENT_REQUEST,
                    expected: 12,
                    actual: len
                })
            );
        }
    }

    #[test]
    fn content_data_round_trips() {
        let p = content_data_payload(7, 3, 5 << 32);
        assert_eq!(decode_content_data(&p), Ok((7, 3, 5 << 32)));
        assert_eq!(
            decode_content_data(&p[..19]),
            Err(ProtocolError::PayloadLength {
                ty: registry::FILE_CONTENT_DATA,
                expected: 20,
                actual: 19
            })
        );
    }
}
