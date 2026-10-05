//! The node: one task that owns this machine's state and every decision about
//! connections (design D1). Connection tasks only move bytes.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use model::{Millis, PeerId, PeerMessage};
use pairing::{
    Binding, Code, ConnId, Desk, EXPORTER_LABEL, EXPORTER_LEN, JoinFailure, Joiner, JoinerOutput,
    Machine, MachineName, ModeClosed, ModeOutput, Now, PairingMode, RecordInbox, SignedRecord,
    UnixTime,
};
use proto::CloseReason;
use proto::pairing::{PairingMessage, RecordsDone};
use rand_core::OsRng;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tokio::time::Instant;

use crate::conn::{self, Cause, Out};
use crate::discovery::{Mdns, Sighting};
use crate::endpoint::{self, PORT, StartError};
use crate::net::{self, Network};
use crate::tls::{self, ALPN_MEMBER, ALPN_PAIR, Credentials, Expect, SERVER_NAME};

/// Most connections from keys that are not members, held at once.
pub const MAX_STRANGERS: usize = 8;

/// How long a stranger has to send `PairPake` or finish introducing itself.
pub const STRANGER_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a dial may take to finish its handshake.
pub const DIAL_TIMEOUT: Duration = Duration::from_secs(5);

/// Most sightings remembered.
const MAX_SIGHTINGS: usize = 64;

/// How a node starts.
pub struct Config {
    /// This machine, with its trust store already loaded.
    pub machine: Machine,
    /// Address to listen on.
    pub bind: IpAddr,
    /// Port to listen on. 0 picks one (tests).
    pub port: u16,
    /// Port to dial when nothing advertises one.
    pub dial_port: u16,
    /// Advertise and browse with mDNS.
    pub discovery: bool,
    /// Where each member was last reached, as reported by [`Event::Addresses`].
    pub addresses: BTreeMap<PeerId, SocketAddr>,
    /// The interfaces to use instead of the operating system's (tests).
    pub network: Option<Network>,
    pub stranger_timeout: Duration,
}

impl Config {
    /// Listen on every IPv4 interface on port 47391, with mDNS.
    pub fn new(machine: Machine) -> Self {
        Self {
            machine,
            bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port: PORT,
            dial_port: PORT,
            discovery: true,
            addresses: BTreeMap::new(),
            network: None,
            stranger_timeout: STRANGER_TIMEOUT,
        }
    }
}

/// Why a peer is down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Down {
    /// Nothing heard for the idle timeout.
    Lost,
    /// It said goodbye, or this machine did.
    Goodbye,
    /// One side removed the other from the desk.
    Removed,
}

/// What the node reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    PairingClosed(ModeClosed),
    /// A pairing completed, on either side.
    Paired {
        peer: PeerId,
        name: MachineName,
    },
    JoinFailed(JoinFailure),
    PeerUp(PeerId),
    PeerDown(PeerId, Down),
    /// The trust store file's new contents.
    TrustStore(Vec<u8>),
    /// Where each member was last reached.
    Addresses(BTreeMap<PeerId, SocketAddr>),
    /// A message from an up member's engine. Forwarded motion arrives as
    /// `PeerMessage::Input(InputAction::Motion { .. })`, rebuilt from the
    /// running totals.
    Message {
        peer: PeerId,
        msg: PeerMessage,
    },
}

/// Pairing mode could not open: no IPv4 network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoNetwork;

impl std::fmt::Display for NoNetwork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("no IPv4 network was found")
    }
}

impl std::error::Error for NoNetwork {}

enum Command {
    OpenPairing(oneshot::Sender<Result<Code, NoNetwork>>),
    ClosePairing,
    Join(Code),
    Remove(PeerId),
    Goodbye(oneshot::Sender<()>),
    Resume,
    Sighting(Sighting),
    Send(PeerId, PeerMessage),
    Connections(oneshot::Sender<Vec<Connection>>),
    Shutdown,
}

/// An open member connection, for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Connection {
    pub peer: PeerId,
    /// This machine dialed it.
    pub dialed_here: bool,
}

/// Commands to a running node. Cheap to clone.
#[derive(Clone)]
pub struct NodeHandle {
    tx: mpsc::UnboundedSender<Command>,
    local_addr: SocketAddr,
}

impl NodeHandle {
    /// The address the node listens on.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Opens pairing mode and returns the code to show.
    pub async fn open_pairing(&self) -> Result<Code, NoNetwork> {
        let (reply, rx) = oneshot::channel();
        let _ = self.tx.send(Command::OpenPairing(reply));
        rx.await.unwrap_or(Err(NoNetwork))
    }

    pub fn close_pairing(&self) {
        let _ = self.tx.send(Command::ClosePairing);
    }

    /// Pairs with the machine showing `code`.
    pub fn join(&self, code: Code) {
        let _ = self.tx.send(Command::Join(code));
    }

    /// Removes `peer` from the desk; this machine's own identity leaves it.
    pub fn remove(&self, peer: PeerId) {
        let _ = self.tx.send(Command::Remove(peer));
    }

    /// Closes every member connection with goodbye, and stops connecting
    /// until [`NodeHandle::resume`].
    pub async fn goodbye(&self) {
        let (reply, rx) = oneshot::channel();
        let _ = self.tx.send(Command::Goodbye(reply));
        let _ = rx.await;
    }

