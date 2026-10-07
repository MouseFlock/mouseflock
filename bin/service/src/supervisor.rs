//! Keeps one agent of each kind running for the console session (design
//! D5; add-winlogon-agent design D4): the session agent while a user is
//! logged on there, and the winlogon agent whenever the console has a
//! session. Each kind has its own [`run`] task.
//!
//! [`Plan`] is a pure state machine over time: which session should have an
//! agent, when to launch, when to give up on a start, when to terminate.
//! [`run`] drives it: it launches through [`crate::launch::agent`], checks
//! that the process that connected is the one launched, hands the link to
//! the host, and watches for the link ending and the process exiting.

use std::time::Duration;

use ipc::link::{AgentHandle, AgentRole, FromAgent, LinkError, ServiceEnd};
use tokio::net::windows::named_pipe::NamedPipeServer;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use transport::Backoff;
use win32_session::SessionId;

use crate::host::{AgentLink, Clock, HostCommand};
use crate::launch::{self, Target, Watch};
use crate::log::Log;
use crate::{info, warn};

/// A launched agent must link within this long.
pub const START_LIMIT: Duration = Duration::from_secs(10);
/// An agent whose link ended must exit within this long.
pub const END_LIMIT: Duration = Duration::from_secs(2);
/// How often the target is read again besides session notifications.
pub const RECHECK: Duration = Duration::from_secs(1);
/// A link up this long resets the backoff.
pub const STABLE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action<T> {
    Launch(T),
    /// Ask the host to drop the current link; the agent then exits.
    EndLink,
    Terminate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Agent<T> {
    None,
    Starting {
        target: T,
        deadline: Duration,
    },
    Linked {
        target: T,
        since: Duration,
    },
    /// Waiting for the process to exit; terminate at `deadline` if set.
    Ending {
        deadline: Option<Duration>,
    },
}

/// When and where to run the agent. Times are offsets from any fixed start.
#[derive(Debug, Clone)]
pub struct Plan<T> {
    target: Option<T>,
    agent: Agent<T>,
    backoff: Backoff,
    retry_at: Duration,
    stopping: bool,
}

impl<T: Copy + Eq> Default for Plan<T> {
    fn default() -> Self {
        Self {
            target: None,
            agent: Agent::None,
            backoff: Backoff::default(),
            retry_at: Duration::ZERO,
            stopping: false,
        }
    }
}

impl<T: Copy + Eq> Plan<T> {
    /// Where the agent should run, read again. Reading the same target again
    /// changes nothing, so it may be read as often as needed.
    pub fn set_target(&mut self, target: Option<T>, now: Duration) -> Vec<Action<T>> {
        let changed = target != self.target;
        self.target = target;
        let out = match self.agent {
            Agent::Starting { target: t, .. } if Some(t) != target => {
                self.agent = Agent::Ending { deadline: None };
                vec![Action::Terminate]
            }
            Agent::Linked { target: t, .. } if Some(t) != target => {
                self.agent = Agent::Ending {
                    deadline: Some(now + END_LIMIT),
                };
                vec![Action::EndLink]
            }
            _ => Vec::new(),
        };
        // A new session is not a failure: launch there without waiting. The
        // same session keeps its backoff.
        if changed {
            self.retry_at = self.retry_at.min(now);
        }
        if out.is_empty() {
            self.try_launch(now)
        } else {
            out
        }
    }

    /// A launch could not even start a process.
    pub fn launch_failed(&mut self, now: Duration) {
        self.agent = Agent::None;
        self.fail(now, None);
    }

    pub fn linked(&mut self, now: Duration) {
        if let Agent::Starting { target, .. } = self.agent {
            self.agent = Agent::Linked { target, since: now };
        }
    }

    /// The link ended while the process may still run: it must exit soon.
    pub fn link_ended(&mut self, now: Duration) {
        if let Agent::Linked { since, .. } = self.agent {
            self.fail(now, Some(since));
            self.agent = Agent::Ending {
                deadline: Some(now + END_LIMIT),
            };
        }
    }

    /// The agent process exited.
    pub fn exited(&mut self, now: Duration) -> Vec<Action<T>> {
        match self.agent {
            Agent::Starting { .. } => self.fail(now, None),
            Agent::Linked { since, .. } => self.fail(now, Some(since)),
            Agent::Ending { .. } | Agent::None => {}
        }
        self.agent = Agent::None;
        self.try_launch(now)
    }

    pub fn tick(&mut self, now: Duration) -> Vec<Action<T>> {
        match self.agent {
            Agent::Starting { deadline, .. } if now >= deadline => {
                self.fail(now, None);
                self.agent = Agent::Ending { deadline: None };
                vec![Action::Terminate]
            }
            Agent::Ending {
                deadline: Some(deadline),
            } if now >= deadline => {
                self.agent = Agent::Ending { deadline: None };
                vec![Action::Terminate]
            }
            Agent::None => self.try_launch(now),
            _ => Vec::new(),
        }
    }

    /// The service is stopping: end the agent and launch nothing more.
    pub fn stop(&mut self, now: Duration) -> Vec<Action<T>> {
        self.stopping = true;
        match self.agent {
            Agent::Starting { .. } => {
                self.agent = Agent::Ending { deadline: None };
                vec![Action::Terminate]
            }
            Agent::Linked { .. } => {
                self.agent = Agent::Ending {
                    deadline: Some(now + END_LIMIT),
                };
                vec![Action::EndLink]
            }
            _ => Vec::new(),
        }
    }

    /// Stopping, with no agent left.
    pub fn is_done(&self) -> bool {
        self.stopping && self.agent == Agent::None
    }

    /// When [`Plan::tick`] next has something to do.
    pub fn next_wake(&self) -> Option<Duration> {
        match self.agent {
            Agent::Starting { deadline, .. } => Some(deadline),
            Agent::Ending { deadline } => deadline,
            Agent::None if !self.stopping && self.target.is_some() => Some(self.retry_at),
            _ => None,
        }
    }

    fn fail(&mut self, now: Duration, linked_since: Option<Duration>) {
        if linked_since.is_some_and(|since| now.saturating_sub(since) >= STABLE) {
            self.backoff.reset();
        }
        self.retry_at = now + self.backoff.wait();
    }

    fn try_launch(&mut self, now: Duration) -> Vec<Action<T>> {
        match (self.agent, self.target) {
            (Agent::None, Some(target)) if !self.stopping && now >= self.retry_at => {
                self.agent = Agent::Starting {
                    target,
                    deadline: now + START_LIMIT,
                };
                vec![Action::Launch(target)]
            }
            _ => Vec::new(),
        }
    }
}

// ---- accepting the agent ----

#[derive(Debug)]
pub enum AcceptError {
    Io(std::io::Error),
    /// A process other than the launched agent connected.
    WrongProcess {
        expected: u32,
        found: u32,
    },
    Link(LinkError),
    TimedOut,
}

impl std::fmt::Display for AcceptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "pipe error: {e}"),
            Self::WrongProcess { expected, found } => write!(
                f,
                "process {found} connected instead of the launched agent {expected}; closed unread"
            ),
            Self::Link(e) => write!(f, "handshake failed: {e}"),
            Self::TimedOut => write!(f, "the agent did not link within {START_LIMIT:?}"),
        }
    }
}

