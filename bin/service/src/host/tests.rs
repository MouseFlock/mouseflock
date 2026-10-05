//! Two hosts on loopback with fake agents over in-memory pipes: the whole
//! path from agent bytes through engine and node to the other machine's
//! agent (spec: service-host).

use std::net::Ipv4Addr;
use std::path::PathBuf;

use ipc::link::{AgentEnd, ServiceEnd, ServiceHandle};
use ipc::message::ToService;
use model::{InputAction, InputEvent, Key, Monitor, MonitorId, Origin, Point, Rect, Screen};
use pairing::MachineName;
use tokio::time::timeout;
use transport::discovery::Sighting;
use transport::net::{Interface, Network};

use super::*;

const WAIT: Duration = Duration::from_secs(5);
const LOCAL: Ipv4Addr = Ipv4Addr::LOCALHOST;
const CTRL: Key = Key::LEFT_CTRL;

fn temp_dir(tag: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("deskhop-host-{}-{tag}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// One machine: a host on loopback with discovery off.
struct Pc {
    id: PeerId,
    dir: PathBuf,
    log: Log,
    clock: Clock,
    running: Running,
    commands: mpsc::UnboundedSender<HostCommand>,
    links: mpsc::UnboundedSender<AgentLink>,
    reports: mpsc::UnboundedReceiver<Report>,
    seen: Vec<Report>,
}

fn pc(name: &str, dir: PathBuf) -> Pc {
    let log = Log::memory();
    let stored = store::load(&dir, MachineName::new(name).unwrap(), &log).unwrap();
    let id = stored.machine.peer_id();
    let mut node = transport::Config::new(stored.machine);
    node.bind = LOCAL.into();
    node.port = 0;
    node.dial_port = 9;
    node.discovery = false;
    node.network = Some(Network {
        interfaces: vec![Interface {
            addr: LOCAL,
            prefix: 8,
        }],
        default_route: Some(LOCAL),
    });
    node.addresses = stored.addresses;
    let (reports_tx, reports) = mpsc::unbounded_channel();
    let (links, links_rx) = mpsc::unbounded_channel();
    let (commands, commands_rx) = mpsc::unbounded_channel();
    let clock = Clock::new();
    let config = HostConfig {
        node,
        layouts: stored.layouts,
        store: store::start(dir.clone(), log.clone()),
        log: log.clone(),
        sas: Sas::Console,
        clock,
        reports: Some(reports_tx),
    };
    let running = start(config, links_rx, commands_rx).unwrap();
    Pc {
        id,
        dir,
        log,
        clock,
        running,
        commands,
        links,
        reports,
        seen: Vec::new(),
    }
}

impl Pc {
    async fn expect(&mut self, what: &str, want: impl Fn(&Report) -> bool) -> Report {
        let deadline = Instant::now() + WAIT;
        loop {
            match tokio::time::timeout_at(deadline, self.reports.recv()).await {
                Ok(Some(r)) => {
                    self.seen.push(r.clone());
                    if want(&r) {
                        return r;
                    }
                }
                _ => panic!("no {what}; seen {:?}", self.seen),
            }
        }
    }

    /// Waits until `peer` is up. The report may already have been seen: a
    /// node can report its new peer up before it reports the pairing.
    async fn up(&mut self, peer: PeerId) {
        while let Ok(r) = self.reports.try_recv() {
            self.seen.push(r);
        }
        let is_up = |r: &Report| matches!(r, Report::PeerUp { peer: p, .. } if *p == peer);
        let ups = self.seen.iter().filter(|r| is_up(r)).count();
        let downs = self
            .seen
            .iter()
            .filter(|r| matches!(r, Report::PeerDown { peer: p, .. } if *p == peer))
            .count();
        if ups > downs {
            return;
        }
        self.expect("peer up", is_up).await;
    }

    fn send(&self, command: HostCommand) {
        self.commands.send(command).unwrap();
    }

    async fn stop(self) {
        let (done, rx) = oneshot::channel();
        self.send(HostCommand::Stop(done));
        timeout(WAIT, rx).await.unwrap().unwrap();
        timeout(WAIT, self.running.task).await.unwrap().unwrap();
    }

    /// Links a fake agent with `monitor` as its one 1920×1080 monitor.
    async fn agent(&self, monitor: &str) -> Agent {
        let mut agent = self.bare_agent().await;
        agent.screen(monitor);
        agent
    }

    /// Links a fake agent that reports nothing yet.
    async fn bare_agent(&self) -> Agent {
        let (service_side, agent_side) = tokio::io::duplex(1 << 16);
        let clock = self.clock;
        let (service, agent) = tokio::join!(
            ServiceEnd::accept(service_side, move || clock.now()),
            AgentEnd::connect(agent_side, move || clock.now()),
        );
        let (handle, events) = service.unwrap();
        let (reports, commands) = agent.unwrap();
        let (ended, ended_rx) = oneshot::channel();
        self.links
            .send(AgentLink {
                handle,
                events,
                ended,
            })
            .unwrap();
        Agent {
            clock,
            reports,
            commands,
            ended: ended_rx,
            seen: Vec::new(),
        }
    }
}

struct Agent {
    clock: Clock,
    reports: ServiceHandle,
    commands: mpsc::UnboundedReceiver<ToAgent>,
    ended: oneshot::Receiver<()>,
    seen: Vec<ToAgent>,
}

impl Agent {
    fn screen(&mut self, id: &str) {
        let monitor = Monitor {
            id: MonitorId(id.into()),
            rect: Rect::new(0, 0, 1920, 1080),
            dpi: 96,
        };
        self.reports
            .send(ToService::Screen(Screen::new(vec![monitor])))
            .unwrap();
    }

    fn physical(&self, input: InputEvent) {
        self.reports
            .send(ToService::Input {
                at: self.clock.now(),
                input,
                origin: Origin::Physical,
            })
            .unwrap();
    }

    fn key(&self, key: Key, down: bool) {
        self.physical(InputEvent::Key { key, down });
    }

    /// Pushes the cursor out of the right edge.
    fn push_right(&self, times: usize) {
        for _ in 0..times {
            self.physical(InputEvent::Motion {
                dx: 5,
                dy: 0,
                cursor: Point::new(1919, 540),
            });
        }
    }

    async fn next(&mut self) -> ToAgent {
        match timeout(WAIT, self.commands.recv()).await {
            Ok(Some(c)) => {
                self.seen.push(c);
                c
            }
            _ => panic!("no command; seen {:?}", self.seen),
        }
    }

    async fn expect(&mut self, what: &str, want: impl Fn(&ToAgent) -> bool) -> ToAgent {
        let deadline = Instant::now() + WAIT;
        loop {
            match tokio::time::timeout_at(deadline, self.commands.recv()).await {
                Ok(Some(c)) => {
                    self.seen.push(c);
                    if want(&c) {
                        return c;
                    }
                }
                _ => panic!("no {what}; seen {:?}", self.seen),
            }
        }
    }
}

/// Pairs `b` with `a` through console commands, and waits for both up.
async fn pair(a: &mut Pc, b: &mut Pc) {
    let (reply, code) = oneshot::channel();
    a.send(HostCommand::OpenPairing(reply));
    let code = code.await.unwrap().unwrap();
    b.running.node.sighting(Sighting {
        id: a.id,
        addrs: vec![LOCAL],
        port: a.running.node.local_addr().port(),
        pairing: true,
    });
    b.send(HostCommand::Join(code));
    let (ida, idb) = (a.id, b.id);
    a.expect(
        "paired",
        |r| matches!(r, Report::Paired { peer, .. } if *peer == idb),
    )
    .await;
    b.expect(
        "paired",
        |r| matches!(r, Report::Paired { peer, .. } if *peer == ida),
    )
    .await;
    a.up(idb).await;
    b.up(ida).await;
}

async fn paired() -> (Pc, Pc) {
    let mut a = pc("A", temp_dir("a"));
    let mut b = pc("B", temp_dir("b"));
    pair(&mut a, &mut b).await;
    (a, b)
}

/// Lets the hosts take in what was sent.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(200)).await;
}

/// A and B paired, each with an agent, and A's input on B after learning
/// that B is to A's right.
async fn forwarding() -> (Pc, Pc, Agent, Agent) {
    let (mut a, b) = paired().await;
    let mut agent_a = a.agent("A1").await;
    let mut agent_b = b.agent("B1").await;
    assert_eq!(
        agent_a.next().await,
        ToAgent::SetCapture(CaptureMode::PassAll)
    );
    assert_eq!(
        agent_b.next().await,
        ToAgent::SetCapture(CaptureMode::PassAll)
    );
    settle().await;
    agent_a.push_right(5);
    agent_a
        .expect("withhold all", |c| {
            *c == ToAgent::SetCapture(CaptureMode::WithholdAll)
        })
        .await;
    agent_b
        .expect("withhold mouse", |c| {
            *c == ToAgent::SetCapture(CaptureMode::WithholdMouse)
        })
        .await;
    let idb = b.id;
    a.expect(
        "learned",
        |r| matches!(r, Report::Learned { peer, .. } if *peer == idb),
    )
    .await;
    (a, b, agent_a, agent_b)
}

#[tokio::test(flavor = "multi_thread")]
async fn pairing_through_commands() {
    let (a, b) = paired().await;
    let (reply, rows) = oneshot::channel();
    a.send(HostCommand::Peers(reply));
    assert_eq!(
        rows.await.unwrap(),
        [PeerRow {
            peer: b.id,
            name: "B".into(),
            up: true
        }]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cursor_crosses() {
    let (_a, _b, agent_a, mut agent_b) = forwarding().await;
    // B was told where the cursor enters before it withheld the mouse.
    let warp = agent_b
        .seen
        .iter()
        .position(|c| matches!(c, ToAgent::Warp(_)));
    let withhold = agent_b
        .seen
        .iter()
        .position(|c| *c == ToAgent::SetCapture(CaptureMode::WithholdMouse));
    assert!(warp.is_some() && warp < withhold, "{:?}", agent_b.seen);
    assert_eq!(
        agent_b.seen[warp.unwrap()],
        ToAgent::Warp(Point::new(0, 540))
    );
    // The user's movement arrives as injected motion.
    agent_a.physical(InputEvent::Motion {
        dx: 3,
        dy: 1,
        cursor: Point::new(1919, 540),
    });
    agent_b
        .expect("injected motion", |c| {
            matches!(c, ToAgent::Inject(InputAction::Motion { .. }))
        })
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn new_link_gets_capture_mode_first() {
    let (a, mut b) = paired().await;
    let mut agent_a = a.agent("A1").await;
    assert_eq!(
        agent_a.next().await,
        ToAgent::SetCapture(CaptureMode::PassAll)
    );
    settle().await;
    // B has no agent when A's cursor enters it.
    agent_a.push_right(5);
    let ida = a.id;
    b.expect(
        "controlled",
        |r| matches!(r, Report::Focus { focus: FocusView::Controlled(p), .. } if *p == ida),
    )
    .await;
    let mut agent_b = b.bare_agent().await;
    assert_eq!(
        agent_b.next().await,
        ToAgent::SetCapture(CaptureMode::WithholdMouse)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn link_lost_while_forwarding_returns_focus() {
    let (mut a, mut b, agent_a, mut agent_b) = forwarding().await;
    agent_a.key(CTRL, true);
    agent_b
        .expect("ctrl down", |c| {
            *c == ToAgent::Inject(InputAction::Key {
                key: CTRL,
                down: true,
            })
        })
        .await;
    let ended = agent_a.ended;
    drop(agent_a.reports);
    drop(agent_a.commands);
    // B releases Ctrl and takes focus back; A is local.
    agent_b
        .expect("ctrl up", |c| {
            *c == ToAgent::Inject(InputAction::Key {
                key: CTRL,
                down: false,
            })
        })
        .await;
    agent_b
        .expect("pass all", |c| {
            *c == ToAgent::SetCapture(CaptureMode::PassAll)
        })
        .await;
    a.expect("local", |r| {
        matches!(
            r,
            Report::Focus {
                focus: FocusView::Local,
                ..
            }
        )
    })
    .await;
    b.expect("local", |r| {
        matches!(
            r,
            Report::Focus {
                focus: FocusView::Local,
                ..
            }
        )
    })
    .await;
    // The supervisor learns the link ended.
    assert!(timeout(WAIT, ended).await.is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn peer_down_while_controlled_passes_all() {
    let (a, mut b, _agent_a, mut agent_b) = forwarding().await;
    // A's node goes away without a goodbye.
    a.running.node.shutdown();
    let ida = a.id;
    b.expect(
        "A down",
        |r| matches!(r, Report::PeerDown { peer, .. } if *peer == ida),
    )
    .await;
    agent_b
        .expect("pass all", |c| {
            *c == ToAgent::SetCapture(CaptureMode::PassAll)
        })
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_keeps_desk_and_layout() {
    let (a, mut b, agent_a, agent_b) = forwarding().await;
    drop((agent_a, agent_b));
    let dir = a.dir.clone();
    a.stop().await;
    let ida = b.seen.iter().find_map(|r| match r {
        Report::Paired { peer, .. } => Some(*peer),
        _ => None,
    });

    // A restarts from its files on a new port; it dials B where it last
    // reached it.
    let mut a = pc("A", dir);
    assert_eq!(Some(a.id), ida);
    let idb = b.id;
    a.up(idb).await;
    b.up(a.id).await;
    let mut agent_a = a.agent("A1").await;
    let mut agent_b = b.agent("B1").await;
    agent_a.next().await;
    agent_b.next().await;
    settle().await;
    // One push crosses: the side is already known, so nothing is learned.
    agent_a.push_right(1);
    agent_b
        .expect("warp", |c| matches!(c, ToAgent::Warp(_)))
        .await;
    assert!(
        !a.seen.iter().any(|r| matches!(r, Report::Learned { .. })),
        "{:?}",
        a.seen
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn suspend_says_goodbye() {
    let (a, mut b, _agent_a, mut agent_b) = forwarding().await;
    let (done, rx) = std::sync::mpsc::channel();
    a.send(HostCommand::Suspend(done));
    tokio::task::spawn_blocking(move || rx.recv_timeout(Duration::from_secs(2)))
        .await
        .unwrap()
        .unwrap();
    let ida = a.id;
    b.expect(
        "goodbye",
        |r| matches!(r, Report::PeerDown { peer, why: Down::Goodbye, .. } if *peer == ida),
    )
    .await;
    agent_b
        .expect("pass all", |c| {
            *c == ToAgent::SetCapture(CaptureMode::PassAll)
        })
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_reconnects() {
    let (mut a, mut b) = paired().await;
    let (done, rx) = std::sync::mpsc::channel();
    a.send(HostCommand::Suspend(done));
    tokio::task::spawn_blocking(move || rx.recv_timeout(Duration::from_secs(2)))
        .await
        .unwrap()
        .unwrap();
    let ida = a.id;
    b.expect(
        "goodbye",
        |r| matches!(r, Report::PeerDown { peer, .. } if *peer == ida),
    )
    .await;
    let idb = b.id;
    a.expect(
        "B down",
        |r| matches!(r, Report::PeerDown { peer, .. } if *peer == idb),
    )
    .await;
    a.send(HostCommand::Resume);
    a.up(idb).await;
    b.up(ida).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_while_forwarding() {
    let (a, mut b, agent_a, mut agent_b) = forwarding().await;
    agent_a.key(CTRL, true);
    agent_b
        .expect("ctrl down", |c| {
            matches!(c, ToAgent::Inject(InputAction::Key { down: true, .. }))
        })
        .await;
    let Agent { ended, .. } = agent_a;
    let dir = a.dir.clone();
    let ida = a.id;
    a.stop().await;
    b.expect(
        "goodbye",
        |r| matches!(r, Report::PeerDown { peer, why: Down::Goodbye, .. } if *peer == ida),
    )
    .await;
    agent_b
        .expect("ctrl up", |c| {
            *c == ToAgent::Inject(InputAction::Key {
                key: CTRL,
                down: false,
            })
        })
        .await;
    assert!(timeout(WAIT, ended).await.is_ok());
    // Pending writes finished before the stop completed.
    for name in [store::TRUST_STORE, store::ADDRESSES, store::LAYOUTS] {
        assert!(dir.join(name).exists(), "{name}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn log_has_no_input() {
    let (a, b, agent_a, mut agent_b) = forwarding().await;
    let usages: Vec<u16> = (0x04..0x18).collect();
    for &u in &usages {
        agent_a.key(Key(u), true);
        agent_a.key(Key(u), false);
    }
    agent_b
        .expect("last key", |c| {
            *c == ToAgent::Inject(InputAction::Key {
                key: Key(0x17),
                down: false,
            })
        })
        .await;
    for line in a.log.lines().iter().chain(b.log.lines().iter()) {
        let text = &line[24..]; // after the timestamp
        assert!(
            !text.contains("Key") && !text.contains("Inject") && !text.contains("usage"),
            "{line}"
        );
        for u in &usages {
            assert!(!text.contains(&format!("{u:#04x}")), "{line}");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn console_prints_learned_side_with_undo_hint() {
    let (a, _b, _agent_a, _agent_b) = forwarding().await;
    let learned = a
        .seen
        .iter()
        .find(|r| matches!(r, Report::Learned { .. }))
        .unwrap();
    assert_eq!(
        crate::console::describe(learned),
        "learned: B is to the right; type `undo` within 10 s to take it back"
    );
}