    pub fn resume(&self) {
        let _ = self.tx.send(Command::Resume);
    }

    /// Reports a machine seen on the network. mDNS calls this; so do tests.
    pub fn sighting(&self, sighting: Sighting) {
        let _ = self.tx.send(Command::Sighting(sighting));
    }

    /// Sends an engine message to `peer` if it is up; otherwise drops it.
    /// The engine learns the peer is gone from `Event::PeerDown`.
    pub fn send(&self, peer: PeerId, msg: PeerMessage) {
        let _ = self.tx.send(Command::Send(peer, msg));
    }

    /// How many member connections are open, for diagnostics.
    pub async fn connections(&self) -> Vec<Connection> {
        let (reply, rx) = oneshot::channel();
        let _ = self.tx.send(Command::Connections(reply));
        rx.await.unwrap_or_default()
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(Command::Shutdown);
    }
}

/// Starts a node. Call from within a Tokio runtime.
pub fn start(config: Config) -> Result<(NodeHandle, mpsc::UnboundedReceiver<Event>), StartError> {
    let credentials =
        Credentials::new(config.machine.identity()).map_err(|e| StartError::Tls(e.to_string()))?;
    let endpoint = endpoint::bind(&credentials, SocketAddr::new(config.bind, config.port))?;
    let local_addr = endpoint.local_addr().map_err(|source| StartError::Bind {
        addr: SocketAddr::new(config.bind, config.port),
        source,
    })?;
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (internal_tx, internal_rx) = mpsc::unbounded_channel();
    let (events, events_rx) = mpsc::unbounded_channel();

    let mdns = if config.discovery {
        let seen = cmd_tx.clone();
        Mdns::start(config.machine.peer_id(), local_addr.port(), move |s| {
            let _ = seen.send(Command::Sighting(s));
        })
        .ok()
    } else {
        None
    };

    tokio::spawn(accept(endpoint.clone(), internal_tx.clone()));
    let node = Node {
        me: config.machine.peer_id(),
        machine: config.machine,
        mode: PairingMode::new(),
        credentials: Arc::new(credentials),
        endpoint,
        dial_port: config.dial_port,
        network: config.network,
        stranger_timeout: config.stranger_timeout,
        mdns,
        start: Instant::now(),
        next_conn: 0,
        conns: HashMap::new(),
        up: BTreeMap::new(),
        limbo: BTreeMap::new(),
        redial: BTreeMap::new(),
        sightings: BTreeMap::new(),
        sighting_order: VecDeque::new(),
        last_reached: config.addresses,
        strangers: 0,
        race: None,
        next_race: 0,
        quiet: false,
        events,
        internal: internal_tx,
    };
    tokio::spawn(node.run(cmd_rx, internal_rx));
    Ok((
        NodeHandle {
            tx: cmd_tx,
            local_addr,
        },
        events_rx,
    ))
}

/// Messages from accept, dial, and connection tasks.
pub(crate) enum Internal {
    Accepted(quinn::Connection),
    Dialed(quinn::Connection, Purpose),
    DialFailed(Purpose, Vec<Option<CloseReason>>),
    Ready(ConnId),
    Message(ConnId, u16, Vec<u8>),
    /// An engine message from a member connection, already decoded.
    Peer(ConnId, PeerMessage),
    Closed(ConnId, Cause),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Purpose {
    Member(PeerId),
    Join(u64),
}

async fn accept(endpoint: quinn::Endpoint, node: mpsc::UnboundedSender<Internal>) {
    while let Some(incoming) = endpoint.accept().await {
        let node = node.clone();
        tokio::spawn(async move {
            if let Ok(Ok(connection)) = tokio::time::timeout(DIAL_TIMEOUT, incoming).await {
                let _ = node.send(Internal::Accepted(connection));
            }
        });
    }
}

/// Redialing a member: 0.5 s doubling to 30 s (design D8).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Backoff {
    failures: u32,
}

impl Backoff {
    /// The wait before the next attempt, and counts this failure.
    pub fn wait(&mut self) -> Duration {
        let wait = Duration::from_millis(500)
            .saturating_mul(1 << self.failures.min(6))
            .min(Duration::from_secs(30));
        self.failures = self.failures.saturating_add(1);
        wait
    }

    pub fn reset(&mut self) {
        self.failures = 0;
    }
}

#[derive(Default)]
struct Redial {
    backoff: Backoff,
    next: Millis,
    dialing: bool,
}

enum Role {
    Member {
        inbox: RecordInbox,
        /// This side's records and `RecordsDone` have been sent.
        sent_ours: bool,
        up: bool,
    },
    Inviter {
        binding: Option<Binding>,
    },
    Joiner {
        race: u64,
        joiner: Option<Joiner>,
    },
}

struct Conn {
    connection: quinn::Connection,
    out: mpsc::UnboundedSender<Out>,
    peer: PeerId,
    dialed: bool,
    role: Role,
    /// Counted against [`MAX_STRANGERS`].
    stranger: bool,
    /// When a stranger that has not yet sent `PairPake` or `RecordsDone` is
    /// closed.
    stranger_until: Option<Millis>,
    /// Closed by the one-connection rule; its close reports nothing.
    superseded: bool,
    /// Why this side closed it, if it did.
    local_down: Option<Down>,
}

