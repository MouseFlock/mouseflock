//! The engine loop (add-service design D3): one task owns the engine and
//! connects it to the session agent's link, the transport node, and every
//! connected UI (add-ui-link design D4).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use engine::{CaptureMode, Decision, Engine, Event as EngineEvent, FocusView, Tuning};
use ipc::clipboard::Copied;
use ipc::link::{AgentHandle, FromAgent, UiHandle};
use ipc::message::ToAgent;
use ipc::ui::{
    FocusOf, JoinFailedReason, Member, NotOpenedReason, Pairing, PairingClosedReason, Status,
    UiCommand, UiEvent,
};
use model::{LayoutBook, Millis, PeerId, Side};
use pairing::{Code, CodeError, Desk, JoinFailure, MachineName, ModeClosed, WINDOW_MS};
use rand_core::{OsRng, RngCore};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use transport::{Down, Event as NodeEvent, NoNetwork, NodeHandle, StartError};

use crate::clipboard::{self, Tracker};
use crate::log::Log;
use crate::store::{self, StoreHandle};
use crate::{info, warn};

/// How long goodbye may take before sleep or stop.
pub const GOODBYE_WAIT: Duration = Duration::from_secs(1);

/// UIs get a changed `Status` at most this often.
pub const STATUS_EVERY: Duration = Duration::from_millis(100);

/// The service's monotonic clock: milliseconds since start. The agent link
/// converts agent times to it, and `Undo` is stamped with it.
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    start: Instant,
}

impl Clock {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    pub fn now(&self) -> Millis {
        Millis(self.start.elapsed().as_millis() as u64)
    }
}

/// A session agent's link, handed over by the supervisor. Dropping it ends
/// the link; `ended` then tells the supervisor.
pub struct AgentLink {
    pub handle: AgentHandle,
    pub events: mpsc::UnboundedReceiver<FromAgent>,
    #[expect(dead_code, reason = "never sent on; dropping it is the signal")]
    pub ended: oneshot::Sender<()>,
}

/// A UI's link, handed over by the UI pipe server.
pub struct UiLink {
    pub handle: UiHandle,
    pub commands: mpsc::UnboundedReceiver<UiCommand>,
}

/// What the per-UI forwarding tasks tell the host.
enum FromUi {
    Command(UiCommand),
    Gone(u64),
}

/// A member, for `peers`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRow {
    pub peer: PeerId,
    pub name: String,
    pub up: bool,
}

pub enum HostCommand {
    OpenPairing(oneshot::Sender<Result<Code, NoNetwork>>),
    ClosePairing,
    Join(Code),
    Peers(oneshot::Sender<Vec<PeerRow>>),
    Remove(PeerId),
    Undo,
    ForgetLayout,
    /// The machine is going to sleep: say goodbye, then answer.
    Suspend(std::sync::mpsc::Sender<()>),
    Resume,
    /// End the current agent link (its session ended or switched).
    DropLink,
    /// Goodbye, end the link, finish writes, stop the node; then answer.
    Stop(oneshot::Sender<()>),
}

/// What happened, for the console.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Report {
    PairingClosed(ModeClosed),
    Paired {
        peer: PeerId,
        name: String,
    },
    JoinFailed(JoinFailure),
    PeerUp {
        peer: PeerId,
        name: String,
    },
    PeerDown {
        peer: PeerId,
        name: String,
        why: Down,
    },
    Focus {
        focus: FocusView,
        name: Option<String>,
    },
    Learned {
        peer: PeerId,
        name: String,
        side: Side,
        undo_secs: u64,
    },
    AgentLinked,
    AgentGone,
}

/// How secure attention can be requested here (design D10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sas {
    /// The registered service: `SendSAS`, if policy allows.
    Service,
    /// Console mode: log that it needs the registered service.
    Console,
}

pub struct HostConfig {
    pub node: transport::Config,
    pub layouts: LayoutBook,
    pub store: StoreHandle,
    pub log: Log,
    pub sas: Sas,
    pub clock: Clock,
    pub reports: Option<mpsc::UnboundedSender<Report>>,
}

