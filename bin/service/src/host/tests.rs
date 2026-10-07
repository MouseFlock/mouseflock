//! Two hosts on loopback with fake agents over in-memory pipes: the whole
//! path from agent bytes through engine and node to the other machine's
//! agent (spec: service-host).

use std::net::Ipv4Addr;
use std::path::PathBuf;

use ipc::clipboard::NotShared;
use ipc::link::{AgentEnd, AgentRole, FromService, ServiceEnd, ServiceHandle};
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
    uis: mpsc::UnboundedSender<UiLink>,
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
    let (uis, uis_rx) = mpsc::unbounded_channel();
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
    let running = start(config, links_rx, uis_rx, commands_rx).unwrap();
    Pc {
        id,
        dir,
        log,
        clock,
        running,
        commands,
        links,
        uis,
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

    /// Links a fake session agent that reports nothing yet.
    async fn bare_agent(&self) -> Agent {
        self.bare_agent_of(AgentRole::Session).await
    }

    /// Links a fake winlogon agent that reports nothing yet.
    async fn winlogon_agent(&self) -> Agent {
        self.bare_agent_of(AgentRole::Winlogon).await
    }

    async fn bare_agent_of(&self, role: AgentRole) -> Agent {
        let (service_side, agent_side) = tokio::io::duplex(1 << 16);
        let clock = self.clock;
        let (service, agent) = tokio::join!(
            ServiceEnd::accept(service_side, role, move || clock.now()),
            AgentEnd::connect(agent_side, role, move || clock.now()),
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
            set: Vec::new(),
        }
    }
}

struct Agent {
    clock: Clock,
    reports: ServiceHandle,
    commands: mpsc::UnboundedReceiver<FromService>,
    ended: oneshot::Receiver<()>,
    seen: Vec<ToAgent>,
    /// Clipboard text the service told this agent to set, in order.
    set: Vec<String>,
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

    /// The next command before `deadline`, recording clipboard text set on
    /// the way.
    async fn command_by(&mut self, deadline: Instant) -> Option<ToAgent> {
        loop {
            match tokio::time::timeout_at(deadline, self.commands.recv()).await {
                Ok(Some(FromService::Command(c))) => {
                    self.seen.push(c);
                    return Some(c);
                }
                Ok(Some(FromService::SetClipboard(text))) => self.set.push(text),
                _ => return None,
            }
        }
    }

    async fn next(&mut self) -> ToAgent {
        match self.command_by(Instant::now() + WAIT).await {
            Some(c) => c,
            None => panic!("no command; seen {:?}", self.seen),
        }
    }

    /// Reports a copy of `text`, as the agent does after the user copies.
    fn copy(&self, text: &str) {
        self.reports.send_clipboard_text(text).unwrap();
    }

    /// The next text the service tells this agent to put on the clipboard.
    async fn set_text(&mut self) -> String {
        if !self.set.is_empty() {
            return self.set.remove(0);
        }
        let deadline = Instant::now() + WAIT;
        loop {
            match tokio::time::timeout_at(deadline, self.commands.recv()).await {
                Ok(Some(FromService::SetClipboard(text))) => return text,
                Ok(Some(FromService::Command(c))) => self.seen.push(c),
                _ => panic!("no clipboard set; seen {:?}", self.seen),
            }
        }
    }

    /// Fails if the service tells this agent to set anything within
    /// `period`.
    async fn no_set_for(&mut self, period: Duration) {
        assert!(self.set.is_empty(), "set {:?}", self.set);
        let deadline = Instant::now() + period;
        while let Ok(Some(m)) = tokio::time::timeout_at(deadline, self.commands.recv()).await {
            match m {
                FromService::SetClipboard(text) => panic!("unexpected clipboard set {text:?}"),
                FromService::Command(c) => self.seen.push(c),
            }
        }
    }

    /// Reports, as the winlogon agent does, whether its desktop has the
    /// input.
    fn input_desktop(&self, active: bool) {
        self.reports.send(ToService::InputDesktop(active)).unwrap();
    }

    /// Fails if the service sends this agent a command `unwanted` within
    /// `period`.
    async fn none_for(&mut self, period: Duration, unwanted: impl Fn(&ToAgent) -> bool) {
        let deadline = Instant::now() + period;
        while let Some(c) = self.command_by(deadline).await {
            assert!(!unwanted(&c), "unexpected {c:?}");
        }
    }

