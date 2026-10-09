//! File copies at the session agent (add-clipboard-files D3–D5).
//!
//! - **Reporting:** a copy of files is encoded as a manifest with `proto`'s
//!   rules, the same a receiver checks, and reported with its number on the
//!   link. The agent keeps the newest such copy, with where each entry is on
//!   disk, until another copy replaces it.
//! - **Serving:** asked for a file of that copy, the agent reads it from disk
//!   on a thread of its own and streams it to the service, one file at a
//!   time, within the link's window. A file it cannot read, or that is
//!   shorter than the copy said, is abandoned, so the paste fails rather
//!   than writing a short file.
//! - **Receiving:** a manifest the service sets is offered for paste; when a
//!   file manager reads a file, the agent asks the service for it and hands
//!   the bytes over as they arrive, acknowledging each.
//!
//! No file name or byte is logged.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self as std_mpsc, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use ipc::clipboard::NotShared;
use ipc::files::Credit;
use ipc::link::{Closed, FromService, ServiceHandle};
use ipc::message::ToService;
use proto::files::{FileEntry, Manifest, ManifestError, encode_manifest};
use tokio::sync::mpsc;
use win32_clipboard::Clipboard;
use win32_clipboard::files::{Entry, FileCopy, Source};

/// Bytes read from disk, and sent on, at a time.
const CHUNK: usize = 65_536;

/// How long a pasted file waits for the next of its bytes.
const WAIT: Duration = Duration::from_secs(10);

/// File copies in both directions, driven by the agent's link loop.
pub struct Files {
    reports: Arc<ServiceHandle>,
    /// The newest file copy reported, by its number on the link.
    held: Option<(u32, FileCopy)>,
    /// The window for the bytes this agent serves, across files.
    gate: Arc<Gate>,
    /// The file being served: its index and its stop flag.
    serving: Option<(u32, Arc<AtomicBool>)>,
    /// Files asked for while another was served, in order.
    queue: VecDeque<(u32, u32)>,
    /// A served file is done: start the next.
    done: mpsc::UnboundedSender<()>,
    incoming: Arc<Incoming>,
}

impl Files {
    /// The receiver yields once each time a served file ends; call
    /// [`Files::served`] then.
    pub fn new(reports: Arc<ServiceHandle>) -> (Self, mpsc::UnboundedReceiver<()>) {
        let (done, served) = mpsc::unbounded_channel();
        let incoming = Arc::new(Incoming {
            reports: reports.clone(),
            slot: Mutex::new(None),
            turn: Turn::default(),
        });
        let files = Self {
            reports,
            held: None,
            gate: Arc::new(Gate::default()),
            serving: None,
            queue: VecDeque::new(),
            done,
            incoming,
        };
        (files, served)
    }

    /// Reports a copy of files as a manifest, or as not shared if it breaks
    /// a file copy rule.
    pub fn report(&mut self, copy: FileCopy) -> Result<(), Closed> {
        let mut manifest = Vec::new();
        let entries = copy.entries.iter().map(|e| FileEntry {
            path: &e.path,
            is_dir: e.is_dir,
            size: e.size,
        });
        if let Err(e) = encode_manifest(entries, &mut manifest) {
            self.held = None;
            return self
                .reports
                .send(ToService::ClipboardNotShared(not_shared(e)));
        }
        self.held = self
            .reports
            .send_clipboard_files(&manifest)?
            .map(|number| (number, copy));
        Ok(())
    }

    /// Another copy replaced the file copy: it is no longer served.
    pub fn forget(&mut self) {
        self.held = None;
    }

    /// Offers a received file copy for paste. A manifest that breaks a rule
    /// leaves the clipboard as it was.
    pub fn set(&self, set: u32, manifest: &[u8], clipboard: Option<&Clipboard>) {
        let Ok(parsed) = Manifest::parse(manifest) else {
            eprintln!("deskhop-agent: received file copy refused");
            return;
        };
        let entries: Vec<Entry> = parsed
            .entries()
            .map(|e| Entry {
                path: e.path.to_owned(),
                is_dir: e.is_dir,
                size: e.size,
            })
            .collect();
        let sizes = entries.iter().map(|e| e.size).collect();
        let source = Arc::new(LinkSource {
            set,
            sizes,
            incoming: self.incoming.clone(),
        });
        if !clipboard.is_some_and(|c| c.set_files(entries, source)) {
            eprintln!("deskhop-agent: files cannot be offered on this desktop");
        }
    }

