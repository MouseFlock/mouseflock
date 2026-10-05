//! Both ends of the agent link, as async connections over any byte stream
//! (design D3, D4).
//!
//! After the `Hello` exchange each end runs a reader task and a writer task.
//! The reader holds at most one frame of unread data and hands decoded
//! messages to an unbounded channel; the writer drains a queue, so sending
//! never waits. Any protocol error, or either side dropping its handle, ends
//! both tasks, and the receiver then yields `None`.

use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use model::{InputEvent, Millis, Origin, Screen};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::{mpsc, watch};

use crate::ProtocolError;
use crate::clock::ClockSync;
use crate::frame::{self, Decoded, HEADER_LEN, MAX_PAYLOAD};
use crate::message::{HELLO, Hello, Role, ToAgent, ToService};

/// How long an end waits for the other's `Hello`.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// How often the service end sends `Ping`.
pub const PING_EVERY: Duration = Duration::from_secs(10);

/// Most commands the service may queue for an agent that is not reading;
/// more closes the link.
pub const MAX_QUEUED: usize = 4_096;

/// Most agent events held while waiting for the first `Pong`; more closes
/// the link.
pub const MAX_HELD: usize = 10_000;

/// A byte stream an end can run on: a named pipe, or a duplex in tests.
pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> Stream for T {}

/// Why a link could not be set up.
#[derive(Debug)]
pub enum LinkError {
    Io(std::io::Error),
    /// No `Hello` within [`HANDSHAKE_TIMEOUT`].
    Timeout,
    /// The stream ended before `Hello`.
    Closed,
    Protocol(ProtocolError),
    /// The version ranges do not overlap.
    VersionMismatch {
        ours: Hello,
        theirs: Hello,
    },
    /// The other end is not the role expected on this link.
    WrongRole(Role),
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "pipe error: {e}"),
            Self::Timeout => f.write_str("no Hello within 5 s"),
            Self::Closed => f.write_str("the other end closed before Hello"),
            Self::Protocol(e) => write!(f, "protocol error: {e}"),
            Self::VersionMismatch { ours, theirs } => {
                let older = if theirs.max < ours.min {
                    "the other end"
                } else {
                    "this end"
                };
                write!(
                    f,
                    "IPC versions do not overlap: ours {}..={}, theirs {}..={}; {older} is older",
                    ours.min, ours.max, theirs.min, theirs.max
                )
            }
            Self::WrongRole(role) => write!(f, "unexpected role {role:?} on this link"),
        }
    }
}

impl std::error::Error for LinkError {}

impl From<std::io::Error> for LinkError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Reads frames one at a time, holding at most one frame of unread data.
struct FrameReader<R> {
    r: R,
    buf: Vec<u8>,
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    /// The next frame's type and payload, `Ok(None)` at the end of the
    /// stream, or an error.
    async fn next(&mut self) -> Result<Option<(u16, Vec<u8>)>, LinkError> {
        loop {
            if let Decoded::Frame { frame, consumed } =
                frame::decode(&self.buf).map_err(LinkError::Protocol)?
            {
                let out = (frame.ty, frame.payload.to_vec());
                self.buf.drain(..consumed);
                return Ok(Some(out));
            }
            // Never read past one whole frame.
            let room = HEADER_LEN
                .saturating_add(MAX_PAYLOAD)
                .saturating_sub(self.buf.len())
                .clamp(1, 4_096);
            let start = self.buf.len();
            self.buf.resize(start.saturating_add(room), 0);
            let n = self
                .r
                .read(self.buf.get_mut(start..).unwrap_or_default())
                .await?;
            self.buf.truncate(start.saturating_add(n));
            if n == 0 {
                return Ok(None);
            }
        }
    }
}

