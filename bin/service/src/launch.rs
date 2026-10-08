//! The one launch function (design D6). Every process the service starts is
//! started here: the session agent, as the session's user (registered
//! service) or as the console's own user (console mode), and the winlogon
//! agent, as SYSTEM with privileges removed on the secure desktop
//! (registered service only; add-winlogon-agent design D2, D3).

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use ipc::link::AgentRole;
use rand_core::{OsRng, RngCore};
use tokio::net::windows::named_pipe::NamedPipeServer;
use tokio::sync::oneshot;
use win32_session::{Desktop, Process, SessionId, Sid, TokenError};

use crate::store::hex;

/// Where an agent runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A logged-on session, as its user (needs SYSTEM).
    Session(SessionId),
    /// Console mode: this process's own user and session.
    Console,
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Session(s) => write!(f, "session {s}"),
            Self::Console => f.write_str("this console's session"),
        }
    }
}

#[derive(Debug)]
pub enum LaunchError {
    NoUser,
    Failed(String),
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoUser => f.write_str("no user is logged on"),
            Self::Failed(e) => f.write_str(e),
        }
    }
}

impl From<TokenError> for LaunchError {
    fn from(e: TokenError) -> Self {
        match e {
            TokenError::NoUser => Self::NoUser,
            e => Self::Failed(e.to_string()),
        }
    }
}

fn failed(what: &str) -> impl Fn(std::io::Error) -> LaunchError + '_ {
    move |e| LaunchError::Failed(format!("{what}: {e}"))
}

/// A fresh agent pipe name carrying 128 random bits.
pub fn pipe_name() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    format!(r"\\.\pipe\deskhop-agent-{}", hex(&bytes))
}

/// `deskhop-agent.exe` in this executable's own folder.
pub fn agent_path() -> std::io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .ok_or_else(|| std::io::Error::other("the executable has no folder"))?;
    Ok(dir.join("deskhop-agent.exe"))
}

/// A launched agent: its process ID, a way to end it, and its exit.
pub struct Watch {
    pub id: u32,
    /// The privileges the winlogon agent's token holds, for the log.
    pub privileges: Option<Vec<String>>,
    /// Whether the session agent was launched with UI access; `None` for the
    /// winlogon agent, which never has it.
    pub ui_access: Option<bool>,
    /// Why UI access was not given, to log once (add-elevated-windows D4).
    pub ui_access_note: Option<String>,
    kill: Kill,
    /// Resolves with the exit code when the process exits.
    pub exited: oneshot::Receiver<Option<u32>>,
}

enum Kill {
    Session(Arc<Process>),
    Console(Option<oneshot::Sender<()>>),
}

impl Watch {
    /// Ends the process; its exit then arrives through `exited`.
    pub fn terminate(&mut self) {
        match &mut self.kill {
            Kill::Session(p) => {
                let _ = p.terminate();
            }
            Kill::Console(kill) => {
                if let Some(kill) = kill.take() {
                    let _ = kill.send(());
                }
            }
        }
    }
}

/// The agent kind's name, for the log.
pub fn name(role: AgentRole) -> &'static str {
    match role {
        AgentRole::Session => "session agent",
        AgentRole::Winlogon => "winlogon agent",
    }
}

/// The agent's command line after the executable.
pub fn args(role: AgentRole, pipe: &str) -> [&str; 3] {
    let kind = match role {
        AgentRole::Session => "--session",
        AgentRole::Winlogon => "--winlogon",
    };
    [kind, "--pipe", pipe]
}

/// The desktop an agent of `role` runs on.
pub fn desktop(role: AgentRole) -> Desktop {
    match role {
        AgentRole::Session => Desktop::Default,
        AgentRole::Winlogon => Desktop::Winlogon,
    }
}

