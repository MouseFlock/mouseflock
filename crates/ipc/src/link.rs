//! Both ends of the agent link and of the UI link, as async connections
//! over any byte stream (add-ipc design D3, D4; add-ui-link design D3).
//!
//! After the `Hello` exchange each end runs a reader task and a writer task.
//! The reader holds at most one frame of unread data and hands decoded
//! messages to an unbounded channel; the writer drains a queue, so sending
//! never waits. Any protocol error, or either side dropping its handle, ends
//! both tasks, and the receiver then yields `None`.

mod common;
mod ui;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use model::{InputEvent, Millis, Origin, Screen};
use tokio::io::{AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::mpsc;

use crate::clipboard::{
    self, Copied, Kind, MAX_IMAGE_LEN, MAX_TEXT_LEN, NotShared, PNG_SIGNATURE, Run, RunAssembler,
};
use crate::clock::ClockSync;
use crate::files::{FileRun, MAX_MANIFEST_LEN, Window};
use crate::message::{
    CLIPBOARD_CHUNK, CLIPBOARD_FILES, CLIPBOARD_IMAGE, CLIPBOARD_NOT_SHARED, CLIPBOARD_TEXT,
    ClipboardToAgent, FILE_CHUNK, FILE_CONTENT, Role, SET_CLIPBOARD, SET_CLIPBOARD_CHUNK,
    SET_CLIPBOARD_FILES, SET_CLIPBOARD_IMAGE, SET_FILE_CHUNK, SET_FILE_CONTENT, ToAgent,
    ToAgentAny, ToService, check_agent_role,
};

/// The first IPC version with image runs.
const IMAGES_SINCE: u16 = 2;

/// The first IPC version with file copies.
const FILES_SINCE: u16 = 3;

pub use common::{Closed, HANDSHAKE_TIMEOUT, LinkError, Stream};
pub use ui::{MAX_UI_QUEUED, ServiceUiEnd, ServiceUiHandle, UiEnd, UiHandle};

use common::{FrameReader, Shared, handshake, write_frame};
/// How often the service end sends `Ping`.
pub const PING_EVERY: Duration = Duration::from_secs(10);

/// Most commands the service may queue for an agent that is not reading;
/// more closes the link.
pub const MAX_QUEUED: usize = 4_096;

/// Most agent events held while waiting for the first `Pong`; more closes
/// the link.
pub const MAX_HELD: usize = 10_000;

/// Which agent a link is for. Each end checks the other's `Hello` against
/// it, and the service end checks the messages that only one kind may send.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentRole {
    /// `mouseflock-agent --session`: the user's desktop, with the clipboard.
    Session,
    /// `mouseflock-agent --winlogon`: the secure desktop, without a clipboard.
    Winlogon,
}

impl AgentRole {
    pub const fn role(self) -> Role {
        match self {
            Self::Session => Role::SessionAgent,
            Self::Winlogon => Role::WinlogonAgent,
        }
    }
}

// ---- service end ------------------------------------------------------------

/// What the service learns from an agent, with times on the service's clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FromAgent {
    Input {
        at: Millis,
        input: InputEvent,
        origin: Origin,
    },
    Screen(Screen),
    /// A copy on the agent's desktop, once its whole run has arrived.
    /// Session agent only.
    Clipboard(Copied),
    /// Whether the agent's desktop is the input desktop. Winlogon agent only.
    InputDesktop(bool),
    /// A file copy on the agent's desktop, once its whole manifest run has
    /// arrived: the manifest, checked here only for its length, and the
    /// copy's number on this link, from 1. Session agent only, version 3.
    ClipboardFiles {
        copy: u32,
        manifest: Vec<u8>,
    },
    /// The agent wants file `index` of the file copy set as number `set`.
    FileWanted {
        set: u32,
        index: u32,
    },
    /// The agent starts serving file `index`: `len` bytes follow.
    FileStarted {
        index: u32,
        len: u64,
    },
    /// Bytes of the file being served. Acknowledge every one with
    /// [`AgentHandle::ack_file`], passed on or discarded.
    FileBytes(Vec<u8>),
    /// The file being served is complete.
    FileEnded {
        index: u32,
    },
    /// The agent stopped serving file `index`; its run, if one started,
    /// has ended.
    FileAbandoned {
        index: u32,
    },
    /// The agent passed on this many more bytes of the file being set.
    FileConsumed(u32),
    /// The agent no longer wants file `index`, being set: stop sending it
    /// with [`AgentHandle::abandon_file`].
    FileUnwanted {
        index: u32,
    },
}