    /// Handles a file message from the service.
    pub fn on(&mut self, message: FromService) {
        match message {
            FromService::SendFile { copy, index } => {
                if self.serving.is_some() {
                    self.queue.push_back((copy, index));
                } else {
                    self.serve(copy, index);
                }
            }
            FromService::StopFile { index } => {
                self.queue.retain(|(_, i)| *i != index);
                if let Some((serving, stop)) = &self.serving
                    && *serving == index
                {
                    stop.store(true, Ordering::Release);
                }
            }
            FromService::FileAcked(n) => self.gate.acknowledged(n),
            FromService::FileStarted { len, .. } => self.incoming.route(Piece::Started(len)),
            FromService::FileBytes(bytes) => self.incoming.route(Piece::Bytes(bytes)),
            FromService::FileEnded { .. } => self.incoming.route(Piece::Ended),
            FromService::FileAbandoned { .. } => self.incoming.route(Piece::Abandoned),
            _ => {}
        }
    }

    /// A served file ended: serve the next one asked for.
    pub fn served(&mut self) {
        self.serving = None;
        while let Some((copy, index)) = self.queue.pop_front() {
            self.serve(copy, index);
            if self.serving.is_some() {
                break;
            }
        }
    }

    /// Starts serving file `index` of copy `copy`, if that is the copy held
    /// and the index names a file; otherwise answers nothing.
    fn serve(&mut self, copy: u32, index: u32) {
        let Some((held, files)) = &self.held else {
            return;
        };
        let Ok(i) = usize::try_from(index) else {
            return;
        };
        let (Some(entry), Some(path)) = (files.entries.get(i), files.sources.get(i)) else {
            return;
        };
        if *held != copy || entry.is_dir {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        self.serving = Some((index, stop.clone()));
        let server = Server {
            reports: self.reports.clone(),
            gate: self.gate.clone(),
            index,
            path: path.clone(),
            len: entry.size,
            stop,
        };
        let done = self.done.clone();
        std::thread::spawn(move || {
            if server.run().is_err() {
                let _ = server.reports.abandon_file(server.index);
            }
            let _ = done.send(());
        });
    }
}

/// Why a copy of files that breaks a rule is not shared.
fn not_shared(e: ManifestError) -> NotShared {
    match e {
        ManifestError::TooManyEntries | ManifestError::TooLarge | ManifestError::PathTooLong => {
            NotShared::TooLarge
        }
        _ => NotShared::NoText,
    }
}

// ---- serving -------------------------------------------------------------

/// The window for served bytes: how many the service has not acknowledged.
#[derive(Default)]
struct Gate {
    credit: Mutex<Credit>,
    changed: Condvar,
}

impl Gate {
    fn acknowledged(&self, n: u32) {
        if let Ok(mut credit) = self.credit.lock() {
            credit.acknowledged(n);
        }
        self.changed.notify_all();
    }

