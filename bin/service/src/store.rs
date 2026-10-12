//! The data folder and the files in it (design D7, D8).
//!
//! Every file is written to `<name>.tmp`, flushed, and renamed over the old
//! one, so a crash leaves the old contents or the new ones. A file that
//! cannot be read is moved aside to `<name>.bad-<unix seconds>` and treated
//! as absent; nothing partial is ever used.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use model::{Layout, LayoutBook, MonitorId, MonitorSetKey, PeerId, Side};
use pairing::{Identity, Machine, MachineName};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

use crate::log::Log;
use crate::{error, info, warn};

pub const IDENTITY: &str = "identity.key";
pub const TRUST_STORE: &str = "trust-store.bin";
pub const ADDRESSES: &str = "addresses.json";
pub const LAYOUTS: &str = "layouts.json";
pub const LOG: &str = "service.log";

/// Larger stored files are unreadable.
pub const MAX_FILE: u64 = 1 << 20;

/// The registered service's data folder.
pub fn service_dir() -> PathBuf {
    let base = std::env::var_os("ProgramData").unwrap_or_else(|| r"C:\ProgramData".into());
    Path::new(&base).join("MouseFlock")
}

/// Console mode's default data folder, under the user's local application
/// data.
pub fn console_dir() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(Path::new(&base).join("MouseFlock").join("console"))
}

/// Makes the data folder ready before anything in it is read: in service
/// mode, owned by SYSTEM and open to SYSTEM only (moved aside and replaced
/// if anyone else owns it); in console mode, simply present.
pub fn prepare(dir: &Path, service: bool, log: &Log) -> io::Result<()> {
    if !service {
        return std::fs::create_dir_all(dir);
    }
    match win32_session::secure_folder(dir)? {
        win32_session::Prepared::Created => info!(log, "created data folder {}", dir.display()),
        win32_session::Prepared::Repaired => {}
        win32_session::Prepared::Replaced { moved_to } => warn!(
            log,
            "data folder {} had an untrusted owner; moved it to {} unread and started a new one",
            dir.display(),
            moved_to.display()
        ),
    }
    Ok(())
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Moves `name` aside as unreadable and logs why.
pub fn quarantine(dir: &Path, name: &str, reason: &str, log: &Log) {
    let to = dir.join(format!("{name}.bad-{}", unix_secs()));
    match std::fs::rename(dir.join(name), &to) {
        Ok(()) => warn!(
            log,
            "{name} is unreadable ({reason}); moved it to {}",
            to.display()
        ),
        Err(e) => error!(
            log,
            "{name} is unreadable ({reason}) and could not be moved aside: {e}"
        ),
    }
}

/// The contents of `name`, or `None` if it is absent. A file that cannot be
/// read, or is larger than [`MAX_FILE`], is quarantined.
pub fn read(dir: &Path, name: &str, log: &Log) -> Option<Vec<u8>> {
    let path = dir.join(name);
    let mut file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => {
            quarantine(dir, name, &e.to_string(), log);
            return None;
        }
    };
    let mut bytes = Vec::new();
    match (&mut file).take(MAX_FILE + 1).read_to_end(&mut bytes) {
        Ok(n) if n as u64 > MAX_FILE => {
            drop(file);
            quarantine(dir, name, "larger than 1 MiB", log);
            None
        }
        Ok(_) => Some(bytes),
        Err(e) => {
            drop(file);
            quarantine(dir, name, &e.to_string(), log);
            None
        }
    }
}

/// Replaces `name` with `bytes` atomically.
pub fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    let tmp = dir.join(format!("{name}.tmp"));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, dir.join(name))
}

// ---- peer keys as hex ----

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

pub fn peer_from_hex(s: &str) -> Option<PeerId> {
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut key = [0u8; 32];
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(PeerId(key))
}

// ---- addresses.json ----

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AddressesFile {
    version: u32,
    peers: BTreeMap<String, String>,
}

pub fn addresses_to_json(addresses: &BTreeMap<PeerId, SocketAddr>) -> Vec<u8> {
    let file = AddressesFile {
        version: 1,
        peers: addresses
            .iter()
            .map(|(p, a)| (hex(&p.0), a.to_string()))
            .collect(),
    };
    serde_json::to_vec_pretty(&file).unwrap_or_default()
}