/// Why a copy was not queued for an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetClipboardError {
    Closed,
    /// A winlogon agent's link, which never carries the clipboard.
    NoClipboard,
    /// An image for an agent whose link agreed IPC version 1.
    NoImages,
    /// A file message for an agent whose link agreed a version before 3.
    NoFiles,
}

impl From<Closed> for SetClipboardError {
    fn from(_: Closed) -> Self {
        Self::Closed
    }
}

/// What the service's writer sends: a command, or part of a clipboard run.
enum Outgoing {
    Command(ToAgent),
    Clipboard(ClipboardToAgent),
}

/// Sends commands to an agent. Dropping it closes the link.
pub struct AgentHandle {
    tx: mpsc::UnboundedSender<Outgoing>,
    shared: Arc<Shared>,
    role: AgentRole,
    version: u16,
    /// File bytes from the agent not yet acknowledged.
    window: Arc<Window>,
    /// File copies set on this link so far.
    sets: AtomicU32,
}

impl AgentHandle {
    /// The kind of agent at the other end.
    pub fn role(&self) -> AgentRole {
        self.role
    }

    /// The IPC version the link agreed.
    pub fn version(&self) -> u16 {
        self.version
    }

    /// Queues a command. Never waits; more than [`MAX_QUEUED`] unsent
    /// commands closes the link.
    pub fn send(&self, cmd: ToAgent) -> Result<(), Closed> {
        self.push(Outgoing::Command(cmd))
    }

    /// Queues `text` for the agent to put on the clipboard, as one run.
    /// Text over [`MAX_TEXT_LEN`] bytes is not sent. Each chunk counts
    /// towards [`MAX_QUEUED`]. A winlogon agent's link refuses, sending
    /// nothing.
    pub fn set_clipboard(&self, text: &str) -> Result<(), SetClipboardError> {
        if self.role != AgentRole::Session {
            return Err(SetClipboardError::NoClipboard);
        }
        let Some(len) = u32::try_from(text.len())
            .ok()
            .filter(|l| *l <= MAX_TEXT_LEN)
        else {
            return Ok(());
        };
        self.push(Outgoing::Clipboard(ClipboardToAgent::Set(len)))?;
        for piece in clipboard::chunks(text.as_bytes()) {
            self.push(Outgoing::Clipboard(ClipboardToAgent::Chunk(piece.to_vec())))?;
        }
        Ok(())
    }

    /// Queues `png` for the agent to decode and put on the clipboard, as one
    /// run. An image over [`MAX_IMAGE_LEN`] bytes, or one that does not start
    /// with the PNG signature, is not sent. A winlogon agent's link, or a link
    /// that agreed IPC version 1, refuses, sending nothing.
    pub fn set_clipboard_image(&self, png: &[u8]) -> Result<(), SetClipboardError> {
        if self.role != AgentRole::Session {
            return Err(SetClipboardError::NoClipboard);
        }
        if self.version < IMAGES_SINCE {
            return Err(SetClipboardError::NoImages);
        }
        let Some(len) = u32::try_from(png.len())
            .ok()
            .filter(|l| *l <= MAX_IMAGE_LEN && png.starts_with(&PNG_SIGNATURE))
        else {
            return Ok(());
        };
        self.push(Outgoing::Clipboard(ClipboardToAgent::SetImage(len)))?;
        for piece in clipboard::chunks(png) {
            self.push(Outgoing::Clipboard(ClipboardToAgent::Chunk(piece.to_vec())))?;
        }
        Ok(())
    }