    /// Waits until `n` bytes may be sent; `false` if `stop` is set first.
    fn wait_room(&self, n: usize, stop: &AtomicBool) -> bool {
        let Ok(mut credit) = self.credit.lock() else {
            return false;
        };
        loop {
            if stop.load(Ordering::Acquire) {
                return false;
            }
            if credit.room() >= n as u64 {
                credit.sent(n);
                return true;
            }
            credit = match self
                .changed
                .wait_timeout(credit, Duration::from_millis(100))
            {
                Ok((credit, _)) => credit,
                Err(_) => return false,
            };
        }
    }
}

/// Streams one file of the held copy to the service.
struct Server {
    reports: Arc<ServiceHandle>,
    gate: Arc<Gate>,
    index: u32,
    path: PathBuf,
    len: u64,
    stop: Arc<AtomicBool>,
}

impl Server {
    /// Exactly `len` bytes, or an error to abandon the file.
    fn run(&self) -> Result<(), ()> {
        let mut file = File::open(&self.path).map_err(drop)?;
        self.reports
            .start_file(self.index, self.len)
            .map_err(drop)?;
        let mut buf = vec![0u8; CHUNK];
        let mut sent = 0u64;
        while sent < self.len {
            let want = usize::try_from(self.len - sent).map_or(CHUNK, |left| left.min(CHUNK));
            let n = file.read(&mut buf[..want]).map_err(drop)?;
            // Shorter on disk than the copy said: never pass it off as whole.
            if n == 0 || !self.gate.wait_room(n, &self.stop) {
                return Err(());
            }
            self.reports.file_chunk(&buf[..n]).map_err(drop)?;
            sent += n as u64;
        }
        Ok(())
    }
}

// ---- receiving -------------------------------------------------------------

/// A piece of the file being set, from the service.
enum Piece {
    Started(u64),
    Bytes(Vec<u8>),
    Ended,
    Abandoned,
}

/// Where the bytes of the file being set go: the pasted file reading them,
/// if one is. Shared by the link loop and the files thread.
struct Incoming {
    reports: Arc<ServiceHandle>,
    /// The fetch now receiving, by index.
    slot: Mutex<Option<std_mpsc::Sender<Piece>>>,
    /// One fetch at a time, as one file run at a time arrives.
    turn: Turn,
}

impl Incoming {
    /// Passes a piece to the fetch receiving, or acknowledges and drops it:
    /// every byte received is acknowledged, wanted or not.
    fn route(&self, piece: Piece) {
        let unclaimed = match self.slot.lock() {
            Ok(slot) => match slot.as_ref() {
                Some(fetch) => fetch.send(piece).err().map(|e| e.0),
                None => Some(piece),
            },
            Err(_) => Some(piece),
        };
        if let Some(Piece::Bytes(bytes)) = unclaimed {
            let _ = self.reports.file_consumed(len_u32(bytes.len()));
        }
    }
}

fn len_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// A turn that one fetch holds at a time.
#[derive(Default)]
struct Turn {
    busy: Mutex<bool>,
    freed: Condvar,
}

impl Turn {
    /// Waits up to [`WAIT`] for the turn.
    fn take(&self) -> bool {
        let Ok(busy) = self.busy.lock() else {
            return false;
        };
        match self.freed.wait_timeout_while(busy, WAIT, |busy| *busy) {
            Ok((mut busy, timeout)) if !timeout.timed_out() => {
                *busy = true;
                true
            }
            _ => false,
        }
    }

