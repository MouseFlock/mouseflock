//! What every link shares: the `Hello` exchange, a frame reader that holds
//! at most one frame, the frame writer, and the stop signal.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::watch;

use crate::ProtocolError;
use crate::frame::{self, Decoded, HEADER_LEN, MAX_PAYLOAD};
use crate::message::{HELLO, Hello, Role};

/// How long an end waits for the other's `Hello`.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

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
pub(crate) struct FrameReader<R> {
    r: R,
    buf: Vec<u8>,
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    /// The next frame's type and payload, `Ok(None)` at the end of the
    /// stream, or an error.
    pub(crate) async fn next(&mut self) -> Result<Option<(u16, Vec<u8>)>, LinkError> {
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
pub(crate) async fn handshake<S: Stream>(
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
pub(crate) struct Shared {
    stop: watch::Sender<bool>,
    pub(crate) queued: AtomicUsize,
}

impl Shared {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            stop: watch::Sender::new(false),
            queued: AtomicUsize::new(0),
        })
    }

    pub(crate) fn stop(&self) {
        self.stop.send_replace(true);
    }

    pub(crate) fn stopped(&self) -> bool {
        *self.stop.borrow()
    }

    /// Resolves once the link is stopping.
    pub(crate) async fn wait(&self) {
        let mut rx = self.stop.subscribe();
        let _ = rx.wait_for(|stop| *stop).await;
    }
}

/// The link is closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Closed;

pub(crate) async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, bytes: &[u8]) -> bool {
    w.write_all(bytes).await.is_ok() && w.flush().await.is_ok()
}