/// Waits for a client on `pipe`, checks it is process `expected` before
/// reading anything, and runs the service end's handshake for `role`.
pub async fn accept_agent(
    pipe: NamedPipeServer,
    expected: u32,
    role: AgentRole,
    clock: Clock,
) -> Result<(AgentHandle, mpsc::UnboundedReceiver<FromAgent>), AcceptError> {
    let accept = async {
        pipe.connect().await.map_err(AcceptError::Io)?;
        let found = win32_session::client_process_id(&pipe).map_err(AcceptError::Io)?;
        if found != expected {
            return Err(AcceptError::WrongProcess { expected, found });
        }
        ServiceEnd::accept(pipe, role, move || clock.now())
            .await
            .map_err(AcceptError::Link)
    };
    tokio::time::timeout(START_LIMIT, accept)
        .await
        .unwrap_or(Err(AcceptError::TimedOut))
}

// ---- the driver ----

pub enum SupervisorCommand {
    /// Sessions changed: find the target again.
    SessionsChanged,
    Stop(oneshot::Sender<()>),
}

enum Internal {
    Up(
        u64,
        Result<(AgentHandle, mpsc::UnboundedReceiver<FromAgent>), AcceptError>,
    ),
}

struct Current {
    generation: u64,
    watch: Watch,
    /// Resolves when the host drops the link.
    ended: Option<oneshot::Receiver<()>>,
}

