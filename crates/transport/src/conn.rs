//! One task per connection: `Hello` and frames on the control stream, and
//! engine messages, clipboard messages and motion datagrams once the peer's
//! records are in. Clipboard copies and files' contents travel on
//! unidirectional streams of their own, each read or written by a task of its
//! own, so they never queue ahead of input on the control stream. Every decision about trust belongs to the
//! node; this task moves bytes and keeps the motion totals (design D1, D4;
//! input-forwarding D5; text-clipboard D3).

use std::sync::Arc;
use std::time::Duration;

use model::{InputAction, PeerMessage};
use pairing::ConnId;
use proto::CloseReason;
use proto::clipboard::{self, ClipKind, ClipboardMessage, TransferReader, TransferStep};
use proto::files::{self, FileContentRequest, Manifest};
use proto::frame::{self, Decoded, MAX_FRAME_PAYLOAD};
use proto::hello::Hello;
use proto::input::{InputMessage, Mark};
use proto::registry;
use proto::session::{ControlEvent, DatagramEvent, Session, StreamEvent};
use tokio::sync::{Semaphore, mpsc, oneshot};
use tokio::time::Instant;

use crate::forward::{self, Inbound, Outbound};
use crate::motion::{Receiver, Sender};
use crate::node::{Contents, FileChunk, Internal};

/// How often a receiving clipboard stream reports progress, at most.
const PROGRESS_EVERY: Duration = Duration::from_secs(1);

/// What the node asks a connection task to do.
#[derive(Debug)]
pub(crate) enum Out {
    /// Write this encoded frame on the control stream.
    Frame(Vec<u8>),
    /// Send an engine message. The node sends these only once the peer is up.
    Peer(PeerMessage),
    /// Send a copy with this ID on a clipboard stream of its own. The node
    /// sends these only once the peer is up, and an image only on a
    /// connection that agreed version 2 or later.
    Transfer(u64, Contents),
    /// Send one file's contents, `len` bytes read from `body`, on a file
    /// content stream of its own. Only on a connection that defines files.
    File {
        clip: u64,
        index: u32,
        len: u64,
        body: mpsc::Receiver<Vec<u8>>,
    },
    /// Let what was written reach the peer, then close with this reason.
    Close(CloseReason),
}

/// How a connection ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cause {
    /// This side closed it.
    Local,
    /// The peer closed it with this reason (`None`: a code this release does
    /// not know).
    Remote(Option<CloseReason>),
    /// Nothing heard within the idle timeout.
    TimedOut,
    /// A transport error, a reset, or the endpoint going away.
    Failed,
}

impl Cause {
    pub(crate) fn from_error(e: &quinn::ConnectionError) -> Self {
        match e {
            quinn::ConnectionError::ApplicationClosed(c) => {
                Self::Remote(CloseReason::from_code(c.error_code.into_inner()))
            }
            quinn::ConnectionError::TimedOut => Self::TimedOut,
            quinn::ConnectionError::LocallyClosed => Self::Local,
            _ => Self::Failed,
        }
    }

    /// The peer's close reason, if it gave one this release knows.
    pub(crate) fn reason(self) -> Option<CloseReason> {
        match self {
            Self::Remote(reason) => reason,
            _ => None,
        }
    }
}

/// Closes `connection` at once with `reason`.
pub(crate) fn close(connection: &quinn::Connection, reason: CloseReason) {
    // Close codes are small; they always fit a VarInt.
    let code = quinn::VarInt::from_u64(reason.code()).unwrap_or(quinn::VarInt::from_u32(1));
    connection.close(code, b"");
}

/// How long a close waits for written frames to be acknowledged.
const FLUSH: Duration = Duration::from_millis(500);

/// How long after the peer finishes the control stream its close may take.
const LINGER: Duration = Duration::from_secs(2);

