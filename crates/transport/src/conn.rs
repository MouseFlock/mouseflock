//! One task per connection: `Hello` and frames on the control stream, and
//! engine messages and motion datagrams once the peer's records are in.
//! Every decision about trust belongs to the node; this task moves bytes
//! and keeps the motion totals (design D1, D4; input-forwarding D5).

use std::time::Duration;

use model::{InputAction, PeerMessage};
use pairing::ConnId;
use proto::CloseReason;
use proto::frame::{self, Decoded};
use proto::hello::Hello;
use proto::input::{InputMessage, Mark};
use proto::registry;
use proto::session::{ControlEvent, DatagramEvent, Session};
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
