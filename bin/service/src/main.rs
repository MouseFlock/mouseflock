//! `mouseflock-service.exe`: Windows service, SYSTEM, session 0.
//!
//! Owns transport, pairing keys, and the engine. Launches the session agent
//! and the winlogon agent into the console session; every launch goes
//! through [`launch::agent`]. Never touches input: the agents capture and
//! inject, over their `ipc` links.
//!
//! - With no arguments it runs under the Service Control Manager ([`scm`]).
//! - `--console [--data <dir>] [--port <n>]` runs it in a terminal as the
//!   current user, with its own data folder and typed commands ([`console`]).
//! - `--remove-data` removes the service's data folder and exits. The
//!   installer runs it as SYSTEM on uninstall (add-installer design D4).

#![forbid(unsafe_code)]

mod clipboard;
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

use ipc::link::AgentRole;
use tokio::sync::{mpsc, oneshot};

use host::{Clock, HostCommand, HostConfig, Report, Sas};
use log::Log;
use supervisor::SupervisorCommand;

const USAGE: &str =
    "usage: mouseflock-service [--console [--data <dir>] [--port <n>]] | --remove-data";

#[derive(Debug, PartialEq, Eq)]
enum Mode {
    Service,
    Console { data: Option<PathBuf>, port: u16 },
    RemoveData,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Mode, String> {
    let mut args = args.into_iter();
    let Some(first) = args.next() else {
        return Ok(Mode::Service);
    };
    if first == "--remove-data" {
        return match args.next() {
            None => Ok(Mode::RemoveData),
            Some(extra) => Err(format!(
                "`--remove-data` takes nothing after it, not `{extra}`"
            )),
        };
    }
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

/// A started service: the host, and a supervisor for each agent kind.
pub struct Service {
    pub host: mpsc::UnboundedSender<HostCommand>,
    supervisors: Vec<Supervisor>,
    host_task: tokio::task::JoinHandle<()>,
    ui_task: tokio::task::JoinHandle<()>,
}

struct Supervisor {
    commands: mpsc::UnboundedSender<SupervisorCommand>,
    task: tokio::task::JoinHandle<()>,
}

impl Service {
    /// Sessions changed: every supervisor finds its target again.
    pub fn sessions_changed(&self) {
        for s in &self.supervisors {
            let _ = s.commands.send(SupervisorCommand::SessionsChanged);
        }
    }
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
    let supervisors = [AgentRole::Session, AgentRole::Winlogon]
        .into_iter()
        .map(|role| {
            let (commands, commands_rx) = mpsc::unbounded_channel();
            let task = tokio::spawn(supervisor::run(
                role,
                console,
                clock,
                host.clone(),
                links_tx.clone(),
                commands_rx,
                log.clone(),
            ));
            Supervisor { commands, task }
        })
        .collect();
    Ok(Service {
        host,
        supervisors,
        host_task: running.task,
        ui_task,
    })
}

/// Goodbye to peers, end the agent links, finish writes, then make sure
/// both agents have exited.
pub async fn stop(service: Service) {
    let (done, stopped) = oneshot::channel();
    if service.host.send(HostCommand::Stop(done)).is_ok() {
        let _ = tokio::time::timeout(Duration::from_secs(5), stopped).await;
    }
    let waits = service.supervisors.iter().filter_map(|s| {
        let (done, stopped) = oneshot::channel();
        s.commands.send(SupervisorCommand::Stop(done)).ok()?;
        Some(tokio::time::timeout(Duration::from_secs(5), stopped))
    });
    let waits: Vec<_> = waits.collect();
    for wait in waits {
        let _ = wait.await;
    }
    let _ = service.host_task.await;
    for s in service.supervisors {
        s.task.abort();
    }
    service.ui_task.abort();
}

fn console_main(data: Option<PathBuf>, port: u16) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("mouseflock-service: {e}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(async {
        let Some(dir) = data.or_else(store::console_dir) else {
            eprintln!("mouseflock-service: LOCALAPPDATA is not set; give a folder with --data");
            return ExitCode::FAILURE;
        };
        let log = Log::stderr();
        let (reports_tx, reports) = mpsc::unbounded_channel();
        let service =
            match start(true, dir.clone(), port, log, Some(reports_tx), Sas::Console).await {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("mouseflock-service: {e}");
                    return ExitCode::FAILURE;
                }
            };
        println!(
            "MouseFlock console, data in {}. Type `help` for commands.",
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

/// Removes the service's data folder if it is the folder the service keeps
/// (add-installer design D4). Always exits 0, so a leftover never blocks an
/// uninstall; the installer's log keeps what was printed.
fn remove_data() -> ExitCode {
    let dir = store::service_dir();
    match win32_session::remove_secure_folder(&dir) {
        Ok(win32_session::Removal::Removed) => println!("removed {}", dir.display()),
        Ok(win32_session::Removal::Absent) => {
            println!("nothing to remove: {} does not exist", dir.display())
        }
        Ok(win32_session::Removal::Refused(why)) => {
            println!("left {} in place: {why}", dir.display())
        }
        Err(e) => println!("could not remove {}: {e}", dir.display()),
    }
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    match parse_args(std::env::args().skip(1)) {
        Ok(Mode::Service) => scm::run(),
        Ok(Mode::Console { data, port }) => console_main(data, port),
        Ok(Mode::RemoveData) => remove_data(),
        Err(e) => {
            eprintln!("mouseflock-service: {e}\n{USAGE}");
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
        assert_eq!(parse_args(args("--remove-data")), Ok(Mode::RemoveData));
        for bad in [
            "--service",
            "--remove-data --console",
            "--remove-data x",
            "--console --remove-data",
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
        let dir = std::env::temp_dir().join(format!("mouseflock-port-{}", std::process::id()));
        let Err(e) = start(true, dir.clone(), port, Log::memory(), None, Sas::Console).await else {
            panic!("started on a taken port");
        };
        assert!(e.contains(&port.to_string()), "{e}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
