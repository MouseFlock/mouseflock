//! `--session --pipe <name>` and `--winlogon --pipe <name>`: an agent's link
//! to the service (agent-link spec; add-ipc design D5, add-winlogon-agent
//! design D7).
//!
//! Both kinds run the same loop. The session agent also watches and sets
//! the clipboard; the winlogon agent, on the secure desktop, never touches
//! the clipboard and instead reports whether its desktop is the input
//! desktop.
//!
//! Capture starts only after the `Hello` exchange, and the link ending ends
//! capture: the `Capture` is dropped, which returns to pass-all and removes
//! the hooks, before the process exits.

use std::process::ExitCode;
use std::time::Duration;

use ipc::clipboard::NotShared;
use ipc::link::{AgentEnd, AgentRole, Closed, FromService, ServiceHandle};
use ipc::message::{ToAgent, ToService};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
use tokio::sync::mpsc;
use tokio::time::Instant;
use win32_clipboard::Copied;
use win32_input::{Capture, Captured, Options};

/// `ERROR_PIPE_BUSY`: every instance of the pipe is in use.
const ERROR_PIPE_BUSY: i32 = 231;

/// How long to retry a busy pipe.
const BUSY_RETRY: Duration = Duration::from_secs(1);

/// Exit codes.
const NO_PIPE: u8 = 3;
const NO_HANDSHAKE: u8 = 4;
const NO_CAPTURE: u8 = 5;

pub fn run(pipe: &str, role: AgentRole) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("deskhop-agent: {e}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(link(pipe, role))
}

async fn connect(name: &str) -> std::io::Result<NamedPipeClient> {
    let deadline = Instant::now() + BUSY_RETRY;
    loop {
        match ClientOptions::new().open(name) {
            Ok(client) => return Ok(client),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

async fn link(pipe: &str, role: AgentRole) -> ExitCode {
    let stream = match connect(pipe).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("deskhop-agent: cannot open {pipe}: {e}");
            return ExitCode::from(NO_PIPE);
        }
    };
    let (reports, mut commands) = match AgentEnd::connect(stream, role, win32_input::now).await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("deskhop-agent: {e}");
            return ExitCode::from(NO_HANDSHAKE);
        }
    };
    let options = Options {
        report_input_desktop: role == AgentRole::Winlogon,
    };
    let (capture, captured) = match win32_input::start(options) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("deskhop-agent: {e}");
            return ExitCode::from(NO_CAPTURE);
        }
    };

    // Capture reports on a std channel; a thread moves them onto the link's
    // runtime. It ends when the capture is dropped.
    let mut events = bridge(captured);

    // The clipboard is a convenience: without it, input still works. The
    // winlogon agent never has one.
    let (clipboard, mut copies) = match role {
        AgentRole::Session => match win32_clipboard::start() {
            Ok((clipboard, copied)) => (Some(clipboard), bridge(copied)),
            Err(e) => {
                eprintln!("deskhop-agent: clipboard unavailable: {e}");
                (None, mpsc::unbounded_channel().1)
            }
        },
        AgentRole::Winlogon => (None, mpsc::unbounded_channel().1),
    };

    loop {
        tokio::select! {
            cmd = commands.recv() => match cmd {
                Some(FromService::Command(cmd)) => apply(&capture, cmd),
                Some(FromService::SetClipboard(text)) => {
                    if let Some(c) = &clipboard {
                        c.set(text);
                    }
                }
                None => break,
            },
            event = events.recv() => {
                let Some(event) = event else { break };
                if reports.send(report(event)).is_err() {
                    break;
                }
            }
            Some(copied) = copies.recv() => {
                if report_copy(&reports, copied).is_err() {
                    break;
                }
            }
        }
    }
    // Pass all and unhook before anything else.
    drop(capture);
    drop(clipboard);
    ExitCode::SUCCESS
}

/// Moves items from a worker's std channel onto the link's runtime, on a
/// thread that ends when the worker drops its sender.
fn bridge<T: Send + 'static>(from: std::sync::mpsc::Receiver<T>) -> mpsc::UnboundedReceiver<T> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        while let Ok(item) = from.recv() {
            if tx.send(item).is_err() {
                break;
            }
        }
    });
    rx
}

/// Reports a settled copy: its text as a run, or why it is not shared.
fn report_copy(reports: &ServiceHandle, copied: Copied) -> Result<(), Closed> {
    let why = match copied {
        Copied::Text(text) => return reports.send_clipboard_text(&text),
        Copied::Private => NotShared::Private,
        Copied::NoText => NotShared::NoText,
        Copied::TooLarge => NotShared::TooLarge,
    };
    reports.send(ToService::ClipboardNotShared(why))
}

fn report(c: Captured) -> ToService {
    match c {
        Captured::Input { at, input, origin } => ToService::Input { at, input, origin },
        Captured::Screen(screen, _) => ToService::Screen(screen),
        // Only with `report_input_desktop`, so only on the winlogon agent.
        Captured::InputDesktop(active) => ToService::InputDesktop(active),
    }
}

/// Carries out one command; a local failure is logged and the link stays up.
fn apply(capture: &Capture, cmd: ToAgent) {
    match cmd {
        ToAgent::SetCapture(mode) => capture.set_mode(mode),
        ToAgent::Inject(action) => {
            if let Err(e) = capture.inject(action) {
                eprintln!("deskhop-agent: inject {action:?}: {e}");
            }
        }
        ToAgent::Warp(to) => {
            if let Err(e) = capture.warp(to) {
                eprintln!("deskhop-agent: warp to {to:?}: {e}");
            }
        }
        // Answered inside the link; never yielded.
        ToAgent::Ping(_) => {}
    }
}