/// Exchanges `Hello`s; returns the agreed version and the reader holding
/// any bytes that followed the other end's `Hello`.
async fn handshake<S: Stream>(
    stream: S,
    mine: Role,
    expect: Role,
) -> Result<(u16, FrameReader<ReadHalf<S>>, WriteHalf<S>), LinkError> {
    let (r, mut w) = tokio::io::split(stream);
    let ours = Hello::this_release(mine);
    let mut out = Vec::new();
    ours.encode_frame(&mut out)
        .map_err(|_| LinkError::Protocol(ProtocolError::DuplicateHello))?;
    w.write_all(&out).await?;
    w.flush().await?;

    let mut reader = FrameReader { r, buf: Vec::new() };
    let first = tokio::time::timeout(HANDSHAKE_TIMEOUT, reader.next())
        .await
        .map_err(|_| LinkError::Timeout)??;
    let Some((ty, payload)) = first else {
        return Err(LinkError::Closed);
    };
    if ty != HELLO {
        return Err(LinkError::Protocol(ProtocolError::ExpectedHello { ty }));
    }
    let theirs = Hello::decode(&payload).map_err(LinkError::Protocol)?;
    let version = ours
        .agree(&theirs)
        .ok_or(LinkError::VersionMismatch { ours, theirs })?;
    if theirs.role != expect {
        return Err(LinkError::WrongRole(theirs.role));
    }
    Ok((version, reader, w))
}

/// Shared by an end's tasks: the stop signal and the outbound count.
struct Shared {
    stop: watch::Sender<bool>,
    queued: AtomicUsize,
}

impl Shared {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            stop: watch::Sender::new(false),
            queued: AtomicUsize::new(0),
        })
    }

    fn stop(&self) {
        self.stop.send_replace(true);
    }

    fn stopped(&self) -> bool {
        *self.stop.borrow()
    }

    /// Resolves once the link is stopping.
    async fn wait(&self) {
        let mut rx = self.stop.subscribe();
        let _ = rx.wait_for(|stop| *stop).await;
    }
}

/// The link is closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Closed;

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
}

/// Sends commands to an agent. Dropping it closes the link.
pub struct AgentHandle {
    tx: mpsc::UnboundedSender<ToAgent>,
    shared: Arc<Shared>,
}

impl AgentHandle {
    /// Queues a command. Never waits; more than [`MAX_QUEUED`] unsent
    /// commands closes the link.
    pub fn send(&self, cmd: ToAgent) -> Result<(), Closed> {
        if self.shared.stopped() {
            return Err(Closed);
        }
        let queued = self.shared.queued.fetch_add(1, Ordering::AcqRel);
        if queued >= MAX_QUEUED {
            self.shared.stop();
            return Err(Closed);
        }
        self.tx.send(cmd).map_err(|_| Closed)
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

async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, bytes: &[u8]) -> bool {
    w.write_all(bytes).await.is_ok() && w.flush().await.is_ok()
}

async fn service_writer<S: Stream>(
    mut w: WriteHalf<S>,
    mut commands: mpsc::UnboundedReceiver<ToAgent>,
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
            _ = ping.tick() => ToAgent::Ping(clock()),
        };
        let mut bytes = Vec::new();
        if message.encode_frame(&mut bytes).is_err() || !write_frame(&mut w, &bytes).await {
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
        let mut sync = sync.lock().unwrap_or_else(|e| e.into_inner());
        match message {
            ToService::Pong { service, agent } => sync.observe(service, agent, clock()),
            ToService::Input { at, input, origin } => {
                held.push_back(Held::Input { at, input, origin })
            }
            ToService::Screen(screen) => held.push_back(Held::Screen(screen)),
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

/// A session agent's end of its link.
pub struct AgentEnd;

impl AgentEnd {
    /// Exchanges `Hello` with the service and starts the link. `clock` is
    /// the agent's monotonic clock, read when answering `Ping`. The returned
    /// receiver yields `SetCapture`, `Inject` and `Warp`; `Ping` is answered
    /// here and never yielded.
    pub async fn connect<S, C>(
        stream: S,
        clock: C,
    ) -> Result<(ServiceHandle, mpsc::UnboundedReceiver<ToAgent>), LinkError>
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
    commands: mpsc::UnboundedSender<ToAgent>,
    out: mpsc::UnboundedSender<ToService>,
    shared: Arc<Shared>,
    clock: C,
) {
    loop {
        let next = tokio::select! {
            () = shared.wait() => break,
            next = reader.next() => next,
        };
        let Ok(Some((ty, payload))) = next else {
            break;
        };
        let Ok(message) = ToAgent::decode(ty, &payload, version) else {
            break;
        };
        let sent = match message {
            ToAgent::Ping(service) => out
                .send(ToService::Pong {
                    service,
                    agent: clock(),
                })
                .is_ok(),
            cmd => commands.send(cmd).is_ok(),
        };
        if !sent {
            break;
        }
    }
    shared.stop();
}