/// Engine messages and motion on one connection.
struct Forwarding {
    /// A member connection; engine messages are never allowed on pairing
    /// connections.
    member: bool,
    /// The peer's `RecordsDone` has arrived on the control stream. A peer
    /// counts itself up only after sending it, so engine messages before it
    /// break the protocol.
    records_in: bool,
    sender: Sender,
    receiver: Receiver,
}

impl Forwarding {
    fn open(&self) -> bool {
        self.member && self.records_in
    }

    /// Passes a received engine message to the node, any movement its mark
    /// carries first.
    fn report(&mut self, id: ConnId, inbound: Inbound, node: &mpsc::UnboundedSender<Internal>) {
        let (mark, msg) = match inbound {
            Inbound::Plain(msg) => (None, Some(msg)),
            Inbound::Marked(mark, msg) => (Some(mark), Some(msg)),
            Inbound::Motion(mark) => (Some(mark), None),
        };
        if let Some((dx, dy)) = mark.and_then(|m| self.receiver.apply(m)) {
            let motion = PeerMessage::Input(InputAction::Motion { dx, dy });
            let _ = node.send(Internal::Peer(id, motion));
        }
        if let Some(msg) = msg {
            let _ = node.send(Internal::Peer(id, msg));
        }
    }
}

/// Sends `mark` as a `Motion` datagram. A full send buffer drops it; the
/// next datagram or mark carries the same totals.
fn send_motion(connection: &quinn::Connection, mark: Mark) {
    let mut out = Vec::new();
    if mark.encode_datagram(&mut out).is_ok() {
        let _ = connection.send_datagram(out.into());
    }
}