    /// Queues a file copy's manifest for the agent to offer for paste, as one
    /// run, and returns its number on this link. A manifest over the bound is
    /// not sent (`Ok(None)`). A winlogon agent's link, or one before IPC
    /// version 3, refuses, sending nothing.
    pub fn set_clipboard_files(&self, manifest: &[u8]) -> Result<Option<u32>, SetClipboardError> {
        self.files()?;
        let Some(len) = u32::try_from(manifest.len())
            .ok()
            .filter(|l| *l <= MAX_MANIFEST_LEN)
        else {
            return Ok(None);
        };
        self.push(Outgoing::Clipboard(ClipboardToAgent::SetFiles(len)))?;
        for piece in clipboard::chunks(manifest) {
            self.push(Outgoing::Clipboard(ClipboardToAgent::Chunk(piece.to_vec())))?;
        }
        let set = self.sets.fetch_add(1, Ordering::AcqRel).saturating_add(1);
        Ok(Some(set))
    }

    /// Asks the agent to serve file `index` of the file copy it reported as
    /// number `copy`.
    pub fn send_file(&self, copy: u32, index: u32) -> Result<(), SetClipboardError> {
        self.file_message(ClipboardToAgent::SendFile { copy, index })
    }

    /// Starts the wanted file `index`'s bytes: `len` of them follow with
    /// [`AgentHandle::file_chunk`], within the window the agent's
    /// `FileConsumed` reports open.
    pub fn start_file(&self, index: u32, len: u64) -> Result<(), SetClipboardError> {
        self.file_message(ClipboardToAgent::SetFileContent { index, len })
    }

    /// Queues bytes of the file being set, in chunks of at most one frame.
    pub fn file_chunk(&self, bytes: &[u8]) -> Result<(), SetClipboardError> {
        self.files()?;
        for piece in clipboard::chunks(bytes) {
            self.push(Outgoing::Clipboard(ClipboardToAgent::SetFileChunk(
                piece.to_vec(),
            )))?;
        }
        Ok(())
    }

    /// Tells the agent the file `index` being set will not complete, ending
    /// its run.
    pub fn abandon_file(&self, index: u32) -> Result<(), SetClipboardError> {
        self.file_message(ClipboardToAgent::AbandonFile { index })
    }

    /// Asks the agent to stop serving file `index`. It answers with
    /// `FileAbandoned` if a run had started.
    pub fn stop_file(&self, index: u32) -> Result<(), SetClipboardError> {
        self.file_message(ClipboardToAgent::StopFile { index })
    }

    /// Acknowledges `n` bytes of the file being served, passed on or
    /// discarded, opening the agent's window by as much.
    pub fn ack_file(&self, n: u32) -> Result<(), SetClipboardError> {
        self.files()?;
        self.window.acknowledge(n);
        self.push(Outgoing::Clipboard(ClipboardToAgent::FileAcked(n)))?;
        Ok(())
    }

    fn file_message(&self, message: ClipboardToAgent) -> Result<(), SetClipboardError> {
        self.files()?;
        self.push(Outgoing::Clipboard(message))?;
        Ok(())
    }

    /// Whether this link carries file copies.
    fn files(&self) -> Result<(), SetClipboardError> {
        if self.role != AgentRole::Session {
            return Err(SetClipboardError::NoClipboard);
        }
        if self.version < FILES_SINCE {
            return Err(SetClipboardError::NoFiles);
        }
        Ok(())
    }

    fn push(&self, message: Outgoing) -> Result<(), Closed> {
        if self.shared.stopped() {
            return Err(Closed);
        }
        let queued = self.shared.queued.fetch_add(1, Ordering::AcqRel);
        if queued >= MAX_QUEUED {
            self.shared.stop();
            return Err(Closed);
        }
        self.tx.send(message).map_err(|_| Closed)
    }

    /// Closes the link.
    pub fn close(&self) {
        self.shared.stop();
    }
}

impl Drop for AgentHandle {
    fn drop(&mut self) {
        self.shared.stop();
    }
}

/// The service's end of an agent's link.
pub struct ServiceEnd;

