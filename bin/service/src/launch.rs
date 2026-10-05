//! The one launch function (design D6). Every process the service starts is
//! started here: the session agent, as the session's user (registered
//! service) or as the console's own user (console mode).

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use rand_core::{OsRng, RngCore};
use tokio::net::windows::named_pipe::NamedPipeServer;
use tokio::sync::oneshot;
use win32_session::{Process, SessionId, Sid, TokenError};

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

/// Starts the session agent for `target`: creates its pipe for the user it
/// will run as (a fresh name, first and only instance), then launches
/// `deskhop-agent.exe --session --pipe <name>`. Returns the watch and the
/// pipe, not yet connected.
pub fn agent(target: Target) -> Result<(Watch, NamedPipeServer), LaunchError> {
    let exe = agent_path().map_err(failed("cannot find the agent"))?;
    let dir = exe.parent().map(PathBuf::from).unwrap_or_default();
    let token = match target {
        Target::Session(session) => Some(win32_session::user_token(session)?),
        Target::Console => None,
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
    let args = ["--session", "--pipe", name.as_str()];

    let watch = match token {
        Some(token) => {
            let process = Arc::new(
                win32_session::launch_as(&token, &exe, &args, &dir)
                    .map_err(failed("cannot launch the agent"))?,
            );
            let (tx, exited) = oneshot::channel();
            let waiting = process.clone();
            tokio::task::spawn_blocking(move || {
                let _ = tx.send(waiting.wait().ok());
            });
            Watch {
                id: process.id(),
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