/// Runs one connection until it closes, then reports how.
pub(crate) async fn run(
    id: ConnId,
    connection: quinn::Connection,
    dialer: bool,
    member: bool,
    hello: Hello,
    mut out: mpsc::UnboundedReceiver<Out>,
    node: mpsc::UnboundedSender<Internal>,
) {
    // A peer may never open the control stream; the node can still close it.
    let open = async {
        if dialer {
            connection.open_bi().await
        } else {
            connection.accept_bi().await
        }
    };
    let streams = tokio::select! {
        streams = open => streams,
        next = out.recv() => {
            let reason = match next {
                Some(Out::Close(reason)) => reason,
                _ => CloseReason::Normal,
            };
            close(&connection, reason);
            let e = connection.closed().await;
            let _ = node.send(Internal::Closed(id, Cause::from_error(&e)));
            return;
        }
    };
    let (mut send, mut recv) = match streams {
        Ok(streams) => streams,
        Err(e) => {
            let _ = node.send(Internal::Closed(id, Cause::from_error(&e)));
            return;
        }
    };
    let mut session = Session::new(hello);
    let mut hello = Vec::new();
    // `Hello` is 4 bytes, far under the frame limit.
    let _ = session.local_hello().encode_frame(&mut hello);
    let _ = send.write_all(&hello).await;

    let mut fwd = Forwarding {
        member,
        records_in: false,
        sender: Sender::default(),
        receiver: Receiver::default(),
    };
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    // Clipboard streams that arrived before the peer's records: QUIC does
    // not order streams, so they are read once the records are in. The
    // stream limit of two means there are never more than two.
    let mut held: Vec<quinn::RecvStream> = Vec::new();
    let lanes = Lanes::new();
    loop {
        let deadline = fwd.sender.next_deadline();
        tokio::select! {
            read = recv.read(&mut chunk) => match read {
                Ok(Some(n)) => {
                    buf.extend_from_slice(chunk.get(..n).unwrap_or_default());
                    if let Err(reason) = drain(&mut buf, &mut session, &mut fwd, id, &node) {
                        close(&connection, reason);
                        break;
                    }
                    if fwd.open() {
                        for stream in held.drain(..) {
                            spawn_reader(id, stream, &session, &connection, &lanes, &node);
                        }
                    }
                }
                // A peer finishes the control stream just before closing the
                // connection; wait for its close reason. One that keeps the
                // connection open without the stream breaks the protocol.
                Ok(None) => {
                    if let Ok(e) = tokio::time::timeout(LINGER, connection.closed()).await {
                        let _ = node.send(Internal::Closed(id, Cause::from_error(&e)));
                        return;
                    }
                    close(&connection, CloseReason::ProtocolError);
                    break;
                }
                Err(_) => break,
            },
            datagram = connection.read_datagram() => match datagram {
                Ok(bytes) => {
                    if let Err(reason) = on_datagram(&bytes, &session, &mut fwd, id, &node) {
                        close(&connection, reason);
                        break;
                    }
                }
                Err(e) => {
                    let _ = node.send(Internal::Closed(id, Cause::from_error(&e)));
                    return;
                }
            },
            uni = connection.accept_uni() => match uni {
                Ok(stream) => {
                    // Clipboard streams belong to member connections only.
                    if !fwd.member || held.len() >= 2 {
                        close(&connection, CloseReason::ProtocolError);
                        break;
                    }
                    if fwd.open() {
                        spawn_reader(id, stream, &session, &connection, &lanes, &node);
                    } else {
                        held.push(stream);
                    }
                }
                Err(e) => {
                    let _ = node.send(Internal::Closed(id, Cause::from_error(&e)));
                    return;
                }
            },
            next = out.recv() => match next {
                Some(Out::Frame(bytes)) => {
                    if send.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                Some(Out::Peer(msg)) => {
                    let frame = match forward::outbound(msg) {
                        Outbound::Motion(dx, dy) => {
                            if let Some(mark) = fwd.sender.add(dx, dy, Instant::now()) {
                                send_motion(&connection, mark);
                            }
                            None
                        }
                        Outbound::Plain(m) => Some(m),
                        Outbound::Marked(m) => Some(m.with(fwd.sender.mark())),
                    };
                    let mut bytes = Vec::new();
                    if let Some(m) = frame
                        && m.encode(&mut bytes).is_ok()
                        && send.write_all(&bytes).await.is_err()
                    {
                        break;
                    }
                }
                Some(Out::Transfer(clip, contents)) => {
                    // A second guard behind the node's: never an image to a
                    // peer whose version does not define it.
                    if session.version().unwrap_or(0) >= contents.kind().since() {
                        let lane = lanes.copy_out.clone();
                        tokio::spawn(write_transfer(connection.clone(), clip, contents, lane));
                    }
                }
                Some(Out::File { clip, index, len, body }) => {
                    if session.version().unwrap_or(0) >= ClipKind::Files.since() {
                        let lane = lanes.file_out.clone();
                        tokio::spawn(write_file(connection.clone(), clip, index, len, body, lane));
                    }
                }
                Some(Out::Close(reason)) => {
                    let _ = send.finish();
                    let _ = tokio::time::timeout(FLUSH, send.stopped()).await;
                    close(&connection, reason);
                    break;
                }
                None => {
                    close(&connection, CloseReason::Normal);
                    break;
                }
            },
            () = async {
                match deadline {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            } => {
                if let Some(mark) = fwd.sender.poll(Instant::now()) {
                    send_motion(&connection, mark);
                }
            }
            e = connection.closed() => {
                let _ = node.send(Internal::Closed(id, Cause::from_error(&e)));
                return;
            }
        }
    }
    let e = connection.closed().await;
    let _ = node.send(Internal::Closed(id, Cause::from_error(&e)));
}

/// True for the engine message types that travel on the control stream.
const fn is_engine_frame(ty: u16) -> bool {
    ty >= registry::FOCUS_ENTER && ty <= registry::SECURE_ATTENTION
}

/// Decodes every complete frame in `buf` and passes it on; keeps the rest.
fn drain(
    buf: &mut Vec<u8>,
    session: &mut Session,
    fwd: &mut Forwarding,
    id: ConnId,
    node: &mpsc::UnboundedSender<Internal>,
) -> Result<(), CloseReason> {
    let mut start = 0;
    loop {
        let rest = buf.get(start..).unwrap_or_default();
        match frame::decode(rest) {
            Ok(Decoded::NeedMore) => break,
            Ok(Decoded::Frame { frame, consumed }) => {
                match session.on_control_frame(frame) {
                    Ok(ControlEvent::Agreed(version)) => {
                        let _ = node.send(Internal::Ready(id, version));
                    }
                    Ok(ControlEvent::Message(m)) if is_engine_frame(m.info.ty) => {
                        if !fwd.open() {
                            return Err(CloseReason::ProtocolError);
                        }
                        let msg = InputMessage::decode(m.info.ty, m.payload)
                            .map_err(|_| CloseReason::ProtocolError)?;
                        fwd.report(id, forward::inbound(msg), node);
                    }
                    Ok(ControlEvent::Message(m)) if m.info.ty == registry::FILE_CONTENT_REQUEST => {
                        if !fwd.open() {
                            return Err(CloseReason::ProtocolError);
                        }
                        let request = FileContentRequest::decode(m.payload)
                            .map_err(|_| CloseReason::ProtocolError)?;
                        let _ = node.send(Internal::FileRequest(id, request));
                    }
                    Ok(ControlEvent::Message(m)) if ClipboardMessage::is_clipboard(m.info.ty) => {
                        if !fwd.open() {
                            return Err(CloseReason::ProtocolError);
                        }
                        let msg = ClipboardMessage::decode(m.info.ty, m.payload)
                            .map_err(|_| CloseReason::ProtocolError)?;
                        let _ = node.send(Internal::Clipboard(id, msg));
                    }
                    Ok(ControlEvent::Message(m)) => {
                        if m.info.ty == registry::RECORDS_DONE {
                            fwd.records_in = true;
                        }
                        let _ = node.send(Internal::Message(id, m.info.ty, m.payload.to_vec()));
                    }
                    Err(e) => return Err(e.close_reason()),
                }
                start += consumed;
            }
            Err(_) => return Err(CloseReason::ProtocolError),
        }
    }
    buf.drain(..start);
    Ok(())
}

/// One stream of each kind at a time, in each direction: a copy (text, an
/// image or a manifest) and a file's contents (add-clipboard-files D4). A
/// stream that arrives while one of its kind is being read waits, unread,
/// with QUIC flow control holding its bytes at the sender.
#[derive(Clone)]
struct Lanes {
    copy_in: Arc<Semaphore>,
    file_in: Arc<Semaphore>,
    copy_out: Arc<Semaphore>,
    file_out: Arc<Semaphore>,
}

impl Lanes {
    fn new() -> Self {
        let lane = || Arc::new(Semaphore::new(1));
        Self {
            copy_in: lane(),
            file_in: lane(),
            copy_out: lane(),
            file_out: lane(),
        }
    }
}

/// The application code a receiver stops an unwanted stream with, and a
/// sender resets an abandoned one with.
const ABANDONED: quinn::VarInt = quinn::VarInt::from_u32(0);

/// Reads one clipboard stream on a task of its own. A protocol error closes
/// the connection; a stream the peer abandons is dropped.
fn spawn_reader(
    id: ConnId,
    stream: quinn::RecvStream,
    session: &Session,
    connection: &quinn::Connection,
    lanes: &Lanes,
    node: &mpsc::UnboundedSender<Internal>,
) {
    let (session, connection, lanes, node) = (
        session.clone(),
        connection.clone(),
        lanes.clone(),
        node.clone(),
    );
    tokio::spawn(async move {
        if let Err(reason) = read_stream(id, stream, &session, &lanes, &node).await {
            close(&connection, reason);
        }
    });
}

/// Why a clipboard stream stopped short.
enum StreamError {
    /// It broke the protocol: close the connection.
    Protocol,
    /// The peer reset it, or the connection went away: drop what it carried.
    Reset,
}

/// The frames of one clipboard stream, each checked by the session. Holds at
/// most one frame of undecoded bytes.
struct Frames {
    stream: quinn::RecvStream,
    buf: Vec<u8>,
}

/// The largest payload a clipboard stream's first frame has: a
/// `FileContentData`.
const MAX_HEADER_PAYLOAD: usize = 20;

impl Frames {
    /// The stream's first frame, read exactly and nothing past it, so a
    /// stream waiting for its lane keeps its bytes at the sender and its
    /// stream credit stays spent.
    async fn header(&mut self, session: &Session) -> Result<(u16, Vec<u8>), StreamError> {
        let mut head = [0u8; frame::HEADER_LEN];
        self.read_exact(&mut head).await?;
        let (len, ty) = head.split_at(4);
        let len = <[u8; 4]>::try_from(len)
            .map(u32::from_le_bytes)
            .unwrap_or(u32::MAX);
        let len = usize::try_from(len).unwrap_or(usize::MAX);
        if len > MAX_HEADER_PAYLOAD {
            return Err(StreamError::Protocol);
        }
        let mut payload = vec![0u8; len];
        self.read_exact(&mut payload).await?;
        let ty = <[u8; 2]>::try_from(ty).map(u16::from_le_bytes).unwrap_or(0);
        match session.on_stream_frame(frame::Frame {
            ty,
            payload: &payload,
        }) {
            Ok(StreamEvent::Message(m)) => Ok((m.info.ty, payload.clone())),
            _ => Err(StreamError::Protocol),
        }
    }

    async fn read_exact(&mut self, into: &mut [u8]) -> Result<(), StreamError> {
        match self.stream.read_exact(into).await {
            Ok(()) => Ok(()),
            Err(quinn::ReadExactError::FinishedEarly(_)) => Err(StreamError::Protocol),
            Err(quinn::ReadExactError::ReadError(_)) => Err(StreamError::Reset),
        }
    }

    /// The next frame's type and payload, or `None` at a clean end.
    async fn next(&mut self, session: &Session) -> Result<Option<(u16, Vec<u8>)>, StreamError> {
        let mut chunk = [0u8; 16 * 1024];
        loop {
            match frame::decode(&self.buf) {
                Err(_) => return Err(StreamError::Protocol),
                Ok(Decoded::Frame { frame, consumed }) => {
                    let Ok(StreamEvent::Message(m)) = session.on_stream_frame(frame) else {
                        return Err(StreamError::Protocol);
                    };
                    let next = (m.info.ty, m.payload.to_vec());
                    self.buf.drain(..consumed);
                    return Ok(Some(next));
                }
                Ok(Decoded::NeedMore) => {}
            }
            match self.stream.read(&mut chunk).await {
                Ok(Some(n)) => self
                    .buf
                    .extend_from_slice(chunk.get(..n).unwrap_or_default()),
                Ok(None) if self.buf.is_empty() => return Ok(None),
                Ok(None) => return Err(StreamError::Protocol),
                Err(_) => return Err(StreamError::Reset),
            }
        }
    }
}

/// Reads a clipboard stream's header, waits for its kind's lane, then reads
/// the rest: a copy goes to the node whole; a file's contents go, as they
/// arrive, to whoever asked for them.
async fn read_stream(
    id: ConnId,
    stream: quinn::RecvStream,
    session: &Session,
    lanes: &Lanes,
    node: &mpsc::UnboundedSender<Internal>,
) -> Result<(), CloseReason> {
    // Read only after the peer's records, so a version is agreed.
    let version = session.version().ok_or(CloseReason::ProtocolError)?;
    let mut reader = TransferReader::new(version);
    let mut frames = Frames {
        stream,
        buf: Vec::new(),
    };
    let (ty, payload) = match frames.header(session).await {
        Ok(first) => first,
        Err(StreamError::Protocol) => return Err(CloseReason::ProtocolError),
        Err(StreamError::Reset) => return Ok(()),
    };
    let header = reader
        .on_message(ty, &payload)
        .map_err(|_| CloseReason::ProtocolError)?;
    match header {
        TransferStep::Started { id: clip, kind, .. } => {
            let Ok(_lane) = lanes.copy_in.acquire().await else {
                return Ok(());
            };
            if let Some(contents) =
                read_copy(id, clip, kind, &mut frames, &mut reader, session, node).await?
            {
                let _ = node.send(Internal::ClipboardData(id, clip, contents));
            }
            Ok(())
        }
        TransferStep::FileStarted {
            id: clip,
            index,
            len,
        } => {
            let Ok(_lane) = lanes.file_in.acquire().await else {
                return Ok(());
            };
            read_file(id, clip, index, len, frames, reader, session, node).await
        }
        TransferStep::Chunk(_) => Err(CloseReason::ProtocolError),
    }
}

/// The rest of a copy's stream, or `None` if the peer reset it. The copy is
/// bounded by its kind's limit, which the reader checked in the header.
/// Reports progress to the node at most once per [`PROGRESS_EVERY`]. An
/// image's bytes are passed on undecoded; a manifest is checked whole.
async fn read_copy(
    id: ConnId,
    clip: u64,
    kind: ClipKind,
    frames: &mut Frames,
    reader: &mut TransferReader,
    session: &Session,
    node: &mpsc::UnboundedSender<Internal>,
) -> Result<Option<Contents>, CloseReason> {
    let mut body = Vec::new();
    let mut reported = Instant::now();
    loop {
        match frames.next(session).await {
            Ok(Some((ty, payload))) => match reader.on_message(ty, &payload) {
                Ok(TransferStep::Chunk(bytes)) => body.extend_from_slice(bytes),
                _ => return Err(CloseReason::ProtocolError),
            },
            Ok(None) => break,
            Err(StreamError::Reset) => return Ok(None),
            Err(StreamError::Protocol) => return Err(CloseReason::ProtocolError),
        }
        if reported.elapsed() >= PROGRESS_EVERY {
            reported = Instant::now();
            let _ = node.send(Internal::ClipboardProgress(id, clip));
        }
    }
    reader.finish().map_err(|_| CloseReason::ProtocolError)?;
    Ok(Some(match kind {
        ClipKind::Text => {
            let text = clipboard::text(&body).map_err(|_| CloseReason::ProtocolError)?;
            Contents::Text(Arc::from(text))
        }
        ClipKind::Image => Contents::Image(Arc::from(body)),
        ClipKind::Files => {
            Manifest::parse(&body).map_err(|_| CloseReason::ProtocolError)?;
            Contents::Files(Arc::from(body))
        }
    }))
}

/// The rest of a file content stream, passed on as it arrives to the fetch
/// that asked for it. One nobody asked for, or nobody waits on any more, is
/// stopped unread. The fetch ends without `End` if the peer resets the
/// stream.
#[allow(clippy::too_many_arguments)]
async fn read_file(
    id: ConnId,
    clip: u64,
    index: u32,
    len: u64,
    mut frames: Frames,
    mut reader: TransferReader,
    session: &Session,
    node: &mpsc::UnboundedSender<Internal>,
) -> Result<(), CloseReason> {
    let (reply, asked) = oneshot::channel();
    let _ = node.send(Internal::FileStarted(id, clip, index, reply));
    let Ok(Some(sink)) = asked.await else {
        let _ = frames.stream.stop(ABANDONED);
        return Ok(());
    };
    if sink.send(FileChunk::Len(len)).await.is_err() {
        let _ = frames.stream.stop(ABANDONED);
        return Ok(());
    }
    loop {
        match frames.next(session).await {
            Ok(Some((ty, payload))) => {
                if !matches!(reader.on_message(ty, &payload), Ok(TransferStep::Chunk(_))) {
                    return Err(CloseReason::ProtocolError);
                }
                // A chunk's bytes are exactly its payload.
                if sink.send(FileChunk::Bytes(payload)).await.is_err() {
                    let _ = frames.stream.stop(ABANDONED);
                    return Ok(());
                }
            }
            Ok(None) => break,
            Err(StreamError::Reset) => return Ok(()),
            Err(StreamError::Protocol) => return Err(CloseReason::ProtocolError),
        }
    }
    reader.finish().map_err(|_| CloseReason::ProtocolError)?;
    let _ = sink.send(FileChunk::End).await;
    Ok(())
}

/// Writes `contents` on a new clipboard stream, once the copy lane is free:
/// its kind's header, then chunks of at most one frame, then the end. Holds
/// the lane until the peer has the whole stream.
async fn write_transfer(
    connection: quinn::Connection,
    clip: u64,
    contents: Contents,
    lane: Arc<Semaphore>,
) {
    let kind = contents.kind();
    let bytes = contents.bytes();
    let Ok(len) = u32::try_from(bytes.len()) else {
        return;
    };
    if len > kind.max_len() {
        return;
    }
    let Ok(_lane) = lane.acquire().await else {
        return;
    };
    let Ok(mut stream) = connection.open_uni().await else {
        return;
    };
    // Below the control stream's default 0, so input waiting to go out
    // is always sent first.
    let _ = stream.set_priority(-1);
    let mut out = Vec::new();
    if clipboard::encode_header_for(kind, clip, len, &mut out).is_err()
        || stream.write_all(&out).await.is_err()
    {
        return;
    }
    for piece in bytes.chunks(MAX_FRAME_PAYLOAD) {
        out.clear();
        if clipboard::encode_chunk(piece, &mut out).is_err()
            || stream.write_all(&out).await.is_err()
        {
            return;
        }
    }
    let _ = stream.finish();
    let _ = stream.stopped().await;
}

/// Writes one file's contents on a new file content stream, once the file
/// lane is free: the header, then `body`'s bytes in chunks of at most one
/// frame as they come. Ends the stream when exactly `len` bytes were written;
/// resets it if `body` ends short or runs over, so the peer never mistakes a
/// partial file for a whole one.
async fn write_file(
    connection: quinn::Connection,
    clip: u64,
    index: u32,
    len: u64,
    mut body: mpsc::Receiver<Vec<u8>>,
    lane: Arc<Semaphore>,
) {
    let Ok(_lane) = lane.acquire().await else {
        return;
    };
    let Ok(mut stream) = connection.open_uni().await else {
        return;
    };
    let _ = stream.set_priority(-1);
    let mut out = Vec::new();
    if files::encode_content_data(clip, index, len, &mut out).is_err()
        || stream.write_all(&out).await.is_err()
    {
        let _ = stream.reset(ABANDONED);
        return;
    }
    let mut sent = 0u64;
    while let Some(bytes) = body.recv().await {
        sent = sent.saturating_add(bytes.len() as u64);
        if sent > len {
            let _ = stream.reset(ABANDONED);
            return;
        }
        for piece in bytes.chunks(MAX_FRAME_PAYLOAD) {
            out.clear();
            if clipboard::encode_chunk(piece, &mut out).is_err()
                || stream.write_all(&out).await.is_err()
            {
                let _ = stream.reset(ABANDONED);
                return;
            }
        }
    }
    if sent == len {
        let _ = stream.finish();
        let _ = stream.stopped().await;
    } else {
        let _ = stream.reset(ABANDONED);
    }
}

/// Handles one datagram: dropped until engine messages are allowed, then a
/// `Motion` whose movement goes to the node.
fn on_datagram(
    bytes: &[u8],
    session: &Session,
    fwd: &mut Forwarding,
    id: ConnId,
    node: &mpsc::UnboundedSender<Internal>,
) -> Result<(), CloseReason> {
    if !fwd.open() {
        return Ok(());
    }
    match session.on_datagram(bytes) {
        Ok(DatagramEvent::Dropped) => Ok(()),
        Ok(DatagramEvent::Message(m)) => match InputMessage::decode(m.info.ty, m.payload) {
            Ok(InputMessage::Motion(mark)) => {
                fwd.report(id, Inbound::Motion(mark), node);
                Ok(())
            }
            Ok(_) | Err(_) => Err(CloseReason::ProtocolError),
        },
        Err(e) => Err(e.close_reason()),
    }
}