/// A running host.
pub struct Running {
    /// The node, for tests and for the listening address.
    pub node: NodeHandle,
    pub task: tokio::task::JoinHandle<()>,
}

/// The capture mode for a focus state.
pub fn mode_for(focus: FocusView) -> CaptureMode {
    match focus {
        FocusView::Local => CaptureMode::PassAll,
        FocusView::Forwarding(_) => CaptureMode::WithholdAll,
        FocusView::Controlled(_) => CaptureMode::WithholdMouse,
    }
}

/// Starts the node and the host loop. Call within a Tokio runtime.
pub fn start(
    config: HostConfig,
    links: mpsc::UnboundedReceiver<AgentLink>,
    uis: mpsc::UnboundedReceiver<UiLink>,
    commands: mpsc::UnboundedReceiver<HostCommand>,
) -> Result<Running, StartError> {
    let me = config.node.machine.peer_id();
    let my_name = config.node.machine.name().clone();
    let desk = config.node.machine.desk().clone();
    let (node, node_events) = transport::start(config.node)?;
    let mut engine = Engine::new(me, engine::Config::default());
    engine.handle(EngineEvent::LayoutsLoaded(config.layouts));
    let (from_ui, from_ui_rx) = mpsc::unbounded_channel();
    let host = Host {
        engine,
        node: node.clone(),
        me,
        my_name,
        desk,
        up: BTreeSet::new(),
        link: None,
        store: config.store,
        log: config.log,
        sas: config.sas,
        clock: config.clock,
        reports: config.reports,
        uis: BTreeMap::new(),
        next_ui: 0,
        from_ui,
        pairing: None,
        dirty: false,
        focus: FocusView::Local,
        clipboard: Tracker::default(),
    };
    let task = tokio::spawn(host.run(node_events, links, uis, from_ui_rx, commands));
    Ok(Running { node, task })
}

struct Host {
    engine: Engine,
    node: NodeHandle,
    me: PeerId,
    my_name: MachineName,
    /// A copy of the membership view, for names.
    desk: Desk,
    up: BTreeSet<PeerId>,
    link: Option<AgentLink>,
    store: StoreHandle,
    log: Log,
    sas: Sas,
    clock: Clock,
    reports: Option<mpsc::UnboundedSender<Report>>,
    /// Connected UIs by their number.
    uis: BTreeMap<u64, UiHandle>,
    next_ui: u64,
    from_ui: mpsc::UnboundedSender<FromUi>,
    /// The open pairing mode's code, and when it opened.
    pairing: Option<(Code, Millis)>,
    /// Something in `Status` changed since UIs were last sent one.
    dirty: bool,
    /// The focus the engine last reported, to see the cursor arrive.
    focus: FocusView,
    /// Which copy is newest and what to fetch or serve. Never logged.
    clipboard: Tracker,
}

/// The next event from the current link, or `None` when it ends. Pends
/// forever while there is no link.
async fn next_from(link: &mut Option<AgentLink>) -> Option<FromAgent> {
    match link {
        Some(l) => l.events.recv().await,
        None => std::future::pending().await,
    }
}

