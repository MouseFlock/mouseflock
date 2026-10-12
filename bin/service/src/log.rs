//! A small logger (design D12): a rotating file in service mode, stderr in
//! console mode. Lines never carry input contents, codes or keys; callers
//! log reasons and names only.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// A log file is rotated when it reaches this size.
pub const MAX_LOG: u64 = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Info => "INFO ",
            Self::Warn => "WARN ",
            Self::Error => "ERROR",
        })
    }
}

/// Creates a new, empty log file with the right ACL (service mode).
pub type Create = fn(&Path) -> io::Result<()>;

enum Sink {
    Stderr,
    File {
        path: PathBuf,
        file: File,
        len: u64,
        max: u64,
        create: Option<Create>,
    },
    Memory(Vec<String>),
}

/// Where log lines go. Cheap to clone; clones share the sink.
#[derive(Clone)]
pub struct Log(Arc<Mutex<Sink>>);

impl Log {
    pub fn stderr() -> Self {
        Self(Arc::new(Mutex::new(Sink::Stderr)))
    }

    /// Appends to `path`, keeping one previous file at `<path>.old`.
    /// `create` makes each new file before it is opened.
    pub fn file(path: &Path, create: Option<Create>) -> io::Result<Self> {
        Self::file_with_max(path, MAX_LOG, create)
    }

    pub fn file_with_max(path: &Path, max: u64, create: Option<Create>) -> io::Result<Self> {
        if let Some(create) = create {
            create(path)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let len = file.metadata()?.len();
        Ok(Self(Arc::new(Mutex::new(Sink::File {
            path: path.to_owned(),
            file,
            len,
            max,
            create,
        }))))
    }

    /// Keeps lines in memory (tests).
    pub fn memory() -> Self {
        Self(Arc::new(Mutex::new(Sink::Memory(Vec::new()))))
    }

    /// The lines kept by [`Log::memory`].
    pub fn lines(&self) -> Vec<String> {
        match &*self.lock() {
            Sink::Memory(lines) => lines.clone(),
            _ => Vec::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Sink> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn write(&self, level: Level, message: fmt::Arguments<'_>) {
        let line = format!("{} {level} {message}\n", timestamp(SystemTime::now()));
        match &mut *self.lock() {
            Sink::Stderr => {
                let _ = io::stderr().write_all(line.as_bytes());
            }
            Sink::Memory(lines) => lines.push(line.trim_end().to_owned()),
            Sink::File {
                path,
                file,
                len,
                max,
                create,
            } => {
                if *len + line.len() as u64 > *max && *len > 0 {
                    let _ = rotate(path, file, *create);
                    *len = 0;
                }
                if file.write_all(line.as_bytes()).is_ok() {
                    *len += line.len() as u64;
                }
            }
        }
    }

    pub fn info(&self, message: fmt::Arguments<'_>) {
        self.write(Level::Info, message);
    }

    pub fn warn(&self, message: fmt::Arguments<'_>) {
        self.write(Level::Warn, message);
    }

    pub fn error(&self, message: fmt::Arguments<'_>) {
        self.write(Level::Error, message);
    }
}

/// Renames `path` to `<path>.old`, replacing any older one, and reopens.
fn rotate(path: &Path, file: &mut File, create: Option<Create>) -> io::Result<()> {
    let mut old = path.as_os_str().to_owned();
    old.push(".old");
    // std opens files with FILE_SHARE_DELETE, so the open handle does not
    // block the rename.
    std::fs::rename(path, &old)?;
    if let Some(create) = create {
        create(path)?;
    }
    *file = OpenOptions::new().create(true).append(true).open(path)?;
    Ok(())
}

#[macro_export]
macro_rules! info {
    ($log:expr, $($arg:tt)*) => { $log.info(format_args!($($arg)*)) };
}
#[macro_export]
macro_rules! warn {
    ($log:expr, $($arg:tt)*) => { $log.warn(format_args!($($arg)*)) };
}
#[macro_export]
macro_rules! error {
    ($log:expr, $($arg:tt)*) => { $log.error(format_args!($($arg)*)) };
}

/// `2026-10-04T12:00:00.123Z`.
pub fn timestamp(t: SystemTime) -> String {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs();
    let (y, m, day) = civil_from_days((secs / 86_400) as i64);
    let s = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        s / 3600,
        s / 60 % 60,
        s % 60,
        d.subsec_millis()
    )
}

/// Year, month and day of the `days`th day after 1970-01-01 (Howard
/// Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(secs: u64, millis: u64) -> String {
        timestamp(UNIX_EPOCH + Duration::from_secs(secs) + Duration::from_millis(millis))
    }

    #[test]
    fn timestamps() {
        assert_eq!(at(0, 0), "1970-01-01T00:00:00.000Z");
        // 2024-02-29, a leap day.
        assert_eq!(at(1_709_208_000, 5), "2024-02-29T12:00:00.005Z");
        assert_eq!(at(1_709_251_199, 999), "2024-02-29T23:59:59.999Z");
        assert_eq!(at(1_709_251_200, 0), "2024-03-01T00:00:00.000Z");
        // 2000-02-29: divisible by 400.
        assert_eq!(at(951_782_400, 0), "2000-02-29T00:00:00.000Z");
        assert_eq!(at(1_791_115_200, 123), "2026-10-04T12:00:00.123Z");
    }

    #[test]
    fn rotation_keeps_one_old_file() {
        let dir = std::env::temp_dir().join(format!("mouseflock-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("service.log");
        let log = Log::file_with_max(&path, 200, None).unwrap();
        for i in 0..20 {
            info!(log, "line {i}");
        }
        let old = dir.join("service.log.old");
        let current = std::fs::read_to_string(&path).unwrap();
        let previous = std::fs::read_to_string(&old).unwrap();
        assert!(current.len() <= 200 && previous.len() <= 200);
        assert!(current.contains("line 19"), "{current}");
        assert!(previous.ends_with('\n') && !previous.contains("line 19"));
        let files = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(files, 2);
        drop(log);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn each_new_file_is_created_by_the_hook() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CREATED: AtomicUsize = AtomicUsize::new(0);
        fn create(path: &Path) -> io::Result<()> {
            CREATED.fetch_add(1, Ordering::SeqCst);
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map(drop)
        }
        let dir = std::env::temp_dir().join(format!("mouseflock-loghook-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = Log::file_with_max(&dir.join("service.log"), 50, Some(create)).unwrap();
        assert_eq!(CREATED.load(Ordering::SeqCst), 1);
        for i in 0..6 {
            info!(log, "line {i}");
        }
        // One line per file at this size: the first file, then 5 rotations.
        assert_eq!(CREATED.load(Ordering::SeqCst), 6);
        drop(log);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn memory_keeps_lines() {
        let log = Log::memory();
        warn!(log, "peer {} down", "B");
        let lines = log.lines();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].ends_with("WARN  peer B down"), "{}", lines[0]);
    }
}
