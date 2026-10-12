//! The real service in console mode, launching the real agent (specs:
//! service-host, session-agents). The agent installs hooks, so these need a
//! logged-on desktop and never run by default. Build the agent first, so it
//! sits next to the service:
//!
//! ```text
//! cargo build -p mouseflock-agent
//! cargo test -p mouseflock-service -- --ignored --test-threads=1
//! ```
//!
//! The agent only ever passes input through here: nothing asks it to
//! withhold. Every test still kills the service and every agent it launched
//! when it ends, however it ends.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

const SERVICE: &str = env!("CARGO_BIN_EXE_mouseflock-service");
const WAIT: Duration = Duration::from_secs(10);

fn agent_exe() -> PathBuf {
    let path = PathBuf::from(SERVICE).with_file_name("mouseflock-agent.exe");
    assert!(
        path.exists(),
        "{} is missing; run `cargo build -p mouseflock-agent` first",
        path.display()
    );
    path
}

fn is_running(pid: u32) -> bool {
    let out = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
}

fn kill(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// A console-mode service, killed with every agent it launched on drop.
struct Console {
    child: Child,
    stdin: Option<ChildStdin>,
    out: Receiver<String>,
    err: Receiver<String>,
    agents: Vec<u32>,
    dir: PathBuf,
}

fn lines(r: impl std::io::Read + Send + 'static) -> Receiver<String> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(r).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

impl Console {
    fn start(tag: &str) -> Self {
        agent_exe();
        let dir =
            std::env::temp_dir().join(format!("mouseflock-console-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut child = Command::new(SERVICE)
            .args(["--console", "--port", "0", "--data"])
            .arg(&dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let out = lines(child.stdout.take().unwrap());
        let err = lines(child.stderr.take().unwrap());
        Self {
            child,
            stdin,
            out,
            err,
            agents: Vec::new(),
            dir,
        }
    }

    fn type_line(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
    }

    /// Waits for a stdout line containing `text`.
    fn printed(&mut self, text: &str, within: Duration) {
        let end = Instant::now() + within;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            match self.out.recv_timeout(left) {
                Ok(line) if line.contains(text) => return,
                Ok(_) => {}
                Err(_) => panic!("`{text}` not printed within {within:?}"),
            }
        }
    }

    /// Waits for a log line containing `text`, and returns it.
    fn logged(&mut self, text: &str, within: Duration) -> String {
        let end = Instant::now() + within;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            match self.err.recv_timeout(left) {
                Ok(line) => {
                    if let Some(pid) = launched_pid(&line) {
                        self.agents.push(pid);
                    }
                    if line.contains(text) {
                        return line;
                    }
                }
                Err(_) => panic!("`{text}` not logged within {within:?}"),
            }
        }
    }

    /// Waits for the next agent to be launched and linked; returns its PID.
    fn linked_agent(&mut self) -> u32 {
        let line = self.logged("launched session agent", WAIT);
        self.printed("session agent linked", WAIT);
        launched_pid(&line).unwrap()
    }

    fn exits_within(&mut self, within: Duration) -> Option<ExitStatus> {
        let end = Instant::now() + within;
        while Instant::now() < end {
            if let Some(status) = self.child.try_wait().unwrap() {
                return Some(status);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }
}

/// The PID in a "launched session agent <pid> in …" log line.
fn launched_pid(line: &str) -> Option<u32> {
    let rest = line.split("launched session agent ").nth(1)?;
    rest.split_whitespace().next()?.parse().ok()
}

impl Drop for Console {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        for pid in &self.agents {
            kill(*pid);
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
#[ignore = "needs a desktop"]
fn console_links_its_agent() {
    let mut console = Console::start("links");
    let pid = console.linked_agent();
    assert!(is_running(pid));
    console.type_line("quit");
    assert!(console.exits_within(WAIT).is_some());
}

#[test]
#[ignore = "needs a desktop"]
fn quit_ends_service_and_agent() {
    let mut console = Console::start("quit");
    let pid = console.linked_agent();
    console.type_line("quit");
    console.logged(&format!("session agent {pid} exited with Some(0)"), WAIT);
    let status = console.exits_within(WAIT).expect("service kept running");
    assert_eq!(status.code(), Some(0));
    assert!(!is_running(pid));
}

#[test]
#[ignore = "needs a desktop"]
fn killed_agent_is_replaced() {
    let mut console = Console::start("replace");
    let first = console.linked_agent();
    let killed = Instant::now();
    kill(first);
    let second = console.linked_agent();
    assert_ne!(first, second);
    assert!(
        killed.elapsed() < Duration::from_secs(2),
        "relinked after {:?}",
        killed.elapsed()
    );
    console.type_line("quit");
    assert!(console.exits_within(WAIT).is_some());
}

#[test]
#[ignore = "needs a desktop"]
fn ui_client_sees_status_and_opens_pairing() {
    use ipc::link::UiEnd;
    use ipc::ui::{Pairing, UiCommand, UiEvent};
    use tokio::net::windows::named_pipe::ClientOptions;

    let mut console = Console::start("ui");
    console.logged(r"serving UIs on \\.\pipe\mouseflock-ui", WAIT);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let pipe = ClientOptions::new()
            .open(r"\\.\pipe\mouseflock-ui")
            .unwrap();
        let (to_service, mut events) = UiEnd::connect(pipe).await.unwrap();
        let first = tokio::time::timeout(WAIT, events.recv()).await.unwrap();
        assert!(matches!(first, Some(UiEvent::Status(_))), "{first:?}");
        to_service.send(UiCommand::OpenPairing).unwrap();
        let open = tokio::time::timeout(WAIT, async {
            while let Some(e) = events.recv().await {
                if let UiEvent::Status(s) = e
                    && let Pairing::Open { code, .. } = s.pairing
                {
                    return code;
                }
            }
            panic!("link closed");
        })
        .await
        .unwrap();
        assert!(!open.is_empty());
    });
    console.type_line("quit");
    assert!(console.exits_within(WAIT).is_some());
}