impl Host {
    async fn run(
        mut self,
        mut node_events: mpsc::UnboundedReceiver<NodeEvent>,
        mut links: mpsc::UnboundedReceiver<AgentLink>,
        mut uis: mpsc::UnboundedReceiver<UiLink>,
        mut from_ui: mpsc::UnboundedReceiver<FromUi>,
        mut commands: mpsc::UnboundedReceiver<HostCommand>,
    ) {
        info!(self.log, "running as {}", self.short(self.me));
        let mut status_tick = tokio::time::interval(STATUS_EVERY);
        status_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                Some(event) = node_events.recv() => self.on_node(event),
                Some(ui) = uis.recv() => self.new_ui(ui),
                Some(event) = from_ui.recv() => match event {
                    FromUi::Command(command) => self.on_ui(command).await,
                    FromUi::Gone(id) => {
                        self.uis.remove(&id);
                    }
                },
                _ = status_tick.tick(), if self.dirty => self.send_status(),
                event = next_from(&mut self.link) => match event {
                    Some(event) => self.on_agent(event),
                    None => self.link_ended("the agent link closed"),
                },
                Some(link) = links.recv() => self.new_link(link),
                command = commands.recv() => match command {
                    Some(HostCommand::Stop(done)) => {
                        self.stop().await;
                        let _ = done.send(());
                        return;
                    }
                    Some(command) => self.on_command(command).await,
                    None => {
                        self.stop().await;
                        return;
                    }
                },
            }
        }
    }

    fn engine(&mut self, event: EngineEvent) {
        let decisions = self.engine.handle(event);
        self.apply(decisions);
    }

    fn report(&self, report: Report) {
        if let Some(r) = &self.reports {
            let _ = r.send(report);
        }
    }

    /// A member's name, or its key prefix.
    fn name(&self, peer: PeerId) -> String {
        self.desk
            .members()
            .find(|(p, _)| *p == peer)
            .map(|(_, n)| n.as_str().to_owned())
            .unwrap_or_else(|| self.short(peer))
    }

    fn short(&self, peer: PeerId) -> String {
        store::hex(&peer.0[..4])
    }

    // ---- UIs ----

    fn new_ui(&mut self, ui: UiLink) {
        let id = self.next_ui;
        self.next_ui = self.next_ui.wrapping_add(1);
        if ui.handle.send(UiEvent::Status(self.status())).is_err() {
            return;
        }
        let UiLink {
            handle,
            mut commands,
        } = ui;
        self.uis.insert(id, handle);
        let tx = self.from_ui.clone();
        tokio::spawn(async move {
            while let Some(command) = commands.recv().await {
                if tx.send(FromUi::Command(command)).is_err() {
                    return;
                }
            }
            let _ = tx.send(FromUi::Gone(id));
        });
        info!(self.log, "UI connected ({} now)", self.uis.len());
    }

    /// Sends `event` to every UI, dropping any whose link has closed or
    /// that stopped reading.
    fn broadcast(&mut self, event: &UiEvent) {
        self.uis
            .retain(|_, ui| !ui.is_closed() && ui.send(event.clone()).is_ok());
    }

    fn send_status(&mut self) {
        self.dirty = false;
        let status = UiEvent::Status(self.status());
        self.broadcast(&status);
    }

    /// Everything a UI shows, from the host's state now.
    fn status(&self) -> Status {
        let pairing = match &self.pairing {
            Some((code, opened)) => Pairing::Open {
                code: code.to_string(),
                left_ms: u32::try_from(WINDOW_MS.saturating_sub(self.clock.now().since(*opened)))
                    .unwrap_or(u32::MAX),
            },
            None => Pairing::Closed,
        };
        let focus = match self.engine.focus() {
            FocusView::Local => FocusOf::Local,
            FocusView::Forwarding(p) => FocusOf::On(p.0),
            FocusView::Controlled(p) => FocusOf::ControlledBy(p.0),
        };
        Status {
            key: self.me.0,
            name: self.my_name.as_str().to_owned(),
            agent_linked: self.link.is_some(),
            pairing,
            focus,
            members: self
                .desk
                .members()
                .take(ipc::ui::MAX_MEMBERS)
                .map(|(peer, name)| Member {
                    key: peer.0,
                    name: name.as_str().to_owned(),
                    up: self.up.contains(&peer),
                })
                .collect(),
        }
    }

    async fn on_ui(&mut self, command: UiCommand) {
        match command {
            UiCommand::OpenPairing => {
                let _ = self.open_pairing().await;
            }
            UiCommand::ClosePairing => self.node.close_pairing(),
            UiCommand::Join(text) => match Code::parse(&text) {
                Ok(code) => self.join(code),
                Err(e) => {
                    let reason = match e {
                        CodeError::CheckDigit => JoinFailedReason::MistypedCode,
                        _ => JoinFailedReason::MalformedCode,
                    };
                    info!(self.log, "join refused: {e}");
                    self.broadcast(&UiEvent::JoinFailed(reason));
                }
            },
            UiCommand::RemovePeer(key) => {
                let peer = PeerId(key);
                if self.desk.is_member(&peer) {
                    self.remove(peer);
                }
            }
            UiCommand::Undo => self.undo(),
            UiCommand::ForgetLayout => self.engine(EngineEvent::ForgetLayout),
        }
    }

    // ---- actions shared by the console and UIs ----

    async fn open_pairing(&mut self) -> Result<Code, NoNetwork> {
        let opened = self.node.open_pairing().await;
        match &opened {
            Ok(code) => {
                info!(self.log, "pairing opened");
                self.pairing = Some((code.clone(), self.clock.now()));
                self.dirty = true;
            }
            Err(e) => {
                warn!(self.log, "pairing not opened: {e}");
                self.broadcast(&UiEvent::PairingNotOpened(NotOpenedReason::NoNetwork));
            }
        }
        opened
    }

    fn join(&mut self, code: Code) {
        info!(self.log, "joining by code");
        self.node.join(code);
    }

    fn remove(&mut self, peer: PeerId) {
        info!(self.log, "removing {}", self.name(peer));
        self.node.remove(peer);
    }

    fn undo(&mut self) {
        let at = self.clock.now();
        self.engine(EngineEvent::Undo { at });
    }

    // ---- agent ----

    fn new_link(&mut self, link: AgentLink) {
        if self.link.is_some() {
            self.link_ended("replaced by a new agent link");
        }
        let mode = mode_for(self.engine.focus());
        let _ = link.handle.send(ToAgent::SetCapture(mode));
        self.link = Some(link);
        self.dirty = true;
        info!(self.log, "agent linked");
        self.report(Report::AgentLinked);
    }

    fn link_ended(&mut self, why: &str) {
        if self.link.take().is_none() {
            return;
        }
        self.dirty = true;
        info!(self.log, "agent link ended: {why}");
        self.report(Report::AgentGone);
        self.engine(EngineEvent::LocalInputLost);
    }

    fn on_agent(&mut self, event: FromAgent) {
        match event {
            FromAgent::Input { at, input, origin } => {
                self.engine(EngineEvent::Local { at, input, origin });
            }
            FromAgent::Screen(screen) => {
                info!(self.log, "screen: {} monitor(s)", screen.monitors.len());
                self.engine(EngineEvent::ScreenChanged(screen));
            }
            FromAgent::Clipboard(Copied::Text(text)) => {
                let id = OsRng.next_u64();
                info!(self.log, "clipboard: offered {} bytes", text.len());
                self.clipboard.local_text(id, Arc::from(text));
                for &peer in &self.up {
                    self.node.offer_clipboard(peer, id);
                }
            }
            FromAgent::Clipboard(Copied::NotShared(why)) => {
                info!(self.log, "clipboard: copy not shared ({why:?})");
                self.clipboard.local_not_shared();
            }
        }
    }

    /// The cursor arrived here: fetch the newest copy if a member holds it.
    fn cursor_arrived(&mut self) {
        let up = &self.up;
        let now = Instant::now().into_std();
        if let Some((peer, id)) = self.clipboard.arrived(|p| up.contains(&p), now) {
            self.node.request_clipboard(peer, id);
        }
    }

    fn on_clipboard(&mut self, event: NodeEvent) {
        match event {
            NodeEvent::ClipboardOffer { peer, id } => self.clipboard.offer(peer, id),
            NodeEvent::ClipboardRequest { peer, id } => {
                if let Some(text) = self.clipboard.request(id) {
                    info!(
                        self.log,
                        "clipboard: sent {} bytes to {}",
                        text.len(),
                        self.name(peer)
                    );
                    self.node.send_clipboard(peer, id, text);
                }
            }
            NodeEvent::ClipboardText { peer, id, text } => {
                if let Some(text) = self.clipboard.received(peer, id, text) {
                    info!(
                        self.log,
                        "clipboard: set {} bytes from {}",
                        text.len(),
                        self.name(peer)
                    );
                    if let Some(link) = &self.link {
                        let _ = link.handle.set_clipboard(&text);
                    }
                }
            }
            _ => {}
        }
    }

    fn to_agent(&self, command: ToAgent) {
        if let Some(link) = &self.link {
            // A failed send means the link is closing; its end arrives as
            // `None` from the receiver.
            let _ = link.handle.send(command);
        }
    }

    // ---- decisions ----

    fn apply(&mut self, decisions: Vec<Decision>) {
        for decision in decisions {
            match decision {
                Decision::SetCapture(mode) => self.to_agent(ToAgent::SetCapture(mode)),
                Decision::Inject(action) => self.to_agent(ToAgent::Inject(action)),
                Decision::WarpCursor(to) => self.to_agent(ToAgent::Warp(to)),
                Decision::Send { peer, msg } => self.node.send(peer, msg),
                Decision::RequestSecureAttention => self.secure_attention(),
                Decision::FocusChanged(focus) => {
                    let name = match focus {
                        FocusView::Local => None,
                        FocusView::Forwarding(p) | FocusView::Controlled(p) => Some(self.name(p)),
                    };
                    match (&focus, &name) {
                        (FocusView::Forwarding(_), Some(n)) => info!(self.log, "focus: on {n}"),
                        (FocusView::Controlled(_), Some(n)) => {
                            info!(self.log, "focus: controlled by {n}")
                        }
                        _ => info!(self.log, "focus: local"),
                    }
                    self.report(Report::Focus { focus, name });
                    self.dirty = true;
                    let before = std::mem::replace(&mut self.focus, focus);
                    if clipboard::arrived(before, focus) {
                        self.cursor_arrived();
                    }
                }
                Decision::LayoutsChanged(book) => {
                    self.store
                        .write(store::LAYOUTS, store::layouts_to_json(&book));
                }
                Decision::OfferUndo { peer, side } => {
                    let name = self.name(peer);
                    let undo_secs = Tuning::DEFAULT.undo_ms / 1000;
                    info!(self.log, "learned {name} to the {}", store::side_name(side));
                    self.broadcast(&UiEvent::Learned {
                        key: peer.0,
                        side,
                        undo_ms: u32::try_from(Tuning::DEFAULT.undo_ms).unwrap_or(u32::MAX),
                    });
                    self.report(Report::Learned {
                        peer,
                        name,
                        side,
                        undo_secs,
                    });
                }
            }
        }
    }

    fn secure_attention(&self) {
        let log = self.log.clone();
        let sas = self.sas;
        tokio::task::spawn_blocking(move || match sas {
            Sas::Console => {
                warn!(
                    log,
                    "secure attention requested; it needs the registered service"
                )
            }
            Sas::Service => match win32_session::sas_policy() {
                Ok(policy) if win32_session::allows_services(policy) => {
                    info!(log, "secure attention requested");
                    win32_session::send_sas();
                }
                Ok(policy) => warn!(
                    log,
                    "secure attention requested, but policy {}\\{} is {} (1 or 3 allows services)",
                    win32_session::SAS_POLICY_KEY,
                    win32_session::SAS_POLICY_VALUE,
                    policy.map_or_else(|| "not set".to_owned(), |v| v.to_string())
                ),
                Err(e) => warn!(
                    log,
                    "secure attention requested; cannot read its policy: {e}"
                ),
            },
        });
    }

    // ---- node ----

    fn on_node(&mut self, event: NodeEvent) {
        if !matches!(
            event,
            NodeEvent::Message { .. }
                | NodeEvent::Addresses(_)
                | NodeEvent::ClipboardOffer { .. }
                | NodeEvent::ClipboardRequest { .. }
                | NodeEvent::ClipboardText { .. }
        ) {
            self.dirty = true;
        }
        match event {
            NodeEvent::PairingClosed(why) => {
                info!(self.log, "pairing closed: {why:?}");
                self.pairing = None;
                self.broadcast(&UiEvent::PairingClosed(match why {
                    ModeClosed::Paired => PairingClosedReason::Joined,
                    ModeClosed::Expired => PairingClosedReason::Expired,
                    ModeClosed::ByUser => PairingClosedReason::ByUser,
                    ModeClosed::TooManyFailures => PairingClosedReason::TooManyFailures,
                }));
                self.report(Report::PairingClosed(why));
            }
            NodeEvent::Paired { peer, name } => {
                info!(
                    self.log,
                    "paired with {} ({})",
                    name.as_str(),
                    self.short(peer)
                );
                self.broadcast(&UiEvent::Paired {
                    key: peer.0,
                    name: name.as_str().to_owned(),
                });
                self.report(Report::Paired {
                    peer,
                    name: name.as_str().to_owned(),
                });
            }
            NodeEvent::JoinFailed(why) => {
                warn!(self.log, "join failed: {why:?}");
                self.broadcast(&UiEvent::JoinFailed(match why {
                    JoinFailure::WrongCode => JoinFailedReason::WrongCode,
                    JoinFailure::NotReady => JoinFailedReason::NotReady,
                    JoinFailure::ProtocolError => JoinFailedReason::ProtocolError,
                    JoinFailure::DeskFull => JoinFailedReason::DeskFull,
                    JoinFailure::ConnectionLost => JoinFailedReason::ConnectionLost,
                    JoinFailure::Unreachable => JoinFailedReason::Unreachable,
                }));
                self.report(Report::JoinFailed(why));
            }
            NodeEvent::PeerUp(peer) => {
                self.up.insert(peer);
                let name = self.name(peer);
                info!(self.log, "{name} is up");
                self.report(Report::PeerUp { peer, name });
                self.engine(EngineEvent::PeerConnected(peer));
            }
            NodeEvent::PeerDown(peer, why) => {
                self.up.remove(&peer);
                let name = self.name(peer);
                info!(self.log, "{name} is down: {why:?}");
                self.report(Report::PeerDown { peer, name, why });
                self.clipboard.peer_down(peer);
                self.engine(EngineEvent::PeerLost(peer));
            }
            NodeEvent::TrustStore(bytes) => {
                match Desk::from_file(self.me, &bytes) {
                    Ok(desk) => self.desk = desk,
                    Err(e) => warn!(self.log, "the node reported an unreadable trust store: {e}"),
                }
                self.store.write(store::TRUST_STORE, bytes);
            }
            NodeEvent::Addresses(addresses) => {
                self.store
                    .write(store::ADDRESSES, store::addresses_to_json(&addresses));
            }
            NodeEvent::Message { peer, msg } => self.engine(EngineEvent::FromPeer { peer, msg }),
            event @ (NodeEvent::ClipboardOffer { .. }
            | NodeEvent::ClipboardRequest { .. }
            | NodeEvent::ClipboardText { .. }) => self.on_clipboard(event),
        }
    }

    // ---- commands ----

    async fn on_command(&mut self, command: HostCommand) {
        match command {
            HostCommand::OpenPairing(reply) => {
                let opened = self.open_pairing().await;
                let _ = reply.send(opened);
            }
            HostCommand::ClosePairing => self.node.close_pairing(),
            HostCommand::Join(code) => self.join(code),
            HostCommand::Peers(reply) => {
                let rows = self
                    .desk
                    .members()
                    .map(|(peer, name)| PeerRow {
                        peer,
                        name: name.as_str().to_owned(),
                        up: self.up.contains(&peer),
                    })
                    .collect();
                let _ = reply.send(rows);
            }
            HostCommand::Remove(peer) => self.remove(peer),
            HostCommand::Undo => self.undo(),
            HostCommand::ForgetLayout => self.engine(EngineEvent::ForgetLayout),
            HostCommand::Suspend(done) => {
                info!(self.log, "going to sleep: saying goodbye");
                let _ = tokio::time::timeout(GOODBYE_WAIT, self.node.goodbye()).await;
                let _ = done.send(());
            }
            HostCommand::Resume => {
                info!(self.log, "resumed: reconnecting");
                self.node.resume();
            }
            HostCommand::DropLink => self.link_ended("its session ended"),
            // Handled in `run`, which ends the loop.
            HostCommand::Stop(_) => {}
        }
    }

    async fn stop(&mut self) {
        info!(self.log, "stopping");
        let _ = tokio::time::timeout(GOODBYE_WAIT, self.node.goodbye()).await;
        self.link_ended("the service is stopping");
        self.store.flush().await;
        self.node.shutdown();
    }
}

#[cfg(test)]
mod tests;