impl ServiceEnd {
    /// Exchanges `Hello` with an agent of `role`, refusing any other role,
    /// and starts the link. `clock` is the service's monotonic clock; every
    /// event time is converted to it.
    pub async fn accept<S, C>(
        stream: S,
        role: AgentRole,
        clock: C,
    ) -> Result<(AgentHandle, mpsc::UnboundedReceiver<FromAgent>), LinkError>
    where
        S: Stream,
        C: Fn() -> Millis + Send + Sync + 'static,
    {
        let (version, reader, writer) = handshake(stream, Role::Service, role.role()).await?;
        let shared = Shared::new();
        let clock: Arc<dyn Fn() -> Millis + Send + Sync> = Arc::new(clock);
        let sync = Arc::new(Mutex::new(ClockSync::default()));
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        let window = Arc::new(Window::default());
        tokio::spawn(service_writer(
            writer,
            cmd_rx,
            shared.clone(),
            clock.clone(),
        ));
        tokio::spawn(service_reader(
            reader,
            version,
            role,
            ev_tx,
            shared.clone(),
            sync,
            clock,
            window.clone(),
        ));
        Ok((
            AgentHandle {
                tx: cmd_tx,
                shared,
                role,
                version,
                window,
                sets: AtomicU32::new(0),
            },
            ev_rx,
        ))
    }
}

async fn service_writer<S: Stream>(
    mut w: WriteHalf<S>,
    mut commands: mpsc::UnboundedReceiver<Outgoing>,
    shared: Arc<Shared>,
    clock: Arc<dyn Fn() -> Millis + Send + Sync>,
) {
    let mut ping = tokio::time::interval(PING_EVERY);
    loop {
        let message = tokio::select! {
            () = shared.wait() => break,
            cmd = commands.recv() => match cmd {
                Some(cmd) => {
                    shared.queued.fetch_sub(1, Ordering::AcqRel);
                    cmd
                }
                None => break,
            },
            _ = ping.tick() => Outgoing::Command(ToAgent::Ping(clock())),
        };
        let mut bytes = Vec::new();
        let encoded = match &message {
            Outgoing::Command(cmd) => cmd.encode_frame(&mut bytes),
            Outgoing::Clipboard(part) => part.encode_frame(&mut bytes),
        };
        if encoded.is_err() || !write_frame(&mut w, &bytes).await {
            break;
        }
    }
    shared.stop();
    let _ = w.shutdown().await;
}

/// Agent events waiting for the first `Pong`.
enum Held {
    Input {
        at: Millis,
        input: InputEvent,
        origin: Origin,
    },
    Screen(Screen),
    InputDesktop(bool),
    /// A clipboard or file event, passed on as it is.
    Plain(FromAgent),
}

/// Clipboard and file state at the service's end of a session agent's link.
struct Incoming {
    runs: RunAssembler,
    files: FileRun,
    copies: u32,
    window: Arc<Window>,
}

impl Incoming {
    /// The events one clipboard or file message adds, once complete: a
    /// finished copy, a file run's start, bytes and end. Any other message
    /// adds none.
    fn events(&mut self, message: &mut ToService) -> Result<Vec<FromAgent>, crate::ProtocolError> {
        let mut out = Vec::new();
        match message {
            ToService::ClipboardText(len) => {
                let run = self.runs.start(CLIPBOARD_TEXT, Kind::Text, *len)?;
                self.finished(run, &mut out);
            }
            ToService::ClipboardImage(len) => {
                let run = self.runs.start(CLIPBOARD_IMAGE, Kind::Image, *len)?;
                self.finished(run, &mut out);
            }
            ToService::ClipboardFiles(len) => {
                let run = self.runs.start(CLIPBOARD_FILES, Kind::Files, *len)?;
                self.finished(run, &mut out);
            }
            ToService::ClipboardChunk(bytes) => {
                let run = self.runs.chunk(CLIPBOARD_CHUNK, bytes)?;
                self.finished(run, &mut out);
            }
            ToService::ClipboardNotShared(why) => {
                self.runs.not_in_run(CLIPBOARD_NOT_SHARED)?;
                out.push(FromAgent::Clipboard(Copied::NotShared(*why)));
            }
            ToService::FileWanted { set, index } => out.push(FromAgent::FileWanted {
                set: *set,
                index: *index,
            }),
            ToService::FileContent { index, len } => {
                let empty = self.files.start(FILE_CONTENT, *index, *len)?;
                out.push(FromAgent::FileStarted {
                    index: *index,
                    len: *len,
                });
                if empty {
                    out.push(FromAgent::FileEnded { index: *index });
                }
            }
            ToService::FileChunk(bytes) => {
                self.window.receive(FILE_CHUNK, bytes.len())?;
                let done = self.files.chunk(FILE_CHUNK, bytes.len())?;
                out.push(FromAgent::FileBytes(std::mem::take(bytes)));
                if let Some(index) = done {
                    out.push(FromAgent::FileEnded { index });
                }
            }
            ToService::FileAbandoned { index } => {
                self.files.abandon(*index);
                out.push(FromAgent::FileAbandoned { index: *index });
            }
            ToService::FileConsumed(n) => out.push(FromAgent::FileConsumed(*n)),
            ToService::FileUnwanted { index } => {
                out.push(FromAgent::FileUnwanted { index: *index });
            }
            _ => {}
        }
        Ok(out)
    }