impl Conn {
    fn send(&self, out: Out) {
        let _ = self.out.send(out);
    }

    fn is_live_member(&self) -> bool {
        matches!(self.role, Role::Member { .. }) && !self.superseded
    }
}

struct Race {
    id: u64,
    code: Code,
    pending: usize,
    winner: Option<ConnId>,
    reasons: Vec<Option<CloseReason>>,
}

struct Node {
    me: PeerId,
    machine: Machine,
    mode: PairingMode,
    credentials: Arc<Credentials>,
    endpoint: quinn::Endpoint,
    dial_port: u16,
    network: Option<Network>,
    stranger_timeout: Duration,
    mdns: Option<Mdns>,
    start: Instant,
    next_conn: u64,
    conns: HashMap<ConnId, Conn>,
    /// Each peer that is up, and its connection.
    up: BTreeMap<PeerId, ConnId>,
    /// Peers whose up connection closed while another was still connecting:
    /// still up as far as events go, until the other finishes or fails.
    limbo: BTreeMap<PeerId, Down>,
    redial: BTreeMap<PeerId, Redial>,
    sightings: BTreeMap<PeerId, Sighting>,
    sighting_order: VecDeque<PeerId>,
    last_reached: BTreeMap<PeerId, SocketAddr>,
    strangers: usize,
    race: Option<Race>,
    next_race: u64,
    /// After goodbye: no dialing, and member connections are turned away.
    quiet: bool,
    events: mpsc::UnboundedSender<Event>,
    internal: mpsc::UnboundedSender<Internal>,
}

impl Node {
    async fn run(
        mut self,
        mut commands: mpsc::UnboundedReceiver<Command>,
        mut internal: mpsc::UnboundedReceiver<Internal>,
    ) {
        loop {
            let wake = self
                .next_deadline()
                .map(|t| self.start + Duration::from_millis(t.0));
            tokio::select! {
                command = commands.recv() => match command {
                    None | Some(Command::Shutdown) => break,
                    Some(command) => self.command(command),
                },
                message = internal.recv() => {
                    if let Some(message) = message {
                        self.internal(message);
                    }
                }
                () = async {
                    match wake {
                        Some(at) => tokio::time::sleep_until(at).await,
                        None => std::future::pending().await,
                    }
                } => {}
            }
            self.tick();
        }
        for conn in self.conns.values() {
            conn::close(&conn.connection, CloseReason::Normal);
        }
        self.endpoint.close(0u32.into(), b"");
    }

