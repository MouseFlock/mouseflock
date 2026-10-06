//! One task per connection: `Hello` and frames on the control stream, and
//! engine messages, clipboard messages and motion datagrams once the peer's
//! records are in. Clipboard text travels on unidirectional streams of its
//! own, each read or written by a task of its own, so it never queues ahead
//! of input on the control stream. Every decision about trust belongs to the
//! node; this task moves bytes and keeps the motion totals (design D1, D4;
//! input-forwarding D5; text-clipboard D3).

use std::sync::Arc;
use std::time::Duration;

use model::{InputAction, PeerMessage};
use pairing::ConnId;
use proto::CloseReason;
use proto::clipboard::{self, ClipboardMessage, MAX_TEXT_LEN, TransferReader, TransferStep};
use proto::frame::{self, Decoded, MAX_FRAME_PAYLOAD};
use proto::hello::Hello;
use proto::input::{InputMessage, Mark};
use proto::registry;
use proto::session::{ControlEvent, DatagramEvent, Session, StreamEvent};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::forward::{self, Inbound, Outbound};
use crate::motion::{Receiver, Sender};
use crate::node::Internal;

/// What the node asks a connection task to do.
#[derive(Debug)]
pub(crate) enum Out {
    /// Write this encoded frame on the control stream.
    Frame(Vec<u8>),
    /// Send an engine message. The node sends these only once the peer is up.
    Peer(PeerMessage),
    /// Send clipboard text with this ID on a clipboard stream of its own. The
    /// node sends these only once the peer is up.
    Transfer(u64, Arc<str>),
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
    let mut session = Session::new(Hello::this_release());
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
    // A clipboard stream that arrived before the peer's records: QUIC does
    // not order streams, so it is read once they are in. The stream limit
    // of one means there is never more than one.
    let mut held: Option<quinn::RecvStream> = None;
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
                    if fwd.open()
                        && let Some(stream) = held.take()
                    {
                        spawn_reader(id, stream, &session, &connection, &node);
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
                    if !fwd.member || held.is_some() {
                        close(&connection, CloseReason::ProtocolError);
                        break;
                    }
                    if fwd.open() {
                        spawn_reader(id, stream, &session, &connection, &node);
                    } else {
                        held = Some(stream);
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
                Some(Out::Transfer(clip, text)) => {
                    tokio::spawn(write_transfer(connection.clone(), clip, text));
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
                    Ok(ControlEvent::Agreed(_)) => {
                        let _ = node.send(Internal::Ready(id));
                    }
                    Ok(ControlEvent::Message(m)) if is_engine_frame(m.info.ty) => {
                        if !fwd.open() {
                            return Err(CloseReason::ProtocolError);
                        }
                        let msg = InputMessage::decode(m.info.ty, m.payload)
                            .map_err(|_| CloseReason::ProtocolError)?;
                        fwd.report(id, forward::inbound(msg), node);
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

/// Reads one clipboard stream on a task of its own. Complete text goes to
/// the node; a protocol error closes the connection; a stream the peer
/// abandons is dropped.
fn spawn_reader(
    id: ConnId,
    stream: quinn::RecvStream,
    session: &Session,
    connection: &quinn::Connection,
    node: &mpsc::UnboundedSender<Internal>,
) {
    let (session, connection, node) = (session.clone(), connection.clone(), node.clone());
    tokio::spawn(async move {
        match receive(stream, &session).await {
            Ok(Some((clip, text))) => {
                let _ = node.send(Internal::ClipboardText(id, clip, text));
            }
            Ok(None) => {}
            Err(reason) => close(&connection, reason),
        }
    });
}

/// The text a clipboard stream carries, or `None` if the peer reset it.
/// Holds at most one frame of undecoded bytes plus the text, which the
/// reader bounds at [`MAX_TEXT_LEN`].
async fn receive(
    mut stream: quinn::RecvStream,
    session: &Session,
) -> Result<Option<(u64, Arc<str>)>, CloseReason> {
    let mut reader = TransferReader::new();
    let mut clip = 0;
    let mut text = Vec::new();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        match stream.read(&mut chunk).await {
            Ok(Some(n)) => {
                buf.extend_from_slice(chunk.get(..n).unwrap_or_default());
                let mut start = 0;
                loop {
                    let rest = buf.get(start..).unwrap_or_default();
                    let Decoded::Frame { frame, consumed } =
                        frame::decode(rest).map_err(|_| CloseReason::ProtocolError)?
                    else {
                        break;
                    };
                    // Read only after the peer's records, so a version is agreed.
                    let Ok(StreamEvent::Message(m)) = session.on_stream_frame(frame) else {
                        return Err(CloseReason::ProtocolError);
                    };
                    match reader
                        .on_message(m.info.ty, m.payload)
                        .map_err(|_| CloseReason::ProtocolError)?
                    {
                        TransferStep::Started { id, .. } => clip = id,
                        TransferStep::Chunk(bytes) => text.extend_from_slice(bytes),
                    }
                    start += consumed;
                }
                buf.drain(..start);
            }
            Ok(None) => {
                if !buf.is_empty() || reader.finish().is_err() {
                    return Err(CloseReason::ProtocolError);
                }
                let text = clipboard::text(&text).map_err(|_| CloseReason::ProtocolError)?;
                return Ok(Some((clip, Arc::from(text))));
            }
            Err(_) => return Ok(None),
        }
    }
}

/// Writes `text` on a new clipboard stream: `ClipboardData`, then chunks of
/// at most one frame, then the end. Waits for the peer's stream credit, which
/// it grants one transfer at a time.
async fn write_transfer(connection: quinn::Connection, clip: u64, text: Arc<str>) {
    let bytes = text.as_bytes();
    let Ok(len) = u32::try_from(bytes.len()) else {
        return;
    };
    if len > MAX_TEXT_LEN {
        return;
    }
    let Ok(mut stream) = connection.open_uni().await else {
        return;
    };
    // Below the control stream's default 0, so input waiting to go out
    // is always sent first.
    let _ = stream.set_priority(-1);
    let mut out = Vec::new();
    if clipboard::encode_data(clip, len, &mut out).is_err() || stream.write_all(&out).await.is_err()
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