    fn finished(&mut self, run: Option<Run>, out: &mut Vec<FromAgent>) {
        out.push(match run {
            None => return,
            Some(Run::Text(t)) => FromAgent::Clipboard(Copied::Text(t)),
            Some(Run::Image(b)) => FromAgent::Clipboard(Copied::Image(b)),
            Some(Run::Files(manifest)) => {
                self.copies = self.copies.saturating_add(1);
                FromAgent::ClipboardFiles {
                    copy: self.copies,
                    manifest,
                }
            }
        });
    }
}

#[allow(clippy::too_many_arguments)]
async fn service_reader<S: Stream>(
    mut reader: FrameReader<ReadHalf<S>>,
    version: u16,
    role: AgentRole,
    events: mpsc::UnboundedSender<FromAgent>,
    shared: Arc<Shared>,
    sync: Arc<Mutex<ClockSync>>,
    clock: Arc<dyn Fn() -> Millis + Send + Sync>,
    window: Arc<Window>,
) {
    let mut held: VecDeque<Held> = VecDeque::new();
    let mut incoming = Incoming {
        runs: RunAssembler::default(),
        files: FileRun::default(),
        copies: 0,
        window,
    };
    loop {
        let next = tokio::select! {
            () = shared.wait() => break,
            next = reader.next() => next,
        };
        let Ok(Some((ty, payload))) = next else {
            break;
        };
        let Ok(mut message) = ToService::decode(ty, &payload, version) else {
            break;
        };
        if check_agent_role(ty, role.role()).is_err() {
            break;
        }
        let Ok(added) = incoming.events(&mut message) else {
            break;
        };
        held.extend(added.into_iter().map(Held::Plain));
        let mut sync = sync.lock().unwrap_or_else(|e| e.into_inner());
        match message {
            ToService::Pong { service, agent } => sync.observe(service, agent, clock()),
            ToService::Input { at, input, origin } => {
                held.push_back(Held::Input { at, input, origin })
            }
            ToService::Screen(screen) => held.push_back(Held::Screen(screen)),
            ToService::InputDesktop(active) => held.push_back(Held::InputDesktop(active)),
            _ => {}
        }
        if sync.ready() {
            while let Some(h) = held.pop_front() {
                let event = match h {
                    Held::Input { at, input, origin } => FromAgent::Input {
                        at: sync.to_service(at).unwrap_or(at),
                        input,
                        origin,
                    },
                    Held::Screen(s) => FromAgent::Screen(s),
                    Held::InputDesktop(active) => FromAgent::InputDesktop(active),
                    Held::Plain(event) => event,
                };
                if events.send(event).is_err() {
                    shared.stop();
                }
            }
        } else if held.len() > MAX_HELD {
            break;
        }
    }
    shared.stop();
}

// ---- agent end ----------------------------------------------------------------

/// Sends reports to the service. Dropping it closes the link.
pub struct ServiceHandle {
    tx: mpsc::UnboundedSender<ToService>,
    shared: Arc<Shared>,
    version: u16,
    /// File bytes from the service not yet acknowledged.
    window: Arc<Window>,
    /// File copies reported on this link so far.
    copies: AtomicU32,
}

impl ServiceHandle {
    /// The IPC version the link agreed.
    pub fn version(&self) -> u16 {
        self.version
    }