    fn now(&self) -> Millis {
        Millis(u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX))
    }

    fn unix() -> UnixTime {
        UnixTime(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
        )
    }

    fn clock(&self) -> Now {
        Now {
            mono: self.now(),
            unix: Self::unix(),
        }
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }

    fn network(&self) -> Network {
        self.network.clone().unwrap_or_else(net::os_network)
    }

    // ---- commands -------------------------------------------------------

    fn command(&mut self, command: Command) {
        match command {
            Command::OpenPairing(reply) => {
                let result = match net::inviter_interface(&self.network()) {
                    Some(iface) => {
                        let now = self.now();
                        let out = self.mode.open(iface.addr, iface.prefix, now, &mut OsRng);
                        self.mode_outputs(out);
                        self.advertise(true);
                        self.mode.code().cloned().ok_or(NoNetwork)
                    }
                    None => Err(NoNetwork),
                };
                let _ = reply.send(result);
            }
            Command::ClosePairing => {
                let out = self.mode.close();
                self.mode_outputs(out);
            }
            Command::Join(code) => self.join(code),
            Command::Remove(peer) => self.remove(peer),
            Command::Goodbye(reply) => {
                self.quiet = true;
                for conn in self.conns.values_mut() {
                    if conn.is_live_member() {
                        conn.local_down = Some(Down::Goodbye);
                        conn::close(&conn.connection, CloseReason::Normal);
                    }
                }
                let _ = reply.send(());
            }
            Command::Resume => {
                self.quiet = false;
                let now = self.now();
                for redial in self.redial.values_mut() {
                    redial.backoff.reset();
                    redial.next = now;
                }
            }
            Command::Sighting(s) => self.sighting(s),
            Command::Send(peer, msg) => {
                if let Some(conn) = self.up.get(&peer).and_then(|id| self.conns.get(id)) {
                    conn.send(Out::Peer(msg));
                }
            }
            Command::Connections(reply) => {
                let open = self
                    .conns
                    .values()
                    .filter(|c| c.is_live_member())
                    .map(|c| Connection {
                        peer: c.peer,
                        dialed_here: c.dialed,
                    })
                    .collect();
                let _ = reply.send(open);
            }
            Command::Shutdown => {}
        }
    }

    fn sighting(&mut self, s: Sighting) {
        if s.id == self.me {
            return;
        }
        let id = s.id;
        if self.sightings.insert(id, s).is_none() {
            self.sighting_order.push_back(id);
            if self.sighting_order.len() > MAX_SIGHTINGS
                && let Some(oldest) = self.sighting_order.pop_front()
            {
                self.sightings.remove(&oldest);
            }
        }
        if self.machine.desk().is_member(&id) {
            // A fresh advertisement ends the wait, and does not wait for a
            // dial still stuck from before.
            let now = self.now();
            let redial = self.redial.entry(id).or_default();
            redial.backoff.reset();
            redial.next = now;
            redial.dialing = false;
        }
    }

    fn remove(&mut self, peer: PeerId) {
        let before = self.machine.desk().clone();
        let record = self.machine.remove(peer, Self::unix());
        if peer == self.me {
            // Tell the members before forgetting them.
            self.push(std::slice::from_ref(&record));
        }
        self.desk_changed(&before);
    }

    // ---- connections in -------------------------------------------------

    fn internal(&mut self, message: Internal) {
        match message {
            Internal::Accepted(connection) => self.accepted(connection),
            Internal::Dialed(connection, purpose) => self.dialed(connection, purpose),
            Internal::DialFailed(purpose, reasons) => self.dial_failed(purpose, reasons),
            Internal::Ready(id) => self.ready(id),
            Internal::Message(id, ty, payload) => self.message(id, ty, &payload),
            Internal::Peer(id, msg) => self.peer_message(id, msg),
            Internal::Closed(id, cause) => self.closed(id, cause),
        }
    }

    fn register(
        &mut self,
        connection: quinn::Connection,
        peer: PeerId,
        dialed: bool,
        role: Role,
    ) -> ConnId {
        let id = ConnId(self.next_conn);
        self.next_conn += 1;
        let (out, out_rx) = mpsc::unbounded_channel();
        tokio::spawn(conn::run(
            id,
            connection.clone(),
            dialed,
            matches!(role, Role::Member { .. }),
            out_rx,
            self.internal.clone(),
        ));
        self.conns.insert(
            id,
            Conn {
                connection,
                out,
                peer,
                dialed,
                role,
                stranger: false,
                stranger_until: None,
                superseded: false,
                local_down: None,
            },
        );
        id
    }

    fn accepted(&mut self, connection: quinn::Connection) {
        let Some(peer) = tls::connection_peer(&connection) else {
            conn::close(&connection, CloseReason::ProtocolError);
            return;
        };
        let alpn = connection
            .handshake_data()
            .and_then(|d| d.downcast::<quinn::crypto::rustls::HandshakeData>().ok())
            .and_then(|d| d.protocol);
        let pairing = match alpn.as_deref() {
            Some(ALPN_PAIR) => true,
            Some(ALPN_MEMBER) => false,
            _ => {
                conn::close(&connection, CloseReason::ProtocolError);
                return;
            }
        };
        if self.quiet && !pairing {
            conn::close(&connection, CloseReason::Normal);
            return;
        }
        let stranger = pairing || !self.machine.desk().is_member(&peer);
        let refusal = if pairing {
            CloseReason::NotReadyToPair
        } else {
            CloseReason::NotAMember
        };
        if stranger && self.strangers >= MAX_STRANGERS {
            conn::close(&connection, refusal);
            return;
        }
        let role = if pairing {
            Role::Inviter { binding: None }
        } else {
            Role::Member {
                inbox: RecordInbox::new(),
                sent_ours: false,
                up: false,
            }
        };
        let id = self.register(connection, peer, false, role);
        if stranger {
            self.strangers += 1;
            let until = Millis(self.now().0.saturating_add(
                u64::try_from(self.stranger_timeout.as_millis()).unwrap_or(u64::MAX),
            ));
            if let Some(conn) = self.conns.get_mut(&id) {
                conn.stranger = true;
                conn.stranger_until = Some(until);
            }
        }
    }

    fn dialed(&mut self, connection: quinn::Connection, purpose: Purpose) {
        let Some(peer) = tls::connection_peer(&connection) else {
            conn::close(&connection, CloseReason::ProtocolError);
            return;
        };
        match purpose {
            Purpose::Member(expected) => {
                if let Some(redial) = self.redial.get_mut(&expected) {
                    redial.dialing = false;
                }
                if self.quiet || !self.machine.desk().is_member(&peer) {
                    conn::close(&connection, CloseReason::Normal);
                    return;
                }
                let id = self.register(
                    connection,
                    peer,
                    true,
                    Role::Member {
                        inbox: RecordInbox::new(),
                        sent_ours: false,
                        up: false,
                    },
                );
                let _ = id;
            }
            Purpose::Join(race) => {
                let open = self
                    .race
                    .as_ref()
                    .is_some_and(|r| r.id == race && r.winner.is_none());
                if !open {
                    conn::close(&connection, CloseReason::Normal);
                    return;
                }
                self.register(connection, peer, true, Role::Joiner { race, joiner: None });
            }
        }
    }

    fn dial_failed(&mut self, purpose: Purpose, reasons: Vec<Option<CloseReason>>) {
        match purpose {
            Purpose::Member(peer) => self.schedule_redial(peer),
            Purpose::Join(race) => {
                if let Some(r) = self.race.as_mut().filter(|r| r.id == race) {
                    r.pending = r.pending.saturating_sub(1);
                    r.reasons.extend(reasons);
                }
                self.finish_race_if_done();
            }
        }
    }

    fn exporter(connection: &quinn::Connection) -> [u8; EXPORTER_LEN] {
        let mut out = [0; EXPORTER_LEN];
        // 32 bytes is always within the exporter's limits.
        let _ = connection.export_keying_material(&mut out, EXPORTER_LABEL, b"");
        out
    }

    fn ready(&mut self, id: ConnId) {
        let me = self.me;
        let frames = self.machine.record_frames();
        let Some(conn) = self.conns.get_mut(&id) else {
            return;
        };
        let exporter = Self::exporter(&conn.connection);
        let (dialed, out) = (conn.dialed, conn.out.clone());
        match &mut conn.role {
            Role::Member { sent_ours, .. } => {
                if dialed {
                    for frame in frames {
                        let _ = out.send(Out::Frame(frame));
                    }
                    *sent_ours = true;
                }
            }
            Role::Inviter { binding } => {
                *binding = Some(Binding {
                    exporter,
                    inviter: me,
                    joiner: conn.peer,
                });
            }
            Role::Joiner { race, .. } => {
                let race = *race;
                let peer = conn.peer;
                let Some(code) = self
                    .race
                    .as_ref()
                    .filter(|r| r.id == race)
                    .map(|r| r.code.clone())
                else {
                    conn.send(Out::Close(CloseReason::Normal));
                    return;
                };
                let binding = Binding {
                    exporter,
                    inviter: peer,
                    joiner: me,
                };
                let (joiner, out) = Joiner::start(&code, binding, &mut OsRng);
                if let Role::Joiner { joiner: slot, .. } = &mut conn.role {
                    *slot = Some(joiner);
                }
                self.joiner_outputs(id, out);
            }
        }
    }

    /// Reports an engine message, but only from the peer's current up
    /// connection: not from a dialer about to be closed with `5`, nor
    /// from a duplicate being closed by the one-connection rule.
    fn peer_message(&self, id: ConnId, msg: PeerMessage) {
        let Some(conn) = self.conns.get(&id) else {
            return;
        };
        if self.up.get(&conn.peer) == Some(&id) {
            self.emit(Event::Message {
                peer: conn.peer,
                msg,
            });
        }
    }

    fn message(&mut self, id: ConnId, ty: u16, payload: &[u8]) {
        let Ok(message) = PairingMessage::decode(ty, payload) else {
            self.close(id, CloseReason::ProtocolError, None);
            return;
        };
        let Some(conn) = self.conns.get_mut(&id) else {
            return;
        };
        match &conn.role {
            Role::Member { .. } => self.member_message(id, &message),
            Role::Inviter { binding } => {
                let Some(binding) = *binding else {
                    return;
                };
                if matches!(message, PairingMessage::Pake(_)) {
                    conn.stranger_until = None;
                }
                let before = self.machine.desk().clone();
                let now = self.clock();
                let out =
                    self.mode
                        .on_message(id, binding, &message, &mut self.machine, now, &mut OsRng);
                self.desk_changed(&before);
                self.mode_outputs(out);
            }
            Role::Joiner { race, .. } => {
                let race = *race;
                if matches!(message, PairingMessage::Pake(_)) && !self.claim_race(race, id) {
                    self.close(id, CloseReason::Normal, None);
                    return;
                }
                let before = self.machine.desk().clone();
                let out = match self.conns.get_mut(&id).map(|c| &mut c.role) {
                    Some(Role::Joiner {
                        joiner: Some(joiner),
                        ..
                    }) => joiner.on_message(&message, &mut self.machine, Self::unix()),
                    _ => Vec::new(),
                };
                self.desk_changed(&before);
                self.joiner_outputs(id, out);
            }
        }
    }

    fn member_message(&mut self, id: ConnId, message: &PairingMessage<'_>) {
        let before = self.machine.desk().clone();
        let result = match self.conns.get_mut(&id).map(|c| &mut c.role) {
            Some(Role::Member { inbox, .. }) => inbox.on_message(message, &mut self.machine),
            _ => return,
        };
        self.desk_changed(&before);
        if result.is_err() {
            self.close(id, CloseReason::ProtocolError, None);
            return;
        }
        let Some(conn) = self.conns.get(&id) else {
            return;
        };
        let (dialed, peer, out) = (conn.dialed, conn.peer, conn.out.clone());
        let Role::Member { inbox, up, .. } = &conn.role else {
            return;
        };
        if *up || !inbox.is_done() {
            return;
        }
        if dialed {
            self.mark_up(id);
            return;
        }
        // The listener: is the dialer a member now that its records are in?
        if !self.machine.desk().is_member(&peer) {
            self.close(id, CloseReason::NotAMember, None);
            return;
        }
        for frame in self.machine.record_frames() {
            let _ = out.send(Out::Frame(frame));
        }
        if let Some(Role::Member { sent_ours, .. }) = self.conns.get_mut(&id).map(|c| &mut c.role) {
            *sent_ours = true;
        }
        self.mark_up(id);
    }

    /// Records where `peer` was reached, reporting the addresses if that
    /// changed them.
    fn reached(&mut self, peer: PeerId, addr: SocketAddr) {
        if self.last_reached.get(&peer) != Some(&addr) {
            self.last_reached.insert(peer, addr);
            self.emit(Event::Addresses(self.last_reached.clone()));
        }
    }

    fn mark_up(&mut self, id: ConnId) {
        let me = self.me;
        let Some(conn) = self.conns.get_mut(&id) else {
            return;
        };
        if let Role::Member { up, .. } = &mut conn.role {
            *up = true;
        }
        if conn.stranger {
            conn.stranger = false;
            conn.stranger_until = None;
            self.strangers = self.strangers.saturating_sub(1);
        }
        let peer = conn.peer;
        let new_dialer = if conn.dialed { me } else { peer };
        let remote = conn.connection.remote_address();
        if let Some(redial) = self.redial.get_mut(&peer) {
            redial.backoff.reset();
            redial.dialing = false;
        }
        self.reached(peer, remote);
        match self.up.get(&peer).copied() {
            None => {
                self.up.insert(peer, id);
                if self.limbo.remove(&peer).is_none() {
                    self.emit(Event::PeerUp(peer));
                }
            }
            Some(existing) if existing != id => {
                let lower = me.min(peer);
                let old_dialer = self
                    .conns
                    .get(&existing)
                    .map_or(peer, |c| if c.dialed { me } else { peer });
                if new_dialer == lower && old_dialer != lower {
                    self.supersede(existing);
                    self.up.insert(peer, id);
                } else {
                    self.supersede(id);
                }
            }
            Some(_) => {}
        }
    }

    fn supersede(&mut self, id: ConnId) {
        if let Some(conn) = self.conns.get_mut(&id) {
            conn.superseded = true;
            if let Role::Member { up, .. } = &mut conn.role {
                *up = false;
            }
            conn.send(Out::Close(CloseReason::Normal));
        }
    }

    fn close(&mut self, id: ConnId, reason: CloseReason, down: Option<Down>) {
        if let Some(conn) = self.conns.get_mut(&id) {
            if down.is_some() {
                conn.local_down = down;
            }
            conn.send(Out::Close(reason));
        }
    }

    fn closed(&mut self, id: ConnId, cause: Cause) {
        let Some(conn) = self.conns.remove(&id) else {
            return;
        };
        if conn.stranger {
            self.strangers = self.strangers.saturating_sub(1);
        }
        match conn.role {
            Role::Inviter { .. } => {
                let out = self.mode.on_closed(id);
                self.mode_outputs(out);
            }
            Role::Joiner { race, joiner } => self.candidate_closed(id, race, joiner, cause),
            Role::Member { .. } => self.member_closed(id, &conn, cause),
        }
    }

    fn other_member_conn(&self, peer: PeerId) -> bool {
        self.conns
            .values()
            .any(|c| c.peer == peer && c.is_live_member())
    }

    fn member_closed(&mut self, id: ConnId, conn: &Conn, cause: Cause) {
        let peer = conn.peer;
        let down = conn.local_down.unwrap_or(match cause {
            Cause::Remote(Some(CloseReason::Normal)) => Down::Goodbye,
            Cause::Remote(Some(CloseReason::NotAMember)) => Down::Removed,
            _ => Down::Lost,
        });
        if self.up.get(&peer) == Some(&id) {
            self.up.remove(&peer);
            if self.other_member_conn(peer) {
                self.limbo.insert(peer, down);
            } else {
                self.emit(Event::PeerDown(peer, down));
            }
            self.schedule_redial(peer);
        } else if self.limbo.contains_key(&peer) {
            if !self.other_member_conn(peer) && !self.up.contains_key(&peer) {
                if let Some(down) = self.limbo.remove(&peer) {
                    self.emit(Event::PeerDown(peer, down));
                }
                self.schedule_redial(peer);
            }
        } else if conn.dialed && !conn.superseded {
            self.schedule_redial(peer);
        }
    }

    fn schedule_redial(&mut self, peer: PeerId) {
        if !self.machine.desk().is_member(&peer) || peer == self.me {
            return;
        }
        let now = self.now();
        let redial = self.redial.entry(peer).or_default();
        redial.dialing = false;
        let wait = u64::try_from(redial.backoff.wait().as_millis()).unwrap_or(u64::MAX);
        redial.next = Millis(now.0.saturating_add(wait));
    }

    // ---- membership -----------------------------------------------------

    /// Sends `records` to every member connection that has sent its own.
    fn push(&self, records: &[SignedRecord]) {
        for conn in self.conns.values() {
            if let Role::Member {
                sent_ours: true, ..
            } = conn.role
                && !conn.superseded
            {
                for record in records {
                    let mut frame = Vec::new();
                    let _ = record.wire().encode_frame(&mut frame);
                    conn.send(Out::Frame(frame));
                }
            }
        }
    }

    /// After anything that may have changed the view (design D9).
    fn desk_changed(&mut self, before: &Desk) {
        let after = self.machine.desk().clone();
        if *before == after {
            return;
        }
        self.emit(Event::TrustStore(self.machine.trust_store()));
        let forgotten = after.records().next().is_none() && before.records().next().is_some();
        if forgotten {
            for conn in self.conns.values_mut() {
                if matches!(conn.role, Role::Member { .. }) {
                    conn.local_down = Some(Down::Removed);
                    conn.send(Out::Close(CloseReason::Normal));
                }
            }
            self.redial.clear();
            return;
        }
        let changed: Vec<SignedRecord> = after
            .records()
            .filter(|r| before.record(&r.subject) != Some(*r))
            .cloned()
            .collect();
        self.push(&changed);
        let removed: Vec<ConnId> = self
            .conns
            .iter()
            .filter(|(_, c)| matches!(c.role, Role::Member { .. }) && !after.is_member(&c.peer))
            .filter(|(_, c)| before.is_member(&c.peer))
            .map(|(id, _)| *id)
            .collect();
        for id in removed {
            self.close(id, CloseReason::NotAMember, Some(Down::Removed));
        }
        let now = self.now();
        for (peer, _) in after.members() {
            if !before.is_member(&peer) {
                let redial = self.redial.entry(peer).or_default();
                redial.backoff.reset();
                redial.next = now;
            }
        }
        self.redial.retain(|peer, _| after.is_member(peer));
    }

    // ---- pairing --------------------------------------------------------

    fn advertise(&self, pairing: bool) {
        if let Some(mdns) = &self.mdns {
            let _ = mdns.advertise(pairing);
        }
    }

    fn mode_outputs(&mut self, outputs: Vec<ModeOutput>) {
        for output in outputs {
            match output {
                ModeOutput::Send(id, frame) => {
                    if let Some(conn) = self.conns.get(&id) {
                        conn.send(Out::Frame(frame));
                    }
                }
                ModeOutput::Close(id, reason) => self.close(id, reason, None),
                ModeOutput::Paired { conn, peer, name } => {
                    self.emit(Event::Paired { peer, name });
                    if let Some(c) = self.conns.get(&conn) {
                        let addr = c.connection.remote_address();
                        self.reached(peer, addr);
                    }
                    self.dial_soon(peer);
                }
                ModeOutput::Closed(why) => {
                    self.advertise(false);
                    self.emit(Event::PairingClosed(why));
                }
            }
        }
    }

    fn dial_soon(&mut self, peer: PeerId) {
        let now = self.now();
        let redial = self.redial.entry(peer).or_default();
        redial.backoff.reset();
        redial.next = now;
    }

    fn join(&mut self, code: Code) {
        if let Some(old) = self.race.take() {
            self.close_candidates(old.id, None);
        }
        let sightings: Vec<Sighting> = self.sightings.values().cloned().collect();
        let candidates = net::joiner_candidates(&code, &self.network(), &sightings, self.dial_port);
        if candidates.is_empty() {
            self.emit(Event::JoinFailed(JoinFailure::Unreachable));
            return;
        }
        let id = self.next_race;
        self.next_race += 1;
        self.race = Some(Race {
            id,
            code,
            pending: candidates.len(),
            winner: None,
            reasons: Vec::new(),
        });
        for addr in candidates {
            self.dial(vec![addr], Expect::Any, ALPN_PAIR, Purpose::Join(id));
        }
    }

    /// The first candidate to answer with `PairPake` carries on; true if `id`
    /// is it.
    fn claim_race(&mut self, race: u64, id: ConnId) -> bool {
        let Some(r) = self.race.as_mut().filter(|r| r.id == race) else {
            return false;
        };
        match r.winner {
            Some(winner) => winner == id,
            None => {
                r.winner = Some(id);
                self.close_candidates(race, Some(id));
                true
            }
        }
    }

    fn close_candidates(&mut self, race: u64, keep: Option<ConnId>) {
        let others: Vec<ConnId> = self
            .conns
            .iter()
            .filter(|(id, c)| {
                matches!(c.role, Role::Joiner { race: r, .. } if r == race) && Some(**id) != keep
            })
            .map(|(id, _)| *id)
            .collect();
        for id in others {
            self.close(id, CloseReason::Normal, None);
        }
    }

    fn joiner_outputs(&mut self, id: ConnId, outputs: Vec<JoinerOutput>) {
        for output in outputs {
            match output {
                JoinerOutput::Send(frame) => {
                    if let Some(conn) = self.conns.get(&id) {
                        conn.send(Out::Frame(frame));
                    }
                }
                JoinerOutput::Close(reason) => self.close(id, reason, None),
                JoinerOutput::Paired { peer, name } => {
                    if let Some(c) = self.conns.get(&id) {
                        let addr = c.connection.remote_address();
                        self.reached(peer, addr);
                    }
                    if let Some(race) = self.race.take() {
                        self.close_candidates(race.id, Some(id));
                    }
                    self.emit(Event::Paired { peer, name });
                    self.dial_soon(peer);
                }
                JoinerOutput::Failed(failure) => {
                    if let Some(race) = self.race.take() {
                        self.close_candidates(race.id, Some(id));
                    }
                    self.emit(Event::JoinFailed(failure));
                }
            }
        }
    }

    fn candidate_closed(&mut self, id: ConnId, race: u64, joiner: Option<Joiner>, cause: Cause) {
        let Some(r) = self.race.as_mut().filter(|r| r.id == race) else {
            return;
        };
        if r.winner == Some(id) {
            let failure = joiner
                .map(|mut j| j.on_closed(cause.reason()))
                .unwrap_or(Some(JoinFailure::ConnectionLost));
            self.race = None;
            if let Some(failure) = failure {
                self.emit(Event::JoinFailed(failure));
            }
            return;
        }
        if r.winner.is_some() {
            return;
        }
        r.pending = r.pending.saturating_sub(1);
        r.reasons.push(cause.reason());
        self.finish_race_if_done();
    }

    fn finish_race_if_done(&mut self) {
        let done = self
            .race
            .as_ref()
            .is_some_and(|r| r.winner.is_none() && r.pending == 0);
        if !done {
            return;
        }
        let Some(r) = self.race.take() else {
            return;
        };
        let any = |reason| r.reasons.contains(&Some(reason));
        let failure = if any(CloseReason::WrongPairingCode) {
            JoinFailure::WrongCode
        } else if any(CloseReason::NotReadyToPair) {
            JoinFailure::NotReady
        } else {
            JoinFailure::Unreachable
        };
        self.emit(Event::JoinFailed(failure));
    }

    // ---- dialing and time -----------------------------------------------

    fn dial(&self, addrs: Vec<SocketAddr>, expect: Expect, alpn: &'static [u8], purpose: Purpose) {
        let node = self.internal.clone();
        let Ok(config) = endpoint::client(&self.credentials, expect, alpn) else {
            let _ = node.send(Internal::DialFailed(purpose, vec![None]));
            return;
        };
        let endpoint = self.endpoint.clone();
        tokio::spawn(async move {
            let mut attempts = JoinSet::new();
            for addr in addrs {
                let endpoint = endpoint.clone();
                let config = config.clone();
                attempts.spawn(async move {
                    let connecting = endpoint
                        .connect_with(config, addr, SERVER_NAME)
                        .map_err(|_| None)?;
                    match tokio::time::timeout(DIAL_TIMEOUT, connecting).await {
                        Ok(Ok(connection)) => Ok(connection),
                        Ok(Err(e)) => Err(Cause::from_error(&e).reason()),
                        Err(_) => Err(None),
                    }
                });
            }
            let mut reasons = Vec::new();
            while let Some(result) = attempts.join_next().await {
                match result {
                    Ok(Ok(connection)) => {
                        attempts.abort_all();
                        let _ = node.send(Internal::Dialed(connection, purpose));
                        return;
                    }
                    Ok(Err(reason)) => reasons.push(reason),
                    Err(_) => {}
                }
            }
            let _ = node.send(Internal::DialFailed(purpose, reasons));
        });
    }

    /// Where `peer` might be: its advertisement and where it was last reached.
    fn candidates(&self, peer: &PeerId) -> Vec<SocketAddr> {
        let mut out: Vec<SocketAddr> = self
            .sightings
            .get(peer)
            .map(|s| {
                s.addrs
                    .iter()
                    .map(|a| SocketAddr::from((*a, s.port)))
                    .collect()
            })
            .unwrap_or_default();
        if let Some(addr) = self.last_reached.get(peer)
            && !out.contains(addr)
        {
            out.push(*addr);
        }
        out
    }

    fn wants_dial(&self, peer: &PeerId) -> bool {
        !self.quiet
            && *peer != self.me
            && !self.up.contains_key(peer)
            && !self.other_member_conn(*peer)
            && !self.redial.get(peer).is_some_and(|r| r.dialing)
            && !self.candidates(peer).is_empty()
    }

    fn next_deadline(&self) -> Option<Millis> {
        let members = self.machine.desk().members().map(|(p, _)| p);
        let dials = members
            .filter(|p| self.wants_dial(p))
            .map(|p| self.redial.get(&p).map_or(Millis(0), |r| r.next));
        let strangers = self.conns.values().filter_map(|c| c.stranger_until);
        dials.chain(strangers).chain(self.mode.deadline()).min()
    }

    fn tick(&mut self) {
        let now = self.now();
        let out = self.mode.on_tick(now);
        self.mode_outputs(out);

        let expired: Vec<(ConnId, CloseReason)> = self
            .conns
            .iter()
            .filter(|(_, c)| c.stranger_until.is_some_and(|t| t <= now))
            .map(|(id, c)| {
                let reason = match c.role {
                    Role::Inviter { .. } => CloseReason::NotReadyToPair,
                    _ => CloseReason::NotAMember,
                };
                (*id, reason)
            })
            .collect();
        for (id, reason) in expired {
            if let Some(conn) = self.conns.get_mut(&id) {
                conn.stranger_until = None;
            }
            self.close(id, reason, None);
        }

        let due: Vec<PeerId> = self
            .machine
            .desk()
            .members()
            .map(|(p, _)| p)
            .filter(|p| self.wants_dial(p))
            .filter(|p| self.redial.get(p).is_none_or(|r| r.next <= now))
            .collect();
        for peer in due {
            self.redial.entry(peer).or_default().dialing = true;
            let addrs = self.candidates(&peer);
            self.dial(
                addrs,
                Expect::Peer(peer),
                ALPN_MEMBER,
                Purpose::Member(peer),
            );
        }
    }
}

/// `RecordsDone`'s frame, for tests that speak the protocol by hand.
pub fn records_done_frame() -> Vec<u8> {
    let mut out = Vec::new();
    let _ = RecordsDone.encode_frame(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff() {
        let mut b = Backoff::default();
        let waits: Vec<u64> = (0..9)
            .map(|_| u64::try_from(b.wait().as_millis()).unwrap())
            .collect();
        assert_eq!(
            waits,
            [
                500, 1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000, 30_000
            ]
        );
        b.reset();
        assert_eq!(b.wait(), Duration::from_millis(500));
    }
}