    /// Waits for the link to end.
    async fn closed(&mut self) {
        let end = async { while self.commands.recv().await.is_some() {} };
        timeout(WAIT, end).await.expect("the link stayed up");
    }

    async fn expect(&mut self, what: &str, want: impl Fn(&ToAgent) -> bool) -> ToAgent {
        let deadline = Instant::now() + WAIT;
        loop {
            match self.command_by(deadline).await {
                Some(c) if want(&c) => return c,
                Some(_) => {}
                None => panic!("no {what}; seen {:?}", self.seen),
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
async fn winlogon_agent_gets_every_capture_mode() {
    let (a, b) = paired().await;
    let mut agent_a = a.agent("A1").await;
    let mut secure_a = a.winlogon_agent().await;
    let _agent_b = b.agent("B1").await;
    assert_eq!(
        agent_a.next().await,
        ToAgent::SetCapture(CaptureMode::PassAll)
    );
    assert_eq!(
        secure_a.next().await,
        ToAgent::SetCapture(CaptureMode::PassAll)
    );
    settle().await;
    agent_a.push_right(5);
    secure_a
        .expect("withhold all", |c| {
            *c == ToAgent::SetCapture(CaptureMode::WithholdAll)
        })
        .await;
    // A winlogon agent that links while forwarding withholds all first.
    let mut again = a.winlogon_agent().await;
    assert_eq!(
        again.next().await,
        ToAgent::SetCapture(CaptureMode::WithholdAll)
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

// ---- UIs (spec: ui-link) -------------------------------------------------------

use ipc::link::{ServiceUiEnd, ServiceUiHandle, UiEnd};
use ipc::message::{Hello, Role};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A UI client over an in-memory pipe.
struct Ui {
    to_service: ServiceUiHandle,
    events: mpsc::UnboundedReceiver<UiEvent>,
    seen: Vec<UiEvent>,
}

impl Pc {
    async fn ui(&self) -> Ui {
        let (service_side, ui_side) = tokio::io::duplex(1 << 16);
        let (service, ui) =
            tokio::join!(ServiceUiEnd::accept(service_side), UiEnd::connect(ui_side));
        let (handle, commands) = service.unwrap();
        self.uis.send(UiLink { handle, commands }).unwrap();
        let (to_service, events) = ui.unwrap();
        Ui {
            to_service,
            events,
            seen: Vec::new(),
        }
    }

    /// A UI that says `Hello` and then does only what the test does with
    /// the returned stream.
    async fn raw_ui(&self, buffer: usize) -> tokio::io::DuplexStream {
        let (service_side, mut ui_side) = tokio::io::duplex(buffer);
        let mut hello = Vec::new();
        Hello::this_release(Role::Ui)
            .encode_frame(&mut hello)
            .unwrap();
        ui_side.write_all(&hello).await.unwrap();
        let (handle, commands) = ServiceUiEnd::accept(service_side).await.unwrap();
        self.uis.send(UiLink { handle, commands }).unwrap();
        ui_side
    }
}

impl Ui {
    fn send(&self, command: UiCommand) {
        self.to_service.send(command).unwrap();
    }

    async fn expect(&mut self, what: &str, want: impl Fn(&UiEvent) -> bool) -> UiEvent {
        let deadline = Instant::now() + WAIT;
        loop {
            match tokio::time::timeout_at(deadline, self.events.recv()).await {
                Ok(Some(e)) => {
                    self.seen.push(e.clone());
                    if want(&e) {
                        return e;
                    }
                }
                _ => panic!("no {what}; seen {:?}", self.seen),
            }
        }
    }

    async fn status(&mut self, what: &str, want: impl Fn(&Status) -> bool) -> Status {
        match self
            .expect(what, |e| matches!(e, UiEvent::Status(s) if want(s)))
            .await
        {
            UiEvent::Status(s) => s,
            _ => unreachable!(),
        }
    }
}

fn member_up(s: &Status, key: PeerId, up: bool) -> bool {
    s.members.iter().any(|m| m.key == key.0 && m.up == up)
}

#[tokio::test(flavor = "multi_thread")]
async fn status_on_connect() {
    let a = pc("A", temp_dir("ui-a"));
    let mut ui = a.ui().await;
    let first = match timeout(WAIT, ui.events.recv()).await.unwrap().unwrap() {
        UiEvent::Status(s) => s,
        other => panic!("first event {other:?}"),
    };
    assert_eq!(first.key, a.id.0);
    assert_eq!(first.name, "A");
    assert!(!first.agent_linked);
    assert_eq!(first.pairing, Pairing::Closed);
    assert_eq!(first.focus, FocusOf::Local);
    assert!(first.members.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn status_after_peer_down() {
    let (a, mut b) = paired().await;
    let mut ui = b.ui().await;
    ui.status("A up", |s| member_up(s, a.id, true)).await;
    a.running.node.shutdown();
    let ida = a.id;
    b.expect(
        "A down",
        |r| matches!(r, Report::PeerDown { peer, .. } if *peer == ida),
    )
    .await;
    let reported = Instant::now();
    ui.status("A down", |s| member_up(s, ida, false)).await;
    assert!(
        reported.elapsed() < Duration::from_millis(200),
        "{:?}",
        reported.elapsed()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn burst_is_coalesced() {
    let a = pc("A", temp_dir("ui-burst"));
    let mut ui = a.ui().await;
    ui.status("first", |_| true).await;
    // 50 changes spread over about 100 ms (pairing opens and closes), sent
    // while this task reads, so arrival times are send times.
    let Ui {
        to_service,
        mut events,
        ..
    } = ui;
    let sender = tokio::spawn(async move {
        for _ in 0..25 {
            to_service.send(UiCommand::OpenPairing).unwrap();
            to_service.send(UiCommand::ClosePairing).unwrap();
            tokio::time::sleep(Duration::from_millis(4)).await;
        }
        to_service
    });
    // Collect every Status until the closed one after the last close settles.
    let mut times = Vec::new();
    let end = Instant::now() + Duration::from_secs(2);
    while let Ok(Some(e)) = tokio::time::timeout_at(end, events.recv()).await {
        if let UiEvent::Status(s) = e {
            times.push((Instant::now(), s));
        }
    }
    let _to_service = sender.await.unwrap();
    assert!(
        times.len() >= 2,
        "only {} Status during the burst",
        times.len()
    );
    let last = &times.last().expect("no Status after the burst").1;
    assert_eq!(last.pairing, Pairing::Closed);
    for pair in times.windows(2) {
        let gap = pair[1].0 - pair[0].0;
        // Sent at most every 100 ms; received with some scheduling delay.
        // Without coalescing they arrive microseconds apart.
        assert!(gap >= Duration::from_millis(50), "Status {gap:?} apart");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn open_pairing_shows_code() {
    let a = pc("A", temp_dir("ui-open"));
    let mut ui = a.ui().await;
    ui.send(UiCommand::OpenPairing);
    let s = ui
        .status("open", |s| matches!(s.pairing, Pairing::Open { .. }))
        .await;
    let Pairing::Open { code, left_ms } = s.pairing else {
        unreachable!()
    };
    assert!(Code::parse(&code).is_ok(), "{code}");
    assert!((590_000..=600_000).contains(&left_ms), "{left_ms}");
}

#[tokio::test(flavor = "multi_thread")]
async fn join_from_ui() {
    let mut a = pc("A", temp_dir("ui-join-a"));
    let b = pc("B", temp_dir("ui-join-b"));
    let mut ui_a = a.ui().await;
    let mut ui_b = b.ui().await;
    ui_a.send(UiCommand::OpenPairing);
    let s = ui_a
        .status("open", |s| matches!(s.pairing, Pairing::Open { .. }))
        .await;
    let Pairing::Open { code, .. } = s.pairing else {
        unreachable!()
    };
    b.running.node.sighting(Sighting {
        id: a.id,
        addrs: vec![LOCAL],
        port: a.running.node.local_addr().port(),
        pairing: true,
    });
    ui_b.send(UiCommand::Join(code));
    let ida = a.id;
    ui_b.expect(
        "paired with A",
        |e| matches!(e, UiEvent::Paired { key, name } if *key == ida.0 && name == "A"),
    )
    .await;
    ui_a.expect("paired with B", |e| matches!(e, UiEvent::Paired { .. }))
        .await;
    a.up(b.id).await;
    ui_a.status("closed and B listed", |s| {
        s.pairing == Pairing::Closed && s.members.iter().any(|m| m.name == "B")
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mistyped_code_is_reported() {
    let a = pc("A", temp_dir("ui-typo"));
    let mut ui = a.ui().await;
    // A real code with its last digit changed fails its check digit.
    let (reply, code) = oneshot::channel();
    a.send(HostCommand::OpenPairing(reply));
    let code = code.await.unwrap().unwrap().to_string();
    let (head, last) = code.split_at(code.len() - 1);
    let digit = (last.parse::<u8>().unwrap() + 1) % 10;
    ui.send(UiCommand::Join(format!("{head}{digit}")));
    ui.expect("mistyped", |e| {
        *e == UiEvent::JoinFailed(JoinFailedReason::MistypedCode)
    })
    .await;
    ui.send(UiCommand::Join("hello".into()));
    ui.expect("malformed", |e| {
        *e == UiEvent::JoinFailed(JoinFailedReason::MalformedCode)
    })
    .await;
    // No join was attempted, so no other failure follows.
    assert!(
        !ui.seen
            .iter()
            .any(|e| matches!(e, UiEvent::JoinFailed(r) if !matches!(r, JoinFailedReason::MistypedCode | JoinFailedReason::MalformedCode)))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_unknown_key_changes_nothing() {
    let (mut a, b) = paired().await;
    let mut ui = a.ui().await;
    ui.status("B up", |s| member_up(s, b.id, true)).await;
    ui.send(UiCommand::RemovePeer([0xee; 32]));
    tokio::time::sleep(Duration::from_millis(500)).await;
    while let Ok(r) = a.reports.try_recv() {
        assert!(!matches!(r, Report::PeerDown { .. }), "{r:?}");
    }
    // The link is still up: a change still reaches it.
    let _agent = a.bare_agent().await;
    ui.status("agent linked", |s| {
        s.agent_linked && member_up(s, b.id, true)
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn learned_reaches_every_ui() {
    let (a, b) = paired().await;
    let mut uis = [a.ui().await, a.ui().await];
    let mut agent_a = a.agent("A1").await;
    let mut agent_b = b.agent("B1").await;
    agent_a.next().await;
    agent_b.next().await;
    settle().await;
    agent_a.push_right(5);
    for ui in &mut uis {
        ui.expect("learned", |e| {
            *e == UiEvent::Learned {
                key: b.id.0,
                side: Side::Right,
                undo_ms: 10_000,
            }
        })
        .await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn stalled_ui_is_dropped() {
    let (a, _b) = paired().await;
    let mut agent = a.agent("A1").await;
    agent.next().await;
    let mut stalled = a.raw_ui(64).await;
    let mut ui = a.ui().await;
    ui.status("first", |_| true).await;
    // 300 notices: more than the stalled UI's 256-message allowance.
    for _ in 0..300 {
        ui.send(UiCommand::Join("hello".into()));
    }
    for _ in 0..300 {
        ui.expect("malformed", |e| matches!(e, UiEvent::JoinFailed(_)))
            .await;
    }
    // The working UI still gets changes, and the agent link is untouched.
    ui.send(UiCommand::OpenPairing);
    ui.status("open", |s| matches!(s.pairing, Pairing::Open { .. }))
        .await;
    assert!(matches!(
        agent.ended.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    // The stalled UI was cut off: reading what is left ends.
    let mut buf = vec![0u8; 4096];
    let ended = timeout(WAIT, async {
        loop {
            match stalled.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    })
    .await;
    assert!(ended.is_ok(), "the stalled UI stayed connected");
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_command_closes_only_that_ui() {
    let a = pc("A", temp_dir("ui-bad"));
    let mut raw = a.raw_ui(1 << 16).await;
    let mut ui = a.ui().await;
    ui.status("first", |_| true).await;
    // A `Join` whose code length field says 33.
    let mut frame = vec![34, 0, 0, 0, 0x32, 0x00, 33];
    frame.extend(std::iter::repeat_n(b'1', 33));
    raw.write_all(&frame).await.unwrap();
    // The service closes that UI: its reads end after what was sent before.
    let mut buf = vec![0u8; 1 << 16];
    let closed = timeout(WAIT, async {
        loop {
            match raw.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "the malformed UI stayed connected");
    ui.send(UiCommand::OpenPairing);
    ui.status("open", |s| matches!(s.pairing, Pairing::Open { .. }))
        .await;
}

// ---- clipboard --------------------------------------------------------------

/// A and B paired, each with an agent passing everything, focus local.
async fn linked() -> (Pc, Pc, Agent, Agent) {
    let (a, b) = paired().await;
    let mut agent_a = a.agent("A1").await;
    let mut agent_b = b.agent("B1").await;
    for agent in [&mut agent_a, &mut agent_b] {
        assert_eq!(
            agent.next().await,
            ToAgent::SetCapture(CaptureMode::PassAll)
        );
    }
    settle().await;
    (a, b, agent_a, agent_b)
}

/// Pushes A's cursor into B, learning B to A's right the first time.
async fn cross_to_b(agent_a: &mut Agent, agent_b: &mut Agent) {
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
}

/// B's own user presses a key, taking focus back; A is local again.
async fn b_takes_back(a: &mut Pc, agent_b: &Agent) {
    agent_b.key(Key(0x04), true);
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
}

#[tokio::test(flavor = "multi_thread")]
async fn copy_then_cross_sets_text_on_b() {
    let (_a, _b, mut agent_a, mut agent_b) = linked().await;
    agent_a.copy("deskhop copy on A");
    settle().await;
    agent_b.no_set_for(Duration::from_millis(100)).await;
    cross_to_b(&mut agent_a, &mut agent_b).await;
    assert_eq!(agent_b.set_text().await, "deskhop copy on A");
}

#[tokio::test(flavor = "multi_thread")]
async fn copy_on_b_then_focus_returns_to_a() {
    let (mut a, _b, mut agent_a, mut agent_b) = linked().await;
    cross_to_b(&mut agent_a, &mut agent_b).await;
    agent_b.copy("deskhop copy on B");
    settle().await;
    b_takes_back(&mut a, &agent_b).await;
    assert_eq!(agent_a.set_text().await, "deskhop copy on B");
}

#[tokio::test(flavor = "multi_thread")]
async fn private_copy_is_not_offered() {
    let (_a, _b, mut agent_a, mut agent_b) = linked().await;
    agent_a.copy("deskhop shared");
    agent_a
        .reports
        .send(ToService::ClipboardNotShared(NotShared::Private))
        .unwrap();
    settle().await;
    cross_to_b(&mut agent_a, &mut agent_b).await;
    // B asks for the shared copy it heard of; A no longer serves it.
    agent_b.no_set_for(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_newest_copy_is_sent() {
    let (_a, _b, mut agent_a, mut agent_b) = linked().await;
    agent_a.copy("deskhop first");
    agent_a.copy("deskhop second");
    settle().await;
    cross_to_b(&mut agent_a, &mut agent_b).await;
    assert_eq!(agent_b.set_text().await, "deskhop second");
    agent_b.no_set_for(Duration::from_millis(300)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn no_echo_after_set() {
    let (mut a, _b, mut agent_a, mut agent_b) = linked().await;
    agent_a.copy("deskhop round trip");
    settle().await;
    cross_to_b(&mut agent_a, &mut agent_b).await;
    assert_eq!(agent_b.set_text().await, "deskhop round trip");
    // Back on A, nothing newer exists: A is told to set nothing.
    b_takes_back(&mut a, &agent_b).await;
    agent_a.no_set_for(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn log_has_no_clipboard_text() {
    let (a, b, mut agent_a, mut agent_b) = linked().await;
    let secret = "zq-deskhop-distinctive-7731";
    agent_a.copy(secret);
    settle().await;
    cross_to_b(&mut agent_a, &mut agent_b).await;
    assert_eq!(agent_b.set_text().await, secret);
    let lines: Vec<String> = a.log.lines().into_iter().chain(b.log.lines()).collect();
    assert!(
        lines.iter().any(|l| l.contains("clipboard: set")),
        "{lines:#?}"
    );
    for line in &lines {
        assert!(!line.contains("zq-deskhop"), "{line}");
    }
}

// ---- secure desktop ---------------------------------------------------------

const KEY_A: Key = Key(0x04);
const KEY_B: Key = Key(0x05);

fn key_down(key: Key) -> ToAgent {
    ToAgent::Inject(InputAction::Key { key, down: true })
}

/// Links a winlogon agent to `pc` that reports `monitor` and that its
/// desktop has the input, and returns it after its first command.
async fn secure_desktop(pc: &Pc, monitor: &str, first: CaptureMode) -> Agent {
    let mut agent = pc.winlogon_agent().await;
    assert_eq!(agent.next().await, ToAgent::SetCapture(first));
    agent.screen(monitor);
    agent.input_desktop(true);
    settle().await;
    agent
}

#[tokio::test(flavor = "multi_thread")]
async fn typing_reaches_the_secure_desktop() {
    let (_a, b, agent_a, mut agent_b) = forwarding().await;
    let mut secure_b = secure_desktop(&b, "B1", CaptureMode::WithholdMouse).await;
    agent_a.key(KEY_A, true);
    secure_b
        .expect("the key on B's secure desktop", |c| *c == key_down(KEY_A))
        .await;
    // Motion from the crossing may still be queued; no key reaches B's
    // session agent.
    agent_b
        .none_for(Duration::from_millis(300), |c| {
            matches!(c, ToAgent::Inject(InputAction::Key { .. }))
        })
        .await;

    // The prompt is answered: keys reach B's session agent again.
    secure_b.input_desktop(false);
    settle().await;
    agent_a.key(KEY_B, true);
    agent_b
        .expect("the key on B's desktop", |c| *c == key_down(KEY_B))
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn forwarding_survives_a_uac_prompt_on_a() {
    let (a, _b, agent_a, mut agent_b) = forwarding().await;
    let secure_a = secure_desktop(&a, "A1", CaptureMode::WithholdAll).await;
    secure_a.key(KEY_A, true);
    agent_b
        .expect("A's key from its secure desktop", |c| *c == key_down(KEY_A))
        .await;
    // A's session agent is not on the input desktop: its reports are
    // dropped.
    agent_a.key(KEY_B, true);
    agent_b
        .none_for(Duration::from_millis(300), |c| *c == key_down(KEY_B))
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn losing_the_active_winlogon_agent_returns_focus() {
    let (mut a, _b, _agent_a, mut agent_b) = forwarding().await;
    let secure_a = secure_desktop(&a, "A1", CaptureMode::WithholdAll).await;
    drop(secure_a);
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
    agent_b
        .expect("pass all", |c| {
            *c == ToAgent::SetCapture(CaptureMode::PassAll)
        })
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn inactive_winlogon_agent_lost_changes_nothing() {
    let (mut a, _b, agent_a, mut agent_b) = forwarding().await;
    let mut secure_a = a.winlogon_agent().await;
    assert_eq!(
        secure_a.next().await,
        ToAgent::SetCapture(CaptureMode::WithholdAll)
    );
    drop(secure_a);
    a.expect("winlogon agent gone", |r| {
        matches!(r, Report::AgentGone(AgentRole::Winlogon))
    })
    .await;
    // Still forwarding: A's keys keep reaching B.
    agent_a.key(KEY_A, true);
    agent_b.expect("A's key", |c| *c == key_down(KEY_A)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn arrival_at_a_locked_machine_sets_clipboard_via_session_agent() {
    let (_a, b, mut agent_a, mut agent_b) = linked().await;
    let mut secure_b = secure_desktop(&b, "B1", CaptureMode::PassAll).await;
    agent_a.copy("deskhop copy for a locked B");
    settle().await;
    cross_to_b(&mut agent_a, &mut agent_b).await;
    assert_eq!(agent_b.set_text().await, "deskhop copy for a locked B");
    // The cursor entered B's secure desktop; no clipboard went there.
    secure_b
        .expect("the warp onto B's secure desktop", |c| {
            matches!(c, ToAgent::Warp(_))
        })
        .await;
    secure_b.no_set_for(Duration::from_millis(300)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_ends_both_agent_links() {
    let a = pc("A", temp_dir("stop-both"));
    let mut agent_a = a.agent("A1").await;
    let mut secure_a = a.winlogon_agent().await;
    for agent in [&mut agent_a, &mut secure_a] {
        assert_eq!(
            agent.next().await,
            ToAgent::SetCapture(CaptureMode::PassAll)
        );
    }
    a.stop().await;
    agent_a.closed().await;
    secure_a.closed().await;
}