    /// Queues a report. Never waits.
    pub fn send(&self, message: ToService) -> Result<(), Closed> {
        if self.shared.stopped() {
            return Err(Closed);
        }
        self.tx.send(message).map_err(|_| Closed)
    }

    /// Queues a copy of `text` as one run. Text over [`MAX_TEXT_LEN`] bytes
    /// is reported as too large instead.
    pub fn send_clipboard_text(&self, text: &str) -> Result<(), Closed> {
        let Some(len) = u32::try_from(text.len())
            .ok()
            .filter(|l| *l <= MAX_TEXT_LEN)
        else {
            return self.send(ToService::ClipboardNotShared(NotShared::TooLarge));
        };
        self.send(ToService::ClipboardText(len))?;
        for piece in clipboard::chunks(text.as_bytes()) {
            self.send(ToService::ClipboardChunk(piece.to_vec()))?;
        }
        Ok(())
    }

    /// Queues a copied image, as one PNG, as one run. On a link that agreed
    /// IPC version 1 it is reported as neither text nor an image, as a
    /// version 1 agent would report it; over [`MAX_IMAGE_LEN`] bytes, as too
    /// large. Bytes that do not start with the PNG signature are reported as
    /// neither text nor an image.
    pub fn send_clipboard_image(&self, png: &[u8]) -> Result<(), Closed> {
        if self.version < IMAGES_SINCE || !png.starts_with(&PNG_SIGNATURE) {
            return self.send(ToService::ClipboardNotShared(NotShared::NoText));
        }
        let Some(len) = u32::try_from(png.len())
            .ok()
            .filter(|l| *l <= MAX_IMAGE_LEN)
        else {
            return self.send(ToService::ClipboardNotShared(NotShared::TooLarge));
        };
        self.send(ToService::ClipboardImage(len))?;
        for piece in clipboard::chunks(png) {
            self.send(ToService::ClipboardChunk(piece.to_vec()))?;
        }
        Ok(())
    }

    /// Queues a file copy's manifest as one run, and returns the copy's
    /// number on this link. On a link before IPC version 3 it is reported as
    /// neither text nor an image, as an older agent would report it; over the
    /// manifest bound, as too large; either way `Ok(None)`.
    pub fn send_clipboard_files(&self, manifest: &[u8]) -> Result<Option<u32>, Closed> {
        if self.version < FILES_SINCE {
            self.send(ToService::ClipboardNotShared(NotShared::NoText))?;
            return Ok(None);
        }
        let Some(len) = u32::try_from(manifest.len())
            .ok()
            .filter(|l| *l <= MAX_MANIFEST_LEN)
        else {
            self.send(ToService::ClipboardNotShared(NotShared::TooLarge))?;
            return Ok(None);
        };
        self.send(ToService::ClipboardFiles(len))?;
        for piece in clipboard::chunks(manifest) {
            self.send(ToService::ClipboardChunk(piece.to_vec()))?;
        }
        Ok(Some(
            self.copies.fetch_add(1, Ordering::AcqRel).saturating_add(1),
        ))
    }

    /// Asks for file `index` of the file copy set as number `set`.
    pub fn want_file(&self, set: u32, index: u32) -> Result<(), Closed> {
        self.send_file_message(ToService::FileWanted { set, index })
    }

    /// Starts serving file `index`: `len` bytes follow with
    /// [`ServiceHandle::file_chunk`], within the window the service's
    /// `FileAcked` reports open.
    pub fn start_file(&self, index: u32, len: u64) -> Result<(), Closed> {
        self.send_file_message(ToService::FileContent { index, len })
    }

    /// Queues bytes of the file being served, in chunks of at most one frame.
    pub fn file_chunk(&self, bytes: &[u8]) -> Result<(), Closed> {
        for piece in clipboard::chunks(bytes) {
            self.send_file_message(ToService::FileChunk(piece.to_vec()))?;
        }
        Ok(())
    }

    /// Stops serving file `index`, ending its run.
    pub fn abandon_file(&self, index: u32) -> Result<(), Closed> {
        self.send_file_message(ToService::FileAbandoned { index })
    }

