//! The real session agent against a test-created named pipe and the service
//! end of the link (spec: agent-link).
//!
//! `no_pipe_exits_nonzero` runs anywhere. The rest install real hooks and
//! inject F24, so they need a logged-in desktop and never run by default:
//!
//! ```text
//! cargo test -p deskhop-agent -- --ignored --test-threads=1
//! ```

use std::process::{Child, Command, ExitStatus};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use ipc::link::{AgentHandle, FromAgent, ServiceEnd};
use ipc::message::ToAgent;
use model::{CaptureMode, InputAction, InputEvent, Key, Millis, Origin};
use tokio::net::windows::named_pipe::ServerOptions;
use tokio::sync::mpsc::UnboundedReceiver;

const AGENT: &str = env!("CARGO_BIN_EXE_deskhop-agent");
const F24: Key = Key(0x73);
const WAIT: Duration = Duration::from_secs(5);

fn pipe_name() -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        r"\\.\pipe\deskhop-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    )
}

/// A running agent, killed when the test ends however it ends, so a failing
/// test never leaves an agent holding the user's input.
struct Agent(Child);

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn agent(pipe: &str) -> Agent {
    Agent(
        Command::new(AGENT)
            .args(["--session", "--pipe", pipe])
            .spawn()
            .unwrap(),
    )
}

/// The agent's exit status, if it exits within `within`.
fn exits_within(agent: &mut Agent, within: Duration) -> Option<ExitStatus> {
    let end = Instant::now() + within;
    while Instant::now() < end {
        if let Some(status) = agent.0.try_wait().unwrap() {
            return Some(status);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

fn service_clock() -> Millis {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let start = *START.get_or_init(Instant::now);
    Millis(start.elapsed().as_millis() as u64)
}

/// Starts an agent on a fresh pipe and accepts its link.
async fn linked_agent() -> (Agent, AgentHandle, UnboundedReceiver<FromAgent>) {
    let name = pipe_name();
    let server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&name)
        .unwrap();
    let child = agent(&name);
    tokio::time::timeout(WAIT, server.connect())
        .await
        .unwrap()
        .unwrap();
    let (handle, events) = ServiceEnd::accept(server, service_clock).await.unwrap();
    (child, handle, events)
}

async fn next_matching(
    events: &mut UnboundedReceiver<FromAgent>,
    want: impl Fn(&FromAgent) -> bool,
) -> FromAgent {
    let end = tokio::time::Instant::now() + WAIT;
    loop {
        let e = tokio::time::timeout_at(end, events.recv())
            .await
            .expect("timed out")
            .expect("link closed");
        if want(&e) {
            return e;
        }
    }
}

#[test]
fn no_pipe_exits_nonzero() {
    let mut child = agent(&pipe_name());
    let status = exits_within(&mut child, WAIT).expect("still running");
    assert_eq!(status.code(), Some(3));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a desktop"]
async fn first_report_is_the_screen() {
    let (mut child, handle, mut events) = linked_agent().await;
    let first = tokio::time::timeout(WAIT, events.recv()).await.unwrap();
    assert!(
        matches!(first, Some(FromAgent::Screen(ref s)) if !s.monitors.is_empty()),
        "{first:?}"
    );
    drop(handle);
    assert!(exits_within(&mut child, WAIT).is_some());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a desktop"]
async fn inject_reaches_the_service_as_injected() {
    let (mut child, handle, mut events) = linked_agent().await;
    for down in [true, false] {
        handle
            .send(ToAgent::Inject(InputAction::Key { key: F24, down }))
            .unwrap();
        next_matching(&mut events, |e| {
            matches!(e, FromAgent::Input {
                input: InputEvent::Key { key, down: d },
                origin: Origin::Injected,
                ..
            } if *key == F24 && *d == down)
        })
        .await;
    }
    drop(handle);
    assert!(exits_within(&mut child, WAIT).is_some());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a desktop"]
async fn service_gone_releases_capture() {
    let (mut child, handle, mut events) = linked_agent().await;
    // Capture is running once the screen arrives.
    next_matching(&mut events, |e| matches!(e, FromAgent::Screen(_))).await;
    handle
        .send(ToAgent::SetCapture(CaptureMode::WithholdAll))
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    drop(handle);
    drop(events);
    let status = exits_within(&mut child, Duration::from_secs(1)).expect("agent kept running");
    assert_eq!(status.code(), Some(0));
}