/// Where an agent of `role` should run.
///
/// - Session agent: this console in console mode; otherwise the console
///   session, if a user is logged on to it.
/// - Winlogon agent: never in console mode; otherwise the console session,
///   logged on or not.
///
/// `logged_on` is asked only when it decides the answer.
pub fn target(
    role: AgentRole,
    console: bool,
    console_session: Option<SessionId>,
    logged_on: impl FnOnce(SessionId) -> bool,
) -> Option<Target> {
    match (role, console) {
        (AgentRole::Session, true) => Some(Target::Console),
        (AgentRole::Winlogon, true) => None,
        (AgentRole::Session, false) => console_session
            .filter(|s| logged_on(*s))
            .map(Target::Session),
        (AgentRole::Winlogon, false) => console_session.map(Target::Session),
    }
}

/// [`target`], reading the console session and its user from Windows. A
/// token error is logged to `log`, if given.
pub fn find_target(role: AgentRole, console: bool, log: Option<&Log>) -> Option<Target> {
    let session = if console {
        None
    } else {
        win32_session::active_console_session()
    };
    target(
        role,
        console,
        session,
        |session| match win32_session::user_token(session) {
            Ok(_) => true,
            Err(win32_session::TokenError::NoUser) => false,
            Err(e) => {
                if let Some(log) = log {
                    warn!(log, "session {session}: {e}");
                }
                false
            }
        },
    )
}

/// What the current agent did.
enum Seen {
    Exited(Option<u32>),
    /// The host dropped its link.
    LinkEnded,
}

async fn watch(current: &mut Option<Current>) -> Seen {
    let Some(c) = current.as_mut() else {
        return std::future::pending().await;
    };
    match c.ended.as_mut() {
        Some(ended) => tokio::select! {
            code = &mut c.watch.exited => Seen::Exited(code.ok().flatten()),
            _ = ended => Seen::LinkEnded,
        },
        None => Seen::Exited((&mut c.watch.exited).await.ok().flatten()),
    }
}