    fn give_back(&self) {
        if let Ok(mut busy) = self.busy.lock() {
            *busy = false;
        }
        self.freed.notify_one();
    }
}

/// A received file copy's files, fetched from the service when read.
struct LinkSource {
    set: u32,
    sizes: Vec<u64>,
    incoming: Arc<Incoming>,
}

impl Source for LinkSource {
    fn open(&self, index: usize) -> io::Result<Box<dyn Read + Send>> {
        let len = *self.sizes.get(index).ok_or(io::ErrorKind::NotFound)?;
        let index = u32::try_from(index).map_err(|_| io::ErrorKind::NotFound)?;
        Ok(Box::new(Fetch {
            set: self.set,
            index,
            len,
            incoming: self.incoming.clone(),
            state: FetchState::NotAsked,
            pending: Vec::new(),
            at: 0,
        }))
    }
}

enum FetchState {
    NotAsked,
    Receiving(std_mpsc::Receiver<Piece>),
    Ended,
    Failed,
}

/// One pasted file's bytes, asked for on its first read.
struct Fetch {
    set: u32,
    index: u32,
    len: u64,
    incoming: Arc<Incoming>,
    state: FetchState,
    /// Bytes received and not yet read.
    pending: Vec<u8>,
    at: usize,
}

impl Fetch {
    /// Takes the turn and asks the service for the file.
    fn ask(&mut self) -> io::Result<()> {
        if !self.incoming.turn.take() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let (tx, rx) = std_mpsc::channel();
        if let Ok(mut slot) = self.incoming.slot.lock() {
            *slot = Some(tx);
        }
        self.state = FetchState::Receiving(rx);
        if self
            .incoming
            .reports
            .want_file(self.set, self.index)
            .is_err()
        {
            self.finish(FetchState::Failed);
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        Ok(())
    }

    /// Ends this fetch's turn.
    fn finish(&mut self, state: FetchState) {
        let was_receiving = matches!(self.state, FetchState::Receiving(_));
        self.state = state;
        if was_receiving {
            if let Ok(mut slot) = self.incoming.slot.lock() {
                *slot = None;
            }
            self.incoming.turn.give_back();
        }
    }

    fn fail(&mut self, kind: io::ErrorKind) -> io::Error {
        if matches!(self.state, FetchState::Receiving(_)) {
            let _ = self.incoming.reports.unwant_file(self.index);
        }
        self.finish(FetchState::Failed);
        kind.into()
    }
}

impl Read for Fetch {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if matches!(self.state, FetchState::NotAsked) {
            self.ask()?;
        }
        loop {
            if self.at < self.pending.len() {
                let n = buf.len().min(self.pending.len() - self.at);
                buf[..n].copy_from_slice(&self.pending[self.at..self.at + n]);
                self.at += n;
                return Ok(n);
            }
            let piece = match &self.state {
                FetchState::Receiving(rx) => rx.recv_timeout(WAIT),
                FetchState::Ended => return Ok(0),
                FetchState::Failed | FetchState::NotAsked => {
                    return Err(io::ErrorKind::BrokenPipe.into());
                }
            };
            match piece {
                Ok(Piece::Started(len)) if len == self.len => {}
                Ok(Piece::Started(_)) => return Err(self.fail(io::ErrorKind::InvalidData)),
                Ok(Piece::Bytes(bytes)) => {
                    // Acknowledged as taken: the window then bounds what
                    // waits here.
                    let _ = self.incoming.reports.file_consumed(len_u32(bytes.len()));
                    self.pending = bytes;
                    self.at = 0;
                }
                Ok(Piece::Ended) => self.finish(FetchState::Ended),
                Ok(Piece::Abandoned) => {
                    self.finish(FetchState::Failed);
                    return Err(io::ErrorKind::ConnectionAborted.into());
                }
                Err(RecvTimeoutError::Timeout) => return Err(self.fail(io::ErrorKind::TimedOut)),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(self.fail(io::ErrorKind::BrokenPipe));
                }
            }
        }
    }
}

