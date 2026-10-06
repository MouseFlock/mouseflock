//! `deskhop-service.exe`: Windows service, SYSTEM, session 0.
//!
//! Owns transport, pairing keys, and the engine. Launches the session agent
//! into the console session; every launch goes through [`launch::agent`].
//! Never touches input: the agent captures and injects, over the `ipc` link.
//!
//! - With no arguments it runs under the Service Control Manager ([`scm`]).
//! - `--console [--data <dir>] [--port <n>]` runs it in a terminal as the
//!   current user, with its own data folder and typed commands ([`console`]).

#![forbid(unsafe_code)]

mod console;
mod host;
mod launch;
mod log;
mod scm;
mod store;
mod supervisor;
mod ui;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use host::{Clock, HostCommand, HostConfig, Report, Sas};
use log::Log;
use supervisor::SupervisorCommand;

const USAGE: &str = "usage: deskhop-service [--console [--data <dir>] [--port <n>]]";

#[derive(Debug, PartialEq, Eq)]
enum Mode {
    Service,
    Console { data: Option<PathBuf>, port: u16 },
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Mode, String> {
    let mut args = args.into_iter();
    let Some(first) = args.next() else {
        return Ok(Mode::Service);
    };
    if first != "--console" {
        return Err(format!("unknown argument `{first}`"));
    }
    let (mut data, mut port) = (None, transport::PORT);
    while let Some(arg) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("`{arg}` needs a value"))?;
        match arg.as_str() {
            "--data" if data.is_none() => data = Some(PathBuf::from(value)),
            "--port" => port = value.parse().map_err(|_| format!("bad port `{value}`"))?,
            _ => return Err(format!("unknown argument `{arg}`")),
        }
    }
    Ok(Mode::Console { data, port })
}

/// A started service: the host and the supervisor.
pub struct Service {
    pub host: mpsc::UnboundedSender<HostCommand>,
    pub supervisor: mpsc::UnboundedSender<SupervisorCommand>,
    host_task: tokio::task::JoinHandle<()>,
    supervisor_task: tokio::task::JoinHandle<()>,
    ui_task: tokio::task::JoinHandle<()>,
}

/// Prepares the data folder, loads what is stored, and starts the host,
/// the supervisor and the UI pipe server.
pub async fn start(
    console: bool,
    dir: PathBuf,
    port: u16,
    log: Log,
    reports: Option<mpsc::UnboundedSender<Report>>,
    sas: Sas,
) -> Result<Service, String> {
    store::prepare(&dir, !console, &log)
        .map_err(|e| format!("cannot prepare {}: {e}", dir.display()))?;
    let name = store::machine_name(std::env::var("COMPUTERNAME").ok());
    let stored = store::load(&dir, name, &log).map_err(|e| format!("cannot load state: {e}"))?;
    let mut node = transport::Config::new(stored.machine);
    node.port = port;
    node.addresses = stored.addresses;
    let clock = Clock::new();
    let (links_tx, links) = mpsc::unbounded_channel();
    let (host, host_rx) = mpsc::unbounded_channel();
    let (supervisor, supervisor_rx) = mpsc::unbounded_channel();
    let (uis_tx, uis) = mpsc::unbounded_channel();
    let (admit, place) = if console {
        let me =
            win32_session::own_sid().map_err(|e| format!("cannot read this user's SID: {e}"))?;
        let session =
            win32_session::own_session().map_err(|e| format!("cannot read this session: {e}"))?;
        (
            win32_session::UiAdmit::User(me),
            ui::Place::Session(session),
        )
    } else {
        (win32_session::UiAdmit::InteractiveUsers, ui::Place::Console)
    };
    let running = host::start(
        HostConfig {
            node,
            layouts: stored.layouts,
            store: store::start(dir, log.clone()),
            log: log.clone(),
            sas,
            clock,
            reports,
        },
        links,
        uis,
        host_rx,
    )
    .map_err(|e| e.to_string())?;
    crate::info!(log, "listening on {}", running.node.local_addr());
    let ui_task = tokio::spawn(ui::serve(
        ipc::ui::PIPE_NAME.to_owned(),
        admit,
        place,
        uis_tx,
        log.clone(),
        ui::RETRY,
    ));
    let supervisor_task = tokio::spawn(supervisor::run(
        console,
        clock,
        host.clone(),
        links_tx,
        supervisor_rx,
        log,
    ));
    Ok(Service {
        host,
        supervisor,
        host_task: running.task,
        supervisor_task,
        ui_task,
    })
}

/// Goodbye to peers, end the agent link, finish writes, then make sure the
/// agent has exited.
pub async fn stop(service: Service) {
    let (done, stopped) = oneshot::channel();
    if service.host.send(HostCommand::Stop(done)).is_ok() {
        let _ = tokio::time::timeout(Duration::from_secs(5), stopped).await;
    }
    let (done, stopped) = oneshot::channel();
    if service
        .supervisor
        .send(SupervisorCommand::Stop(done))
        .is_ok()
    {
        let _ = tokio::time::timeout(Duration::from_secs(5), stopped).await;
    }
    let _ = service.host_task.await;
    service.supervisor_task.abort();
    service.ui_task.abort();
}

fn console_main(data: Option<PathBuf>, port: u16) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("deskhop-service: {e}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(async {
        let Some(dir) = data.or_else(store::console_dir) else {
            eprintln!("deskhop-service: LOCALAPPDATA is not set; give a folder with --data");
            return ExitCode::FAILURE;
        };
        let log = Log::stderr();
        let (reports_tx, reports) = mpsc::unbounded_channel();
        let service =
            match start(true, dir.clone(), port, log, Some(reports_tx), Sas::Console).await {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("deskhop-service: {e}");
                    return ExitCode::FAILURE;
                }
            };
        println!(
            "DeskHop console, data in {}. Type `help` for commands.",
            dir.display()
        );
        tokio::select! {
            () = console::run(service.host.clone(), reports) => {}
            _ = tokio::signal::ctrl_c() => {}
        }
        println!("stopping…");
        stop(service).await;
        ExitCode::SUCCESS
    })
}

fn main() -> ExitCode {
    match parse_args(std::env::args().skip(1)) {
        Ok(Mode::Service) => scm::run(),
        Ok(Mode::Console { data, port }) => console_main(data, port),
        Err(e) => {
            eprintln!("deskhop-service: {e}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn arguments() {
        assert_eq!(parse_args(args("")), Ok(Mode::Service));
        assert_eq!(
            parse_args(args("--console")),
            Ok(Mode::Console {
                data: None,
                port: transport::PORT
            })
        );
        assert_eq!(
            parse_args(args(r"--console --data C:\x --port 5000")),
            Ok(Mode::Console {
                data: Some(PathBuf::from(r"C:\x")),
                port: 5000
            })
        );
        for bad in [
            "--service",
            "--console --port",
            "--console --port x",
            "--console --data a --data b",
            "--console --verbose 1",
        ] {
            assert!(parse_args(args(bad)).is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn port_taken_names_the_port() {
        let held = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
        let port = held.local_addr().unwrap().port();
        let dir = std::env::temp_dir().join(format!("deskhop-port-{}", std::process::id()));
        let Err(e) = start(true, dir.clone(), port, Log::memory(), None, Sas::Console).await else {
            panic!("started on a taken port");
        };
        assert!(e.contains(&port.to_string()), "{e}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
