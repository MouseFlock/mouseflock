//! The engine loop (design D3): one task owns the engine and connects it to
//! the session agent's link and the transport node.

use std::collections::BTreeSet;
use std::time::Duration;

use engine::{CaptureMode, Decision, Engine, Event as EngineEvent, FocusView, Tuning};
use ipc::link::{AgentHandle, FromAgent};
use ipc::message::ToAgent;
use model::{LayoutBook, Millis, PeerId, Side};
use pairing::{Code, Desk, JoinFailure, ModeClosed};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use transport::{Down, Event as NodeEvent, NoNetwork, NodeHandle, StartError};

use crate::log::Log;
use crate::store::{self, StoreHandle};
use crate::{info, warn};

/// How long goodbye may take before sleep or stop.
pub const GOODBYE_WAIT: Duration = Duration::from_secs(1);

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
    commands: mpsc::UnboundedReceiver<HostCommand>,
) -> Result<Running, StartError> {
    let me = config.node.machine.peer_id();
    let desk = config.node.machine.desk().clone();
    let (node, node_events) = transport::start(config.node)?;
    let mut engine = Engine::new(me, engine::Config::default());
    engine.handle(EngineEvent::LayoutsLoaded(config.layouts));
    let host = Host {
        engine,
        node: node.clone(),
        me,
        desk,
        up: BTreeSet::new(),
        link: None,
        store: config.store,
        log: config.log,
        sas: config.sas,
        clock: config.clock,
        reports: config.reports,
    };
    let task = tokio::spawn(host.run(node_events, links, commands));
    Ok(Running { node, task })
}

struct Host {
    engine: Engine,
    node: NodeHandle,
    me: PeerId,
    /// A copy of the membership view, for names.
    desk: Desk,
    up: BTreeSet<PeerId>,
    link: Option<AgentLink>,
    store: StoreHandle,
    log: Log,
    sas: Sas,
    clock: Clock,
    reports: Option<mpsc::UnboundedSender<Report>>,
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
        mut commands: mpsc::UnboundedReceiver<HostCommand>,
    ) {
        info!(self.log, "running as {}", self.short(self.me));
        loop {
            tokio::select! {
                Some(event) = node_events.recv() => self.on_node(event),
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

    // ---- agent ----

    fn new_link(&mut self, link: AgentLink) {
        if self.link.is_some() {
            self.link_ended("replaced by a new agent link");
        }
        let mode = mode_for(self.engine.focus());
        let _ = link.handle.send(ToAgent::SetCapture(mode));
        self.link = Some(link);
        info!(self.log, "agent linked");
        self.report(Report::AgentLinked);
    }

    fn link_ended(&mut self, why: &str) {
        if self.link.take().is_none() {
            return;
        }
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
                }
                Decision::LayoutsChanged(book) => {
                    self.store
                        .write(store::LAYOUTS, store::layouts_to_json(&book));
                }
                Decision::OfferUndo { peer, side } => {
                    let name = self.name(peer);
                    let undo_secs = Tuning::DEFAULT.undo_ms / 1000;
                    info!(self.log, "learned {name} to the {}", store::side_name(side));
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
        match event {
            NodeEvent::PairingClosed(why) => {
                info!(self.log, "pairing closed: {why:?}");
                self.report(Report::PairingClosed(why));
            }
            NodeEvent::Paired { peer, name } => {
                info!(
                    self.log,
                    "paired with {} ({})",
                    name.as_str(),
                    self.short(peer)
                );
                self.report(Report::Paired {
                    peer,
                    name: name.as_str().to_owned(),
                });
            }
            NodeEvent::JoinFailed(why) => {
                warn!(self.log, "join failed: {why:?}");
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
        }
    }

    // ---- commands ----

    async fn on_command(&mut self, command: HostCommand) {
        match command {
            HostCommand::OpenPairing(reply) => {
                let opened = self.node.open_pairing().await;
                match &opened {
                    Ok(_) => info!(self.log, "pairing opened"),
                    Err(e) => warn!(self.log, "pairing not opened: {e}"),
                }
                let _ = reply.send(opened);
            }
            HostCommand::ClosePairing => self.node.close_pairing(),
            HostCommand::Join(code) => {
                info!(self.log, "joining by code");
                self.node.join(code);
            }
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
            HostCommand::Remove(peer) => {
                info!(self.log, "removing {}", self.name(peer));
                self.node.remove(peer);
            }
            HostCommand::Undo => {
                let at = self.clock.now();
                self.engine(EngineEvent::Undo { at });
            }
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
