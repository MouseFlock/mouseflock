//! `--session --pipe <name>`: the session agent's link to the service
//! (agent-link spec, design D5).
//!
//! Capture starts only after the `Hello` exchange, and the link ending ends
//! capture: the `Capture` is dropped, which returns to pass-all and removes
//! the hooks, before the process exits.

use std::process::ExitCode;
use std::time::Duration;

use ipc::link::AgentEnd;
use ipc::message::{ToAgent, ToService};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
use tokio::sync::mpsc;
use tokio::time::Instant;
use win32_input::{Capture, Captured};

/// `ERROR_PIPE_BUSY`: every instance of the pipe is in use.
const ERROR_PIPE_BUSY: i32 = 231;

/// How long to retry a busy pipe.
const BUSY_RETRY: Duration = Duration::from_secs(1);

/// Exit codes.
const NO_PIPE: u8 = 3;
const NO_HANDSHAKE: u8 = 4;
const NO_CAPTURE: u8 = 5;

pub fn run(pipe: &str) -> ExitCode {
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
    runtime.block_on(session(pipe))
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

async fn session(pipe: &str) -> ExitCode {
    let stream = match connect(pipe).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("deskhop-agent: cannot open {pipe}: {e}");
            return ExitCode::from(NO_PIPE);
        }
    };
    let (reports, mut commands) = match AgentEnd::connect(stream, win32_input::now).await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("deskhop-agent: {e}");
            return ExitCode::from(NO_HANDSHAKE);
        }
    };
    let (capture, captured) = match win32_input::start() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("deskhop-agent: {e}");
            return ExitCode::from(NO_CAPTURE);
        }
    };

    // Capture reports on a std channel; a thread moves them onto the link's
    // runtime. It ends when the capture is dropped.
    let (events_tx, mut events) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        while let Ok(c) = captured.recv() {
            if events_tx.send(c).is_err() {
                break;
            }
        }
    });

    loop {
        tokio::select! {
            cmd = commands.recv() => match cmd {
                Some(cmd) => apply(&capture, cmd),
                None => break,
            },
            event = events.recv() => {
                let Some(event) = event else { break };
                if reports.send(report(event)).is_err() {
                    break;
                }
            }
        }
    }
    // Pass all and unhook before anything else.
    drop(capture);
    ExitCode::SUCCESS
}

fn report(c: Captured) -> ToService {
    match c {
        Captured::Input { at, input, origin } => ToService::Input { at, input, origin },
        Captured::Screen(screen, _) => ToService::Screen(screen),
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