    /// Says file `index`, being set, is no longer wanted. The service
    /// answers with `AbandonFile` if its run had started.
    pub fn unwant_file(&self, index: u32) -> Result<(), Closed> {
        self.send_file_message(ToService::FileUnwanted { index })
    }

    /// Acknowledges `n` bytes of the file being set, passed on or discarded,
    /// opening the service's window by as much.
    pub fn file_consumed(&self, n: u32) -> Result<(), Closed> {
        self.window.acknowledge(n);
        self.send_file_message(ToService::FileConsumed(n))
    }

    /// A file message, which a link before IPC version 3 never carries: it is
    /// dropped there.
    fn send_file_message(&self, message: ToService) -> Result<(), Closed> {
        if self.version < FILES_SINCE {
            return if self.shared.stopped() {
                Err(Closed)
            } else {
                Ok(())
            };
        }
        self.send(message)
    }

    /// Closes the link.
    pub fn close(&self) {
        self.shared.stop();
    }
}

impl Drop for ServiceHandle {
    fn drop(&mut self) {
        self.shared.stop();
    }
}

/// What an agent is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FromService {
    /// `SetCapture`, `Inject` or `Warp`.
    Command(ToAgent),
    /// Put this text on the clipboard; its whole run has arrived.
    SetClipboard(String),
    /// Decode this PNG and put the image on the clipboard; its whole run has
    /// arrived. Its bytes are checked only for length and signature.
    SetClipboardImage(Vec<u8>),
    /// Offer this file copy for paste; its whole manifest run has arrived,
    /// checked only for its length. `set` is its number on this link, from 1.
    /// Version 3.
    SetClipboardFiles { set: u32, manifest: Vec<u8> },
    /// Serve file `index` of the file copy reported as number `copy`.
    SendFile { copy: u32, index: u32 },
    /// The wanted file `index` starts: `len` bytes follow.
    FileStarted { index: u32, len: u64 },
    /// Bytes of the file being set. Acknowledge every one with
    /// [`ServiceHandle::file_consumed`], passed on or discarded.
    FileBytes(Vec<u8>),
    /// The file being set is complete.
    FileEnded { index: u32 },
    /// The file `index` being set will not complete; its run, if one
    /// started, has ended.
    FileAbandoned { index: u32 },
    /// The service passed on this many more bytes of the file being served.
    FileAcked(u32),
    /// Stop serving file `index`, with [`ServiceHandle::abandon_file`].
    StopFile { index: u32 },
}

/// An agent's end of its link.
pub struct AgentEnd;

impl AgentEnd {
    /// Exchanges `Hello` with the service as an agent of `role` and starts
    /// the link. `clock` is the agent's monotonic clock, read when answering
    /// `Ping`. The returned receiver yields `SetCapture`, `Inject`, `Warp`
    /// and complete `SetClipboard` runs; `Ping` is answered here and never
    /// yielded.
    pub async fn connect<S, C>(
        stream: S,
        role: AgentRole,
        clock: C,
    ) -> Result<(ServiceHandle, mpsc::UnboundedReceiver<FromService>), LinkError>
    where
        S: Stream,
        C: Fn() -> Millis + Send + Sync + 'static,
    {
        let (version, reader, writer) = handshake(stream, role.role(), Role::Service).await?;
        let shared = Shared::new();
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let window = Arc::new(Window::default());
        tokio::spawn(agent_writer(writer, out_rx, shared.clone()));
        tokio::spawn(agent_reader(
            reader,
            version,
            cmd_tx,
            out_tx.clone(),
            shared.clone(),
            clock,
            window.clone(),
        ));
        Ok((
            ServiceHandle {
                tx: out_tx,
                shared,
                version,
                window,
                copies: AtomicU32::new(0),
            },
            cmd_rx,
        ))
    }
}

async fn agent_writer<S: Stream>(
    mut w: WriteHalf<S>,
    mut out: mpsc::UnboundedReceiver<ToService>,
    shared: Arc<Shared>,
) {
    loop {
        let message = tokio::select! {
            () = shared.wait() => break,
            m = out.recv() => match m {
                Some(m) => m,
                None => break,
            },
        };
        let mut bytes = Vec::new();
        // A report the protocol cannot carry, such as an oversized screen,
        // is skipped rather than ending the link.
        if message.encode_frame(&mut bytes).is_err() {
            continue;
        }
        if !write_frame(&mut w, &bytes).await {
            break;
        }
    }
    shared.stop();
    let _ = w.shutdown().await;
}