async fn wake(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// Runs the supervisor for agents of `role` until stopped.
pub async fn run(
    role: AgentRole,
    console: bool,
    clock: Clock,
    host: mpsc::UnboundedSender<HostCommand>,
    links: mpsc::UnboundedSender<AgentLink>,
    mut commands: mpsc::UnboundedReceiver<SupervisorCommand>,
    log: Log,
) {
    let start = Instant::now();
    let now = || start.elapsed();
    let (internal_tx, mut internal) = mpsc::unbounded_channel();
    let mut plan = Plan::default();
    let mut current: Option<Current> = None;
    let mut generation = 0u64;
    let mut stopped: Option<oneshot::Sender<()>> = None;
    let agent = launch::name(role);
    if console && role == AgentRole::Winlogon {
        info!(
            log,
            "no {agent} in console mode: it needs the registered service"
        );
    }
    let mut actions = plan.set_target(find_target(role, console, Some(&log)), now());
    // Session notifications can arrive before Windows reports the new
    // console session, as when signing out moves the logon screen to a new
    // session. Reading the target again catches up.
    let mut recheck = tokio::time::interval_at(start + RECHECK, RECHECK);
    recheck.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        while let Some(action) = actions.pop() {
            match action {
                Action::Launch(target) => match launch::agent(role, target) {
                    Ok((watch, pipe)) => {
                        generation += 1;
                        let g = generation;
                        match &watch.privileges {
                            Some(p) => info!(
                                log,
                                "launched {agent} {} in {target}, privileges: {}",
                                watch.id,
                                p.join(", ")
                            ),
                            None => info!(log, "launched {agent} {} in {target}", watch.id),
                        }
                        let tx = internal_tx.clone();
                        let expected = watch.id;
                        tokio::spawn(async move {
                            let up = accept_agent(pipe, expected, role, clock).await;
                            let _ = tx.send(Internal::Up(g, up));
                        });
                        current = Some(Current {
                            generation: g,
                            watch,
                            ended: None,
                        });
                    }
                    Err(e) => {
                        warn!(log, "cannot launch the {agent} in {target}: {e}");
                        plan.launch_failed(now());
                    }
                },
                Action::EndLink => {
                    let _ = host.send(HostCommand::DropLink(role));
                }
                Action::Terminate => {
                    if let Some(c) = &mut current {
                        warn!(log, "terminating {agent} {}", c.watch.id);
                        c.watch.terminate();
                    }
                }
            }
        }
        if plan.is_done() {
            if let Some(done) = stopped.take() {
                let _ = done.send(());
            }
            return;
        }

        let wake_at = plan.next_wake().map(|d| start + d);
        tokio::select! {
            command = commands.recv() => match command {
                Some(SupervisorCommand::SessionsChanged) => {
                    actions = plan.set_target(find_target(role, console, Some(&log)), now());
                }
                Some(SupervisorCommand::Stop(done)) => {
                    stopped = Some(done);
                    actions = plan.stop(now());
                }
                None => actions = plan.stop(now()),
            },
            seen = watch(&mut current) => match seen {
                Seen::Exited(code) => {
                    if let Some(c) = current.take() {
                        info!(log, "{agent} {} exited with {code:?}", c.watch.id);
                    }
                    actions = plan.exited(now());
                }
                Seen::LinkEnded => {
                    if let Some(c) = &mut current {
                        c.ended = None;
                    }
                    plan.link_ended(now());
                }
            },
            Some(event) = internal.recv() => match event {
                Internal::Up(g, Ok((handle, events))) if current.as_ref().is_some_and(|c| c.generation == g) => {
                    let (ended_tx, ended_rx) = oneshot::channel();
                    if let Some(c) = &mut current {
                        c.ended = Some(ended_rx);
                    }
                    plan.linked(now());
                    let _ = links.send(AgentLink { handle, events, ended: ended_tx });
                }
                Internal::Up(g, Err(e)) if current.as_ref().is_some_and(|c| c.generation == g) => {
                    warn!(log, "{agent} did not link: {e}");
                    if let Some(c) = &mut current {
                        c.watch.terminate();
                    }
                }
                Internal::Up(..) => {}
            },
            () = wake(wake_at) => actions = plan.tick(now()),
            _ = recheck.tick() => {
                actions = plan.set_target(find_target(role, console, None), now());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type P = Plan<u32>;
    const A: u32 = 1;
    const B: u32 = 2;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn logon_launches_and_logoff_ends() {
        let mut p = P::default();
        assert_eq!(p.set_target(None, ms(0)), []);
        assert_eq!(p.next_wake(), None);
        assert_eq!(p.set_target(Some(A), ms(10)), [Action::Launch(A)]);
        p.linked(ms(50));
        assert_eq!(p.set_target(None, secs(5)), [Action::EndLink]);
        assert_eq!(p.exited(secs(6)), []);
        assert_eq!(p.next_wake(), None);
    }

    #[test]
    fn switch_ends_then_launches_in_the_new_session() {
        let mut p = P::default();
        p.set_target(Some(A), ms(0));
        p.linked(ms(10));
        assert_eq!(p.set_target(Some(B), secs(1)), [Action::EndLink]);
        // Not a failure: the new session's agent starts as soon as the old
        // one has exited.
        assert_eq!(p.exited(secs(2)), [Action::Launch(B)]);
    }

    #[test]
    fn exits_back_off() {
        let mut p = P::default();
        assert_eq!(p.set_target(Some(A), ms(0)), [Action::Launch(A)]);
        let mut now = ms(0);
        for wait in [500, 1000, 2000, 4000, 8000, 16000, 30000, 30000] {
            assert_eq!(p.exited(now), [], "relaunched without waiting");
            assert_eq!(p.next_wake(), Some(now + ms(wait)));
            assert_eq!(p.tick(now + ms(wait - 1)), []);
            now += ms(wait);
            assert_eq!(p.tick(now), [Action::Launch(A)]);
        }
    }

    #[test]
    fn reading_the_same_target_keeps_the_backoff() {
        let mut p = P::default();
        assert_eq!(p.set_target(Some(A), ms(0)), [Action::Launch(A)]);
        assert_eq!(p.exited(ms(0)), []);
        assert_eq!(p.next_wake(), Some(ms(500)));
        // Rechecks find the same session: no launch before the backoff.
        assert_eq!(p.set_target(Some(A), ms(100)), []);
        assert_eq!(p.next_wake(), Some(ms(500)));
        // A linked agent in the same session is left alone.
        assert_eq!(p.tick(ms(500)), [Action::Launch(A)]);
        p.linked(ms(600));
        assert_eq!(p.set_target(Some(A), secs(5)), []);
        // A new session still launches at once.
        p.exited(secs(6));
        assert_eq!(p.set_target(Some(B), secs(6)), [Action::Launch(B)]);
    }

    #[test]
    fn backoff_resets_after_a_stable_link() {
        let mut p = P::default();
        p.set_target(Some(A), ms(0));
        p.exited(ms(0));
        p.tick(ms(500));
        p.exited(ms(500));
        p.tick(ms(1500));
        p.linked(ms(1600));
        // Up for 30 s, then lost: the wait is back to 0.5 s.
        p.link_ended(ms(31_600));
        p.exited(ms(31_700));
        assert_eq!(p.next_wake(), Some(ms(32_100)));
    }

    #[test]
    fn start_deadline_terminates() {
        let mut p = P::default();
        p.set_target(Some(A), ms(0));
        assert_eq!(p.next_wake(), Some(START_LIMIT));
        assert_eq!(p.tick(START_LIMIT - ms(1)), []);
        assert_eq!(p.tick(START_LIMIT), [Action::Terminate]);
        assert_eq!(p.exited(START_LIMIT + ms(5)), []);
        assert_eq!(p.next_wake(), Some(START_LIMIT + ms(500)));
    }

    #[test]
    fn end_deadline_terminates() {
        let mut p = P::default();
        p.set_target(Some(A), ms(0));
        p.linked(ms(10));
        p.link_ended(secs(1));
        assert_eq!(p.next_wake(), Some(secs(1) + END_LIMIT));
        assert_eq!(p.tick(secs(1) + END_LIMIT), [Action::Terminate]);
        assert_eq!(p.next_wake(), None);
    }

    #[test]
    fn stop_ends_and_launches_nothing_more() {
        let mut p = P::default();
        p.set_target(Some(A), ms(0));
        p.linked(ms(10));
        assert_eq!(p.stop(secs(1)), [Action::EndLink]);
        assert!(!p.is_done());
        assert_eq!(p.exited(secs(2)), []);
        assert!(p.is_done());
        assert_eq!(p.tick(secs(60)), []);
        assert_eq!(p.set_target(Some(B), secs(61)), []);
    }

    const S: SessionId = SessionId(3);

    #[test]
    fn targets() {
        use AgentRole::{Session, Winlogon};
        let user = |_| true;
        let nobody = |_| false;
        let unasked = |_: SessionId| -> bool { panic!("asked whether a user is logged on") };

        // Console mode: the session agent here, never a winlogon agent.
        assert_eq!(target(Session, true, None, unasked), Some(Target::Console));
        assert_eq!(target(Winlogon, true, Some(S), unasked), None);
        // Registered service, user logged on: both in the console session.
        assert_eq!(
            target(Session, false, Some(S), user),
            Some(Target::Session(S))
        );
        assert_eq!(
            target(Winlogon, false, Some(S), unasked),
            Some(Target::Session(S))
        );
        // Logon screen: only the winlogon agent.
        assert_eq!(target(Session, false, Some(S), nobody), None);
        // No session attached to the console, briefly while it switches.
        assert_eq!(target(Session, false, None, unasked), None);
        assert_eq!(target(Winlogon, false, None, unasked), None);
    }

    #[test]
    fn no_user_no_launch() {
        let mut p = P::default();
        assert_eq!(p.set_target(None, ms(0)), []);
        assert_eq!(p.tick(secs(100)), []);
    }

    #[tokio::test]
    async fn pid_mismatch_is_closed_unread() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::windows::named_pipe::ClientOptions;

        let name = launch::pipe_name();
        let pipe =
            win32_session::create_agent_pipe(&name, &win32_session::own_sid().unwrap()).unwrap();
        let accepting = tokio::spawn(accept_agent(
            pipe,
            std::process::id() + 1,
            AgentRole::Session,
            Clock::new(),
        ));
        let mut client = loop {
            match ClientOptions::new().open(&name) {
                Ok(c) => break c,
                Err(_) => tokio::time::sleep(ms(10)).await,
            }
        };
        // What an agent would send first.
        let _ = client.write_all(&[5, 0, 0, 0, 1, 0, 1, 0, 1, 0, 1]).await;
        let Err(err) = accepting.await.unwrap() else {
            panic!("accepted the wrong process");
        };
        assert!(
            matches!(err, AcceptError::WrongProcess { found, .. } if found == std::process::id()),
            "{err}"
        );
        // The service closed the pipe without answering.
        let mut buf = [0u8; 16];
        assert!(matches!(client.read(&mut buf).await, Ok(0) | Err(_)));
    }
}
