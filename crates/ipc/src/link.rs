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
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use model::{InputEvent, Millis, Origin, Screen};
use tokio::io::{AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::mpsc;

use crate::clipboard::{self, Copied, MAX_TEXT_LEN, NotShared, RunAssembler};
use crate::clock::ClockSync;
use crate::message::{
    CLIPBOARD_CHUNK, CLIPBOARD_NOT_SHARED, CLIPBOARD_TEXT, ClipboardToAgent, Role, SET_CLIPBOARD,
    SET_CLIPBOARD_CHUNK, ToAgent, ToAgentAny, ToService,
};

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
    Clipboard(Copied),
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
}

impl AgentHandle {
    /// Queues a command. Never waits; more than [`MAX_QUEUED`] unsent
    /// commands closes the link.
    pub fn send(&self, cmd: ToAgent) -> Result<(), Closed> {
        self.push(Outgoing::Command(cmd))
    }

    /// Queues `text` for the agent to put on the clipboard, as one run.
    /// Text over [`MAX_TEXT_LEN`] bytes is not sent. Each chunk counts
    /// towards [`MAX_QUEUED`].
    pub fn set_clipboard(&self, text: &str) -> Result<(), Closed> {
        let Some(len) = u32::try_from(text.len())
            .ok()
            .filter(|l| *l <= MAX_TEXT_LEN)
        else {
            return Ok(());
        };
        self.push(Outgoing::Clipboard(ClipboardToAgent::Set(len)))?;
        for piece in clipboard::chunks(text) {
            self.push(Outgoing::Clipboard(ClipboardToAgent::Chunk(piece.to_vec())))?;
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

/// The service's end of a session agent's link.
pub struct ServiceEnd;

impl ServiceEnd {
    /// Exchanges `Hello` with a session agent and starts the link. `clock`
    /// is the service's monotonic clock; every event time is converted to
    /// it.
    pub async fn accept<S, C>(
        stream: S,
        clock: C,
    ) -> Result<(AgentHandle, mpsc::UnboundedReceiver<FromAgent>), LinkError>
    where
        S: Stream,
        C: Fn() -> Millis + Send + Sync + 'static,
    {
        let (version, reader, writer) =
            handshake(stream, Role::Service, Role::SessionAgent).await?;
        let shared = Shared::new();
        let clock: Arc<dyn Fn() -> Millis + Send + Sync> = Arc::new(clock);
        let sync = Arc::new(Mutex::new(ClockSync::default()));
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        tokio::spawn(service_writer(
            writer,
            cmd_rx,
            shared.clone(),
            clock.clone(),
        ));
        tokio::spawn(service_reader(
            reader,
            version,
            ev_tx,
            shared.clone(),
            sync,
            clock,
        ));
        Ok((AgentHandle { tx: cmd_tx, shared }, ev_rx))
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
    Clipboard(Copied),
}

/// Applies one clipboard message to the run in progress; a complete copy is
/// returned. Any other message is `Ok(None)`.
fn agent_clipboard(
    runs: &mut RunAssembler,
    message: &ToService,
) -> Result<Option<Copied>, crate::ProtocolError> {
    Ok(match message {
        ToService::ClipboardText(len) => runs.start(CLIPBOARD_TEXT, *len)?.map(Copied::Text),
        ToService::ClipboardChunk(bytes) => runs.chunk(CLIPBOARD_CHUNK, bytes)?.map(Copied::Text),
        ToService::ClipboardNotShared(why) => {
            runs.not_in_run(CLIPBOARD_NOT_SHARED)?;
            Some(Copied::NotShared(*why))
        }
        _ => None,
    })
}

async fn service_reader<S: Stream>(
    mut reader: FrameReader<ReadHalf<S>>,
    version: u16,
    events: mpsc::UnboundedSender<FromAgent>,
    shared: Arc<Shared>,
    sync: Arc<Mutex<ClockSync>>,
    clock: Arc<dyn Fn() -> Millis + Send + Sync>,
) {
    let mut held: VecDeque<Held> = VecDeque::new();
    let mut runs = RunAssembler::default();
    loop {
        let next = tokio::select! {
            () = shared.wait() => break,
            next = reader.next() => next,
        };
        let Ok(Some((ty, payload))) = next else {
            break;
        };
        let Ok(message) = ToService::decode(ty, &payload, version) else {
            break;
        };
        let Ok(copied) = agent_clipboard(&mut runs, &message) else {
            break;
        };
        let mut sync = sync.lock().unwrap_or_else(|e| e.into_inner());
        match message {
            ToService::Pong { service, agent } => sync.observe(service, agent, clock()),
            ToService::Input { at, input, origin } => {
                held.push_back(Held::Input { at, input, origin })
            }
            ToService::Screen(screen) => held.push_back(Held::Screen(screen)),
            ToService::ClipboardText(_)
            | ToService::ClipboardChunk(_)
            | ToService::ClipboardNotShared(_) => {
                if let Some(copied) = copied {
                    held.push_back(Held::Clipboard(copied));
                }
            }
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
                    Held::Clipboard(c) => FromAgent::Clipboard(c),
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
}

impl ServiceHandle {
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
        for piece in clipboard::chunks(text) {
            self.send(ToService::ClipboardChunk(piece.to_vec()))?;
        }
        Ok(())
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
}

/// A session agent's end of its link.
pub struct AgentEnd;

impl AgentEnd {
    /// Exchanges `Hello` with the service and starts the link. `clock` is
    /// the agent's monotonic clock, read when answering `Ping`. The returned
    /// receiver yields `SetCapture`, `Inject`, `Warp` and complete
    /// `SetClipboard` runs; `Ping` is answered here and never yielded.
    pub async fn connect<S, C>(
        stream: S,
        clock: C,
    ) -> Result<(ServiceHandle, mpsc::UnboundedReceiver<FromService>), LinkError>
    where
        S: Stream,
        C: Fn() -> Millis + Send + Sync + 'static,
    {
        let (version, reader, writer) =
            handshake(stream, Role::SessionAgent, Role::Service).await?;
        let shared = Shared::new();
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        tokio::spawn(agent_writer(writer, out_rx, shared.clone()));
        tokio::spawn(agent_reader(
            reader,
            version,
            cmd_tx,
            out_tx.clone(),
            shared.clone(),
            clock,
        ));
        Ok((ServiceHandle { tx: out_tx, shared }, cmd_rx))
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

async fn agent_reader<S: Stream, C: Fn() -> Millis + Send + Sync + 'static>(
    mut reader: FrameReader<ReadHalf<S>>,
    version: u16,
    commands: mpsc::UnboundedSender<FromService>,
    out: mpsc::UnboundedSender<ToService>,
    shared: Arc<Shared>,
    clock: C,
) {
    let mut runs = RunAssembler::default();
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
                let done = match part {
                    ClipboardToAgent::Set(len) => runs.start(SET_CLIPBOARD, len),
                    ClipboardToAgent::Chunk(bytes) => runs.chunk(SET_CLIPBOARD_CHUNK, &bytes),
                };
                match done {
                    Ok(Some(text)) => commands.send(FromService::SetClipboard(text)).is_ok(),
                    Ok(None) => true,
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