/// The events one clipboard or file message from the service adds.
fn service_events(
    runs: &mut RunAssembler,
    files: &mut FileRun,
    sets: &mut u32,
    window: &Window,
    part: ClipboardToAgent,
) -> Result<Vec<FromService>, crate::ProtocolError> {
    let run = match part {
        ClipboardToAgent::Set(len) => runs.start(SET_CLIPBOARD, Kind::Text, len)?,
        ClipboardToAgent::SetImage(len) => runs.start(SET_CLIPBOARD_IMAGE, Kind::Image, len)?,
        ClipboardToAgent::SetFiles(len) => runs.start(SET_CLIPBOARD_FILES, Kind::Files, len)?,
        ClipboardToAgent::Chunk(bytes) => runs.chunk(SET_CLIPBOARD_CHUNK, &bytes)?,
        ClipboardToAgent::SendFile { copy, index } => {
            return Ok(vec![FromService::SendFile { copy, index }]);
        }
        ClipboardToAgent::SetFileContent { index, len } => {
            let empty = files.start(SET_FILE_CONTENT, index, len)?;
            let mut out = vec![FromService::FileStarted { index, len }];
            if empty {
                out.push(FromService::FileEnded { index });
            }
            return Ok(out);
        }
        ClipboardToAgent::SetFileChunk(bytes) => {
            window.receive(SET_FILE_CHUNK, bytes.len())?;
            let done = files.chunk(SET_FILE_CHUNK, bytes.len())?;
            let mut out = vec![FromService::FileBytes(bytes)];
            if let Some(index) = done {
                out.push(FromService::FileEnded { index });
            }
            return Ok(out);
        }
        ClipboardToAgent::AbandonFile { index } => {
            files.abandon(index);
            return Ok(vec![FromService::FileAbandoned { index }]);
        }
        ClipboardToAgent::FileAcked(n) => return Ok(vec![FromService::FileAcked(n)]),
        ClipboardToAgent::StopFile { index } => return Ok(vec![FromService::StopFile { index }]),
    };
    Ok(match run {
        None => Vec::new(),
        Some(Run::Text(text)) => vec![FromService::SetClipboard(text)],
        Some(Run::Image(png)) => vec![FromService::SetClipboardImage(png)],
        Some(Run::Files(manifest)) => {
            *sets = sets.saturating_add(1);
            vec![FromService::SetClipboardFiles {
                set: *sets,
                manifest,
            }]
        }
    })
}

#[allow(clippy::too_many_arguments)]
async fn agent_reader<S: Stream, C: Fn() -> Millis + Send + Sync + 'static>(
    mut reader: FrameReader<ReadHalf<S>>,
    version: u16,
    commands: mpsc::UnboundedSender<FromService>,
    out: mpsc::UnboundedSender<ToService>,
    shared: Arc<Shared>,
    clock: C,
    window: Arc<Window>,
) {
    let mut runs = RunAssembler::default();
    let mut files = FileRun::default();
    let mut sets = 0u32;
    loop {
        let next = tokio::select! {
            () = shared.wait() => break,
            next = reader.next() => next,
        };
        let Ok(Some((ty, payload))) = next else {
            break;
        };
        let Ok(message) = ToAgentAny::decode(ty, &payload, version) else {
            break;
        };
        let sent = match message {
            ToAgentAny::Command(ToAgent::Ping(service)) => out
                .send(ToService::Pong {
                    service,
                    agent: clock(),
                })
                .is_ok(),
            ToAgentAny::Command(cmd) => commands.send(FromService::Command(cmd)).is_ok(),
            ToAgentAny::Clipboard(part) => {
                match service_events(&mut runs, &mut files, &mut sets, &window, part) {
                    Ok(events) => events.into_iter().all(|e| commands.send(e).is_ok()),
                    Err(_) => false,
                }
            }
        };
        if !sent {
            break;
        }
    }
    shared.stop();
}
