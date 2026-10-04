//! Loopback nodes, event waiting, raw clients, and a UDP relay that can be
//! cut.

#![allow(dead_code)]

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use model::PeerId;
use pairing::{Identity, Machine, MachineName, UnixTime};
use rand_core::OsRng;
use tokio::net::UdpSocket;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::{Instant, timeout};
use transport::discovery::Sighting;
use transport::net::{Interface, Network};
use transport::tls::{self, Credentials, Expect, SERVER_NAME};
use transport::{Config, Event, NodeHandle};

pub const LOCAL: Ipv4Addr = Ipv4Addr::LOCALHOST;

pub fn machine(name: &str) -> Machine {
    Machine::new(
        Identity::generate(&mut OsRng),
        MachineName::new(name).unwrap(),
    )
}

/// A copy of `machine` as it would be read back from its files.
pub fn reload(machine: &Machine) -> Machine {
    let identity = Identity::from_file(&*machine.identity().to_file()).unwrap();
    let mut copy = Machine::new(identity, machine.name().clone());
    copy.load_trust_store(&machine.trust_store()).unwrap();
    copy
}

/// Makes `a` and `b` members of each other's desks without the network.
pub fn befriend(a: &mut Machine, b: &mut Machine) {
    let now = UnixTime(1);
    a.add(b.peer_id(), b.name().clone(), now).unwrap();
    b.add(a.peer_id(), a.name().clone(), now).unwrap();
    let from_a: Vec<_> = a.desk().records().cloned().collect();
    let from_b: Vec<_> = b.desk().records().cloned().collect();
    a.apply(from_b);
    b.apply(from_a);
}

pub fn network(addr: Ipv4Addr) -> Network {
    Network {
        interfaces: vec![Interface { addr, prefix: 8 }],
        default_route: Some(addr),
    }
}

/// A loopback config: no mDNS, interface `127.0.0.1/8`, and a dial port with
/// nothing behind it.
pub fn config(machine: Machine) -> Config {
    let mut config = Config::new(machine);
    config.bind = LOCAL.into();
    config.port = 0;
    config.dial_port = 9;
    config.discovery = false;
    config.network = Some(network(LOCAL));
    config
}

pub struct Node {
    pub handle: NodeHandle,
    pub events: UnboundedReceiver<Event>,
    pub id: PeerId,
    pub addr: SocketAddr,
    pub seen: Vec<Event>,
}

pub fn start(config: Config) -> Node {
    let id = config.machine.peer_id();
    let (handle, events) = transport::start(config).unwrap();
    let addr = handle.local_addr();
    Node {
        handle,
        events,
        id,
        addr,
        seen: Vec::new(),
    }
}

pub fn node(machine: Machine) -> Node {
    start(config(machine))
}

impl Node {
    /// Waits up to `within` for an event matching `want`, keeping every event
    /// seen.
    pub async fn expect(&mut self, within: Duration, want: impl Fn(&Event) -> bool) -> Event {
        let deadline = Instant::now() + within;
        loop {
            match timeout(
                deadline.saturating_duration_since(Instant::now()),
                self.events.recv(),
            )
            .await
            {
                Ok(Some(event)) => {
                    self.seen.push(event.clone());
                    if want(&event) {
                        return event;
                    }
                }
                Ok(None) => panic!("node stopped; seen {:?}", self.seen),
                Err(_) => panic!("timed out after {within:?}; seen {:?}", self.seen),
            }
        }
    }

    /// An event matching `want` that was already seen, or the next one.
    pub async fn saw(&mut self, within: Duration, want: impl Fn(&Event) -> bool) -> Event {
        if let Some(event) = self.seen.iter().find(|e| want(e)) {
            return event.clone();
        }
        self.expect(within, want).await
    }

    /// Collects events for `period`, failing if any matches `forbidden`.
    pub async fn none_for(&mut self, period: Duration, forbidden: impl Fn(&Event) -> bool) {
        let deadline = Instant::now() + period;
        while let Ok(Some(event)) = timeout(
            deadline.saturating_duration_since(Instant::now()),
            self.events.recv(),
        )
        .await
        {
            assert!(
                !forbidden(&event),
                "unexpected {event:?}; seen {:?}",
                self.seen
            );
            self.seen.push(event);
        }
    }

    pub async fn up(&mut self, peer: PeerId) {
        self.expect(Duration::from_secs(5), |e| *e == Event::PeerUp(peer))
            .await;
    }

    /// Tells this node that `other` is at its address.
    pub fn sees(&self, other: &Node, pairing: bool) {
        self.sees_at(other.id, other.addr, pairing);
    }

    pub fn sees_at(&self, id: PeerId, addr: SocketAddr, pairing: bool) {
        let SocketAddr::V4(v4) = addr else {
            panic!("IPv4 only")
        };
        self.handle.sighting(Sighting {
            id,
            addrs: vec![*v4.ip()],
            port: v4.port(),
            pairing,
        });
    }
}

/// Two nodes already in one desk, connected.
pub async fn connected_pair() -> (Node, Node) {
    let (mut a, mut b) = (machine("A"), machine("B"));
    befriend(&mut a, &mut b);
    let (mut a, mut b) = (node(a), node(b));
    a.sees(&b, false);
    let (ida, idb) = (a.id, b.id);
    a.up(idb).await;
    b.up(ida).await;
    (a, b)
}