/// Starts an agent of `role` for `target`: creates its pipe for the user it
/// will run as (a fresh name, first and only instance), then launches
/// `deskhop-agent.exe --session|--winlogon --pipe <name>` on its desktop.
/// Returns the watch and the pipe, not yet connected.
///
/// The session agent runs as the session's user, or in console mode as
/// this process's user. The winlogon agent runs as SYSTEM with privileges
/// removed, behind a pipe only SYSTEM can open; it needs a session target.
pub fn agent(role: AgentRole, target: Target) -> Result<(Watch, NamedPipeServer), LaunchError> {
    let exe = agent_path().map_err(failed("cannot find the agent"))?;
    let dir = exe.parent().map(PathBuf::from).unwrap_or_default();
    // The session agent in a session is launched with UI access and shielded
    // (add-elevated-windows D2, D4); if Windows refuses, it falls back to a
    // plain launch, works with every window but elevated ones, and says why
    // once. `ui_access` is `None` for the winlogon agent, which never has it.
    let mut ui_access: Option<bool> = None;
    let mut ui_access_note: Option<String> = None;
    let mut shield = false;
    let token = match (role, target) {
        (AgentRole::Session, Target::Session(session)) => {
            match win32_session::ui_access_token(session) {
                Ok(token) => {
                    ui_access = Some(true);
                    shield = true;
                    Some(token)
                }
                Err(win32_session::UiAccessError::NoUser) => return Err(LaunchError::NoUser),
                Err(e) => {
                    ui_access = Some(false);
                    ui_access_note = Some(e.to_string());
                    Some(win32_session::user_token(session)?)
                }
            }
        }
        (AgentRole::Session, Target::Console) => {
            ui_access = Some(false);
            None
        }
        (AgentRole::Winlogon, Target::Session(session)) => Some(
            win32_session::system_token(session)
                .map_err(failed("cannot make the winlogon agent's token"))?,
        ),
        (AgentRole::Winlogon, Target::Console) => {
            return Err(LaunchError::Failed(
                "the winlogon agent needs the registered service".into(),
            ));
        }
    };
    let user: Sid = match &token {
        Some(t) => t.user_sid(),
        None => win32_session::own_sid(),
    }
    .map_err(failed("cannot read the user's SID"))?;

    let mut created = None;
    for _ in 0..3 {
        let name = pipe_name();
        if let Ok(pipe) = win32_session::create_agent_pipe(&name, &user) {
            created = Some((name, pipe));
            break;
        }
    }
    let (name, pipe) =
        created.ok_or_else(|| LaunchError::Failed("cannot create the agent pipe".into()))?;
    let args = args(role, &name);

    let watch = match token {
        Some(token) => {
            let privileges =
                (role == AgentRole::Winlogon).then(|| token.privileges().unwrap_or_default());
            let shield_sid = shield.then_some(&user);
            let launched =
                win32_session::launch_as(&token, &exe, &args, &dir, desktop(role), shield_sid);
            // A shielded launch that Windows rejects falls back to a plain one,
            // so the agent still runs, without elevated windows.
            let process = match (launched, shield) {
                (Ok(p), _) => p,
                (Err(e), true) => {
                    ui_access = Some(false);
                    ui_access_note = Some(format!("shielded launch failed: {e}"));
                    win32_session::launch_as(&token, &exe, &args, &dir, desktop(role), None)
                        .map_err(failed("cannot launch the agent"))?
                }
                (Err(e), false) => return Err(failed("cannot launch the agent")(e)),
            };
            let process = Arc::new(process);
            let (tx, exited) = oneshot::channel();
            let waiting = process.clone();
            tokio::task::spawn_blocking(move || {
                let _ = tx.send(waiting.wait().ok());
            });
            Watch {
                id: process.id(),
                privileges,
                ui_access,
                ui_access_note,
                kill: Kill::Session(process),
                exited,
            }
        }
        None => {
            let mut child = tokio::process::Command::new(&exe)
                .args(args)
                .current_dir(&dir)
                .kill_on_drop(true)
                .spawn()
                .map_err(failed("cannot launch the agent"))?;
            let id = child.id().unwrap_or(0);
            let (tx, exited) = oneshot::channel();
            let (kill, kill_rx) = oneshot::channel::<()>();
            tokio::spawn(async move {
                let status = tokio::select! {
                    status = child.wait() => status,
                    _ = kill_rx => {
                        let _ = child.start_kill();
                        child.wait().await
                    }
                };
                let code = status.ok().and_then(|s| s.code()).map(|c| c as u32);
                let _ = tx.send(code);
            });
            Watch {
                id,
                privileges: None,
                ui_access,
                ui_access_note: None,
                kill: Kill::Console(Some(kill)),
                exited,
            }
        }
    };
    Ok((watch, pipe))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::Path;

    #[test]
    fn arguments_and_desktop_per_kind() {
        assert_eq!(
            args(AgentRole::Session, r"\\.\pipe\a"),
            ["--session", "--pipe", r"\\.\pipe\a"]
        );
        assert_eq!(
            args(AgentRole::Winlogon, r"\\.\pipe\b"),
            ["--winlogon", "--pipe", r"\\.\pipe\b"]
        );
        assert_eq!(desktop(AgentRole::Session), Desktop::Default);
        assert_eq!(desktop(AgentRole::Winlogon), Desktop::Winlogon);
    }

    #[test]
    fn no_winlogon_agent_in_console_mode() {
        let err = agent(AgentRole::Winlogon, Target::Console).err();
        assert!(
            matches!(&err, Some(LaunchError::Failed(e)) if e.contains("registered service")),
            "{err:?}"
        );
    }

    #[test]
    fn pipe_names_are_unique() {
        let names: BTreeSet<String> = (0..1000).map(|_| pipe_name()).collect();
        assert_eq!(names.len(), 1000);
        for n in &names {
            let hexpart = n.strip_prefix(r"\\.\pipe\deskhop-agent-").unwrap();
            assert_eq!(hexpart.len(), 32);
            assert!(hexpart.bytes().all(|b| b.is_ascii_hexdigit()));
        }
    }

    fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    #[test]
    fn processes_start_only_here() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        sources(&src, &mut files);
        assert!(files.len() > 3);
        for file in files {
            if file.ends_with("launch.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&file).unwrap();
            for call in [
                "Command::new(",
                "launch_as(",
                "CreateProcess",
                "std::process::Command",
            ] {
                assert!(!text.contains(call), "{} calls {call}", file.display());
            }
        }
    }
}