pub fn addresses_from_json(bytes: &[u8]) -> Result<BTreeMap<PeerId, SocketAddr>, String> {
    let file: AddressesFile = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if file.version != 1 {
        return Err(format!("unknown version {}", file.version));
    }
    file.peers
        .iter()
        .map(|(p, a)| {
            let peer = peer_from_hex(p).ok_or_else(|| format!("bad peer key {p:?}"))?;
            let addr = a.parse().map_err(|_| format!("bad address {a:?}"))?;
            Ok((peer, addr))
        })
        .collect()
}

// ---- layouts.json ----

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LayoutsFile {
    version: u32,
    sets: Vec<SetEntry>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetEntry {
    monitors: Vec<String>,
    sides: Vec<SideEntry>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SideEntry {
    monitor: String,
    side: String,
    peer: String,
}

pub fn side_name(side: Side) -> &'static str {
    match side {
        Side::Left => "left",
        Side::Right => "right",
        Side::Top => "top",
        Side::Bottom => "bottom",
    }
}

fn side_from_name(s: &str) -> Option<Side> {
    Some(match s {
        "left" => Side::Left,
        "right" => Side::Right,
        "top" => Side::Top,
        "bottom" => Side::Bottom,
        _ => return None,
    })
}

pub fn layouts_to_json(book: &LayoutBook) -> Vec<u8> {
    let file = LayoutsFile {
        version: 1,
        sets: book
            .iter()
            .map(|(key, layout)| SetEntry {
                monitors: key.0.iter().map(|m| m.0.clone()).collect(),
                sides: layout
                    .iter()
                    .map(|(monitor, side, peer)| SideEntry {
                        monitor: monitor.0.clone(),
                        side: side_name(side).to_owned(),
                        peer: hex(&peer.0),
                    })
                    .collect(),
            })
            .collect(),
    };
    serde_json::to_vec_pretty(&file).unwrap_or_default()
}

pub fn layouts_from_json(bytes: &[u8]) -> Result<LayoutBook, String> {
    let file: LayoutsFile = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if file.version != 1 {
        return Err(format!("unknown version {}", file.version));
    }
    let mut book = LayoutBook::new();
    for set in file.sets {
        let mut layout = Layout::new();
        for s in set.sides {
            let side = side_from_name(&s.side).ok_or_else(|| format!("bad side {:?}", s.side))?;
            let peer =
                peer_from_hex(&s.peer).ok_or_else(|| format!("bad peer key {:?}", s.peer))?;
            layout.set(MonitorId(s.monitor), side, peer);
        }
        let mut ids: Vec<MonitorId> = set.monitors.into_iter().map(MonitorId).collect();
        ids.sort();
        ids.dedup();
        book.set(MonitorSetKey(ids), layout);
    }
    Ok(book)
}

// ---- identity and machine ----

/// The machine name: `COMPUTERNAME`, cut at a character boundary to at most
/// 63 bytes, or `"Windows PC"` if it is unset or empty.
pub fn machine_name(computer_name: Option<String>) -> MachineName {
    let mut name = computer_name.unwrap_or_default().trim().to_owned();
    while name.len() > 63 {
        name.pop();
    }
    MachineName::new(name)
        .or_else(|| MachineName::new("Windows PC"))
        .expect("the fallback name is valid")
}

/// This machine's identity: loaded, or created and stored on first start.
/// A damaged identity is quarantined and replaced, which costs the desk;
/// that is logged as an error.
pub fn identity(dir: &Path, log: &Log) -> io::Result<Identity> {
    if let Some(bytes) = read(dir, IDENTITY, log) {
        match Identity::from_file(&bytes) {
            Ok(id) => return Ok(id),
            Err(e) => {
                quarantine(dir, IDENTITY, &e.to_string(), log);
                error!(
                    log,
                    "the identity was damaged; a new one is created and peers must pair again"
                );
            }
        }
    }
    let id = Identity::generate(&mut OsRng);
    write_atomic(dir, IDENTITY, &id.to_file()[..])?;
    info!(log, "created identity {}", &hex(&id.peer_id().0)[..8]);
    Ok(id)
}

/// Everything stored, ready for the node and the engine.
pub struct Stored {
    pub machine: Machine,
    pub addresses: BTreeMap<PeerId, SocketAddr>,
    pub layouts: LayoutBook,
}

/// Loads identity, trust store, addresses and layouts from `dir`.
pub fn load(dir: &Path, name: MachineName, log: &Log) -> io::Result<Stored> {
    let mut machine = Machine::new(identity(dir, log)?, name);
    if let Some(bytes) = read(dir, TRUST_STORE, log)
        && let Err(e) = machine.load_trust_store(&bytes)
    {
        quarantine(dir, TRUST_STORE, &e.to_string(), log);
    }
    let addresses = read(dir, ADDRESSES, log)
        .and_then(|b| match addresses_from_json(&b) {
            Ok(a) => Some(a),
            Err(e) => {
                quarantine(dir, ADDRESSES, &e, log);
                None
            }
        })
        .unwrap_or_default();
    let layouts = read(dir, LAYOUTS, log)
        .and_then(|b| match layouts_from_json(&b) {
            Ok(l) => Some(l),
            Err(e) => {
                quarantine(dir, LAYOUTS, &e, log);
                None
            }
        })
        .unwrap_or_default();
    Ok(Stored {
        machine,
        addresses,
        layouts,
    })
}

// ---- the store task ----

enum Queued {
    File(&'static str, Vec<u8>),
    Flush(oneshot::Sender<()>),
}

/// Queues writes to the store task. Cheap to clone.
#[derive(Clone)]
pub struct StoreHandle(mpsc::UnboundedSender<Queued>);

impl StoreHandle {
    /// Queues `bytes` as the new contents of `name`; only the newest
    /// contents queued for a file are written.
    pub fn write(&self, name: &'static str, bytes: Vec<u8>) {
        let _ = self.0.send(Queued::File(name, bytes));
    }

    /// Waits until everything queued so far is on disk.
    pub async fn flush(&self) {
        let (tx, rx) = oneshot::channel();
        if self.0.send(Queued::Flush(tx)).is_ok() {
            let _ = rx.await;
        }
    }
}

/// Starts the store task writing into `dir`.
pub fn start(dir: PathBuf, log: Log) -> StoreHandle {
    let (tx, mut rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(first) = rx.recv().await {
            let mut pending = BTreeMap::new();
            let mut flushes = Vec::new();
            let mut next = Some(first);
            while let Some(w) = next {
                match w {
                    Queued::File(name, bytes) => {
                        pending.insert(name, bytes);
                    }
                    Queued::Flush(done) => flushes.push(done),
                }
                next = rx.try_recv().ok();
            }
            let dir = dir.clone();
            let log = log.clone();
            let _ = tokio::task::spawn_blocking(move || {
                for (name, bytes) in pending {
                    if let Err(e) = write_atomic(&dir, name, &bytes) {
                        error!(log, "cannot write {name}: {e}");
                    }
                }
            })
            .await;
            for done in flushes {
                let _ = done.send(());
            }
        }
    });
    StoreHandle(tx)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn temp_dir(tag: &str) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("mouseflock-store-{}-{tag}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn peer(b: u8) -> PeerId {
        PeerId([b; 32])
    }

    fn bad_files(dir: &Path, name: &str) -> usize {
        std::fs::read_dir(dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(&format!("{name}.bad-"))
            })
            .count()
    }

    #[test]
    fn addresses_round_trip() {
        let mut a = BTreeMap::new();
        a.insert(peer(1), "192.168.1.5:47391".parse().unwrap());
        a.insert(peer(0xab), "10.0.0.2:5000".parse().unwrap());
        assert_eq!(addresses_from_json(&addresses_to_json(&a)).unwrap(), a);
    }

    #[test]
    fn layouts_round_trip_and_match_the_design_example() {
        let example = format!(
            r#"{{"version":1,"sets":[{{"monitors":["edid:DEL4321:77","path:\\\\?\\display#1"],
               "sides":[{{"monitor":"edid:DEL4321:77","side":"right","peer":"{}"}}]}}]}}"#,
            hex(&[7; 32])
        );
        let book = layouts_from_json(example.as_bytes()).unwrap();
        let (key, layout) = book.iter().next().unwrap();
        assert_eq!(key.0.len(), 2);
        assert_eq!(
            layout.peer(&MonitorId("edid:DEL4321:77".into()), Side::Right),
            Some(peer(7))
        );
        assert_eq!(layouts_from_json(&layouts_to_json(&book)).unwrap(), book);
    }

    #[test]
    fn bad_contents_are_rejected() {
        assert!(addresses_from_json(br#"{"version":2,"peers":{}}"#).is_err());
        assert!(addresses_from_json(br#"{"version":1,"peers":{"zz":"1.2.3.4:5"}}"#).is_err());
        assert!(addresses_from_json(b"{not json").is_err());
        assert!(layouts_from_json(br#"{"version":1,"sets":[{"monitors":[],"sides":[{"monitor":"m","side":"up","peer":"00"}]}]}"#).is_err());
        assert!(layouts_from_json(br#"{"version":1,"sets":[],"extra":1}"#).is_err());
    }

    #[test]
    fn unreadable_files_are_quarantined_and_load_as_absent() {
        let dir = temp_dir("quarantine");
        let log = Log::memory();
        std::fs::write(dir.join(LAYOUTS), b"{not json").unwrap();
        std::fs::write(dir.join(ADDRESSES), br#"{"version":9,"peers":{}}"#).unwrap();
        std::fs::write(dir.join(TRUST_STORE), b"MFTS\x09").unwrap();
        let stored = load(&dir, machine_name(Some("A".into())), &log).unwrap();
        assert!(stored.layouts.is_empty() && stored.addresses.is_empty());
        assert_eq!(stored.machine.desk().members().count(), 0);
        for name in [LAYOUTS, ADDRESSES, TRUST_STORE] {
            assert!(!dir.join(name).exists(), "{name}");
            assert_eq!(bad_files(&dir, name), 1, "{name}");
        }
        assert_eq!(
            log.lines()
                .iter()
                .filter(|l| l.contains("unreadable"))
                .count(),
            3
        );
    }

    #[test]
    fn oversized_file_is_quarantined() {
        let dir = temp_dir("big");
        std::fs::write(dir.join(LAYOUTS), vec![b' '; MAX_FILE as usize + 1]).unwrap();
        assert_eq!(read(&dir, LAYOUTS, &Log::memory()), None);
        assert_eq!(bad_files(&dir, LAYOUTS), 1);
    }

    #[test]
    fn leftover_tmp_does_not_replace_the_file() {
        let dir = temp_dir("tmp");
        write_atomic(&dir, LAYOUTS, b"good").unwrap();
        std::fs::write(dir.join(format!("{LAYOUTS}.tmp")), b"half").unwrap();
        assert_eq!(read(&dir, LAYOUTS, &Log::memory()).unwrap(), b"good");
        write_atomic(&dir, LAYOUTS, b"new").unwrap();
        assert_eq!(read(&dir, LAYOUTS, &Log::memory()).unwrap(), b"new");
    }

    #[test]
    fn replacing_never_writes_into_the_old_file() {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ | FILE_SHARE_DELETE: renames are allowed, writes
        // into the file are not.
        const READ_DELETE: u32 = 0x1 | 0x4;
        let dir = temp_dir("atomic");
        write_atomic(&dir, LAYOUTS, b"old").unwrap();
        let mut reader = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(READ_DELETE)
            .open(dir.join(LAYOUTS))
            .unwrap();
        write_atomic(&dir, LAYOUTS, b"new").unwrap();
        let mut seen = Vec::new();
        reader.read_to_end(&mut seen).unwrap();
        assert_eq!(seen, b"old");
        drop(reader);
        assert_eq!(std::fs::read(dir.join(LAYOUTS)).unwrap(), b"new");
    }

    #[tokio::test]
    async fn a_burst_ends_with_the_last_contents() {
        let dir = temp_dir("burst");
        let store = start(dir.clone(), Log::memory());
        for i in 0..100 {
            store.write(LAYOUTS, format!("{i}").into_bytes());
        }
        store.flush().await;
        assert_eq!(std::fs::read(dir.join(LAYOUTS)).unwrap(), b"99");
    }

    #[test]
    fn identity_is_created_once() {
        let dir = temp_dir("identity");
        let log = Log::memory();
        let first = identity(&dir, &log).unwrap().peer_id();
        assert_eq!(identity(&dir, &log).unwrap().peer_id(), first);
    }

    #[test]
    fn damaged_identity_is_replaced() {
        let dir = temp_dir("bad-identity");
        let log = Log::memory();
        std::fs::write(dir.join(IDENTITY), b"short").unwrap();
        identity(&dir, &log).unwrap();
        assert_eq!(bad_files(&dir, IDENTITY), 1);
        assert!(
            log.lines()
                .iter()
                .any(|l| l.contains("ERROR") && l.contains("pair again"))
        );
        assert_eq!(
            std::fs::read(dir.join(IDENTITY)).unwrap().len(),
            pairing::IDENTITY_FILE_LEN
        );
    }

    #[test]
    fn machine_names() {
        let long = "é".repeat(35); // 70 bytes
        let name = machine_name(Some(long));
        assert!(name.as_str().len() <= 63 && name.as_str().len() == 62);
        assert_eq!(machine_name(Some("LAPTOP-7".into())).as_str(), "LAPTOP-7");
        assert_eq!(machine_name(Some("  ".into())).as_str(), "Windows PC");
        assert_eq!(machine_name(None).as_str(), "Windows PC");
    }
}