/// Pairs `joiner` with `inviter` by code over loopback, through a `pair=1`
/// sighting, and waits until both report the other up.
pub async fn pair(joiner: &mut Node, inviter: &mut Node) {
    let code = inviter.handle.open_pairing().await.unwrap();
    joiner.sees(inviter, true);
    joiner.handle.join(code);
    let (j, i) = (joiner.id, inviter.id);
    joiner
        .expect(
            Duration::from_secs(5),
            |e| matches!(e, Event::Paired { peer, .. } if *peer == i),
        )
        .await;
    inviter
        .expect(
            Duration::from_secs(5),
            |e| matches!(e, Event::Paired { peer, .. } if *peer == j),
        )
        .await;
    joiner.up(i).await;
    inviter.up(j).await;
}

/// A bare QUIC client with its own identity.
pub struct RawClient {
    pub identity: Identity,
    credentials: Credentials,
    endpoint: quinn::Endpoint,
}

impl RawClient {
    pub fn new() -> Self {
        let identity = Identity::generate(&mut OsRng);
        let credentials = Credentials::new(&identity).unwrap();
        let endpoint = quinn::Endpoint::client((LOCAL, 0).into()).unwrap();
        Self {
            identity,
            credentials,
            endpoint,
        }
    }

    pub async fn connect(&self, to: SocketAddr, expect: Expect, alpn: &[u8]) -> quinn::Connection {
        let config = transport::endpoint::client(&self.credentials, expect, alpn).unwrap();
        self.endpoint
            .connect_with(config, to, SERVER_NAME)
            .unwrap()
            .await
            .unwrap()
    }
}

/// The close code a connection ended with, if the peer closed it.
pub async fn close_code(connection: &quinn::Connection, within: Duration) -> Option<u64> {
    match timeout(within, connection.closed()).await {
        Ok(quinn::ConnectionError::ApplicationClosed(c)) => Some(c.error_code.into_inner()),
        _ => None,
    }
}

/// Forwards UDP between a client and `server` until cut. In lossy mode it
/// drops every third packet in each direction.
pub struct Relay {
    pub addr: SocketAddr,
    cut: Arc<AtomicBool>,
    lossy: Arc<AtomicBool>,
}

/// True if a packet goes through: always, unless lossy, when every third is
/// dropped.
fn passes(lossy: &AtomicBool, count: &AtomicU64) -> bool {
    !lossy.load(Ordering::SeqCst) || count.fetch_add(1, Ordering::SeqCst) % 3 != 2
}

impl Relay {
    pub async fn start(server: SocketAddr) -> Self {
        let front = Arc::new(UdpSocket::bind((LOCAL, 0)).await.unwrap());
        let back = Arc::new(UdpSocket::bind((LOCAL, 0)).await.unwrap());
        let addr = front.local_addr().unwrap();
        let cut = Arc::new(AtomicBool::new(false));
        let lossy = Arc::new(AtomicBool::new(false));
        let client = Arc::new(tokio::sync::Mutex::new(None::<SocketAddr>));
        {
            let (front, back, cut, lossy, client) = (
                front.clone(),
                back.clone(),
                cut.clone(),
                lossy.clone(),
                client.clone(),
            );
            tokio::spawn(async move {
                let count = AtomicU64::new(0);
                let mut buf = vec![0u8; 65_536];
                while let Ok((n, from)) = front.recv_from(&mut buf).await {
                    *client.lock().await = Some(from);
                    if !cut.load(Ordering::SeqCst) && passes(&lossy, &count) {
                        let _ = back.send_to(&buf[..n], server).await;
                    }
                }
            });
        }
        {
            let (front, back, cut, lossy, client) =
                (front, back, cut.clone(), lossy.clone(), client);
            tokio::spawn(async move {
                let count = AtomicU64::new(0);
                let mut buf = vec![0u8; 65_536];
                while let Ok((n, _)) = back.recv_from(&mut buf).await {
                    let to = *client.lock().await;
                    if let (false, Some(to)) = (cut.load(Ordering::SeqCst), to)
                        && passes(&lossy, &count)
                    {
                        let _ = front.send_to(&buf[..n], to).await;
                    }
                }
            });
        }
        Self { addr, cut, lossy }
    }

    pub fn cut(&self) {
        self.cut.store(true, Ordering::SeqCst);
    }

    pub fn restore(&self) {
        self.cut.store(false, Ordering::SeqCst);
    }

    /// Drops every third packet in each direction from now on.
    pub fn lossy(&self) {
        self.lossy.store(true, Ordering::SeqCst);
    }
}

/// A UDP port on loopback with nothing listening.
pub fn unused_port() -> u16 {
    let socket = std::net::UdpSocket::bind((LOCAL, 0)).unwrap();
    socket.local_addr().unwrap().port()
}

pub const SECOND: Duration = Duration::from_secs(1);

/// Lets `ms` pass.
pub async fn wait(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

pub fn tls_peer(connection: &quinn::Connection) -> Option<PeerId> {
    tls::connection_peer(connection)
}