impl Drop for Fetch {
    /// A paste that stops reading part way gives the file up.
    fn drop(&mut self) {
        if matches!(self.state, FetchState::Receiving(_)) {
            let _ = self.fail(io::ErrorKind::Interrupted);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use ipc::files::FILE_WINDOW;
    use ipc::link::{AgentEnd, AgentHandle, AgentRole, FromAgent, ServiceEnd};
    use model::Millis;
    use tokio::sync::mpsc::UnboundedReceiver;

    use super::*;

    /// Both ends of a session agent's link, over an in-memory pipe.
    async fn linked() -> (
        AgentHandle,
        UnboundedReceiver<FromAgent>,
        ServiceHandle,
        UnboundedReceiver<FromService>,
    ) {
        let (a, b) = tokio::io::duplex(1 << 16);
        let (service, agent) = tokio::join!(
            ServiceEnd::accept(a, AgentRole::Session, || Millis(0)),
            AgentEnd::connect(b, AgentRole::Session, || Millis(0)),
        );
        let ((handle, events), (reports, commands)) = (service.unwrap(), agent.unwrap());
        (handle, events, reports, commands)
    }

    fn entry(path: &str, is_dir: bool, size: u64) -> Entry {
        Entry {
            path: path.into(),
            is_dir,
            size,
        }
    }

    /// The next event, skipping clock exchanges.
    async fn next(events: &mut UnboundedReceiver<FromAgent>) -> FromAgent {
        tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .expect("no event")
            .expect("link closed")
    }

    #[tokio::test]
    async fn a_file_copy_is_reported_as_its_manifest() {
        let (_handle, mut events, reports, _commands) = linked().await;
        let (mut files, _served) = Files::new(Arc::new(reports));
        let copy = FileCopy {
            entries: vec![entry("d", true, 0), entry("d/a.txt", false, 3)],
            sources: vec![PathBuf::from("x"), PathBuf::from("x/a.txt")],
        };
        files.report(copy).unwrap();
        let FromAgent::ClipboardFiles { copy, manifest } = next(&mut events).await else {
            panic!("not a manifest");
        };
        assert_eq!(copy, 1);
        let m = Manifest::parse(&manifest).unwrap();
        assert_eq!(m.count(), 2);
        assert_eq!(m.total(), 3);
        // A copy whose name Windows could not create anywhere is not shared.
        let bad = FileCopy {
            entries: vec![entry("con", false, 1)],
            sources: vec![PathBuf::from("con")],
        };
        files.report(bad).unwrap();
        assert_eq!(
            next(&mut events).await,
            FromAgent::Clipboard(ipc::clipboard::Copied::NotShared(NotShared::NoText))
        );
    }

    #[tokio::test]
    async fn a_requested_file_is_served_within_the_window() {
        let (handle, mut events, reports, mut commands) = linked().await;
        let (mut files, mut served) = Files::new(Arc::new(reports));
        let dir = std::env::temp_dir().join("deskhop-agent-serve");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let body: Vec<u8> = (0..3 * FILE_WINDOW as usize + 5).map(|i| i as u8).collect();
        std::fs::write(dir.join("big.bin"), &body).unwrap();
        files
            .report(FileCopy {
                entries: vec![entry("big.bin", false, body.len() as u64)],
                sources: vec![dir.join("big.bin")],
            })
            .unwrap();
        assert!(matches!(
            next(&mut events).await,
            FromAgent::ClipboardFiles { copy: 1, .. }
        ));
        // A stale copy number, or a directory index, is answered with nothing.
        files.on(FromService::SendFile { copy: 2, index: 0 });
        assert!(files.serving.is_none());
        handle.send_file(1, 0).unwrap();
        let request = tokio::time::timeout(Duration::from_secs(5), commands.recv())
            .await
            .unwrap()
            .unwrap();
        files.on(request);
        let mut got = Vec::new();
        loop {
            tokio::select! {
                e = next(&mut events) => match e {
                    FromAgent::FileStarted { index: 0, len } => assert_eq!(len, body.len() as u64),
                    FromAgent::FileBytes(b) => {
                        // Never past the window: the service acknowledges.
                        handle.ack_file(len_u32(b.len())).unwrap();
                        got.extend(b);
                    }
                    FromAgent::FileEnded { index: 0 } => break,
                    other => panic!("{other:?}"),
                },
                Some(ack) = commands.recv() => files.on(ack),
            }
        }
        assert!(got == body, "the file arrived changed");
        served.recv().await.unwrap();
        files.served();
        assert!(files.serving.is_none());
    }

    #[tokio::test]
    async fn a_file_that_shrank_is_abandoned() {
        let (handle, mut events, reports, mut commands) = linked().await;
        let (mut files, mut served) = Files::new(Arc::new(reports));
        let file = std::env::temp_dir().join("deskhop-agent-shrunk.bin");
        std::fs::write(&file, [1; 10]).unwrap();
        files
            .report(FileCopy {
                entries: vec![entry("shrunk.bin", false, 100)],
                sources: vec![file],
            })
            .unwrap();
        next(&mut events).await;
        handle.send_file(1, 0).unwrap();
        files.on(commands.recv().await.unwrap());
        let mut seen = Vec::new();
        loop {
            let e = next(&mut events).await;
            let end = matches!(e, FromAgent::FileAbandoned { .. });
            seen.push(e);
            if end {
                break;
            }
        }
        assert_eq!(
            seen.first(),
            Some(&FromAgent::FileStarted { index: 0, len: 100 })
        );
        assert_eq!(seen.last(), Some(&FromAgent::FileAbandoned { index: 0 }));
        served.recv().await.unwrap();
    }

    #[tokio::test]
    async fn a_bad_manifest_is_refused_and_the_link_stays_up() {
        let (_handle, mut events, reports, _commands) = linked().await;
        let reports = Arc::new(reports);
        let (files, _served) = Files::new(reports.clone());
        // A traversal path, written without the encoder's checks.
        let mut bad = 1u32.to_le_bytes().to_vec();
        bad.push(0);
        bad.extend_from_slice(&1u64.to_le_bytes());
        bad.extend_from_slice(&5u16.to_le_bytes());
        bad.extend_from_slice(b"../x1");
        files.set(1, &bad, None);
        files.set(2, b"not a manifest", None);
        reports.send_clipboard_text("still up").unwrap();
        assert_eq!(
            next(&mut events).await,
            FromAgent::Clipboard(ipc::clipboard::Copied::Text("still up".into()))
        );
    }

    /// The link loop's part, run on its own task: messages from the service
    /// go to `files`, until the test drops the sender.
    fn drive(mut files: Files, mut commands: UnboundedReceiver<FromService>) {
        tokio::spawn(async move {
            while let Some(c) = commands.recv().await {
                files.on(c);
            }
        });
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_pasted_file_asks_for_its_bytes_on_first_read() {
        let (handle, mut events, reports, commands) = linked().await;
        let (files, _served) = Files::new(Arc::new(reports));
        let source = LinkSource {
            set: 1,
            sizes: vec![0, 5],
            incoming: files.incoming.clone(),
        };
        drive(files, commands);
        let mut reader = source.open(1).unwrap();
        // Nothing is asked before the first read.
        let reading = std::thread::spawn(move || {
            let mut out = Vec::new();
            reader.read_to_end(&mut out).map(|_| out)
        });
        assert_eq!(
            next(&mut events).await,
            FromAgent::FileWanted { set: 1, index: 1 }
        );
        handle.start_file(1, 5).unwrap();
        handle.file_chunk(b"hel").unwrap();
        handle.file_chunk(b"lo").unwrap();
        assert_eq!(reading.join().unwrap().unwrap(), b"hello");
        // Each received byte is acknowledged.
        let mut consumed = 0;
        let deadline = Instant::now() + Duration::from_secs(5);
        while consumed < 5 && Instant::now() < deadline {
            if let FromAgent::FileConsumed(n) = next(&mut events).await {
                consumed += n;
            }
        }
        assert_eq!(consumed, 5);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_abandoned_file_fails_the_paste() {
        let (handle, mut events, reports, commands) = linked().await;
        let (files, _served) = Files::new(Arc::new(reports));
        let source = LinkSource {
            set: 1,
            sizes: vec![10],
            incoming: files.incoming.clone(),
        };
        drive(files, commands);
        let mut reader = source.open(0).unwrap();
        let reading = std::thread::spawn(move || {
            let mut out = Vec::new();
            reader.read_to_end(&mut out)
        });
        next(&mut events).await;
        handle.start_file(0, 10).unwrap();
        handle.file_chunk(b"abc").unwrap();
        handle.abandon_file(0).unwrap();
        let err = reading.join().unwrap().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ConnectionAborted);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_paste_given_up_part_way_unwants_the_file() {
        let (handle, mut events, reports, commands) = linked().await;
        let (files, _served) = Files::new(Arc::new(reports));
        let source = LinkSource {
            set: 1,
            sizes: vec![10],
            incoming: files.incoming.clone(),
        };
        drive(files, commands);
        let mut reader = source.open(0).unwrap();
        let reading = std::thread::spawn(move || {
            let mut two = [0u8; 2];
            reader.read_exact(&mut two).unwrap();
            drop(reader);
        });
        next(&mut events).await;
        handle.start_file(0, 10).unwrap();
        handle.file_chunk(b"abc").unwrap();
        reading.join().unwrap();
        loop {
            match next(&mut events).await {
                FromAgent::FileUnwanted { index: 0 } => break,
                FromAgent::FileConsumed(_) => {}
                other => panic!("{other:?}"),
            }
        }
        // The rest of the run, sent before the service heard, is still
        // acknowledged, so the window stays whole.
        handle.file_chunk(b"defghij").unwrap();
        loop {
            if let FromAgent::FileConsumed(7) = next(&mut events).await {
                break;
            }
        }
    }
}
