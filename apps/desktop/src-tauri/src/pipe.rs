//! The real connector (design D2): the UI pipe, and the session of the
//! process serving it.

use std::io;

use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};

/// Opens the UI pipe. A missing pipe or one with every instance busy is an
/// error, and the link tries again later.
pub async fn connect() -> io::Result<(NamedPipeClient, u32)> {
    let client = ClientOptions::new().open(ipc::ui::PIPE_NAME)?;
    let server = win32_shell::pipe_server_session(&client)?;
    Ok((client, server))
}

/// Against a real console-mode service, which the test starts and kills
/// with its agent. It needs a logged-on desktop, the registered service not
/// serving the UI pipe, and both binaries built first:
///
/// ```text
/// cargo build -p mouseflock-service -p mouseflock-agent
/// cargo test -p mouseflock-desktop -- --ignored
/// ```
#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use super::*;
    use crate::link::{Link, Sink, run};
    use crate::view::{Notice, View};

    struct Console(Child);

    impl Console {
        fn start() -> Self {
            // target\debug\deps\mouseflock_desktop-….exe → target\debug
            let exe = std::env::current_exe().unwrap();
            let service: PathBuf = exe
                .parent()
                .and_then(|deps| deps.parent())
                .unwrap()
                .join("mouseflock-service.exe");
            assert!(
                service.exists(),
                "run `cargo build -p mouseflock-service -p mouseflock-agent` first"
            );
            let dir = std::env::temp_dir().join(format!("mouseflock-ui-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let child = Command::new(service)
                .args(["--console", "--port", "0", "--data"])
                .arg(&dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            Self(child)
        }
    }

    impl Drop for Console {
        fn drop(&mut self) {
            // The tree, so the agent it launched goes too.
            let _ = Command::new("taskkill")
                .args(["/F", "/T", "/PID", &self.0.id().to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }

    #[derive(Clone, Default)]
    struct Last(Arc<Mutex<Option<View>>>);

    impl Sink for Last {
        fn view(&self, view: &View) {
            *self.0.lock().unwrap() = Some(view.clone());
        }
        fn notice(&self, _: &Notice) {}
    }

    /// Runs a link for up to `within`, returning the first connected view.
    async fn first_view(dev: bool, within: Duration) -> Option<View> {
        let last = Last::default();
        let link = Link::new(last.clone());
        let own = win32_shell::own_session().unwrap();
        let task = tokio::spawn(run(link, connect, own, dev, Duration::from_millis(200)));
        let end = Instant::now() + within;
        let mut seen = None;
        while Instant::now() < end {
            if let Some(v) = last.0.lock().unwrap().clone().filter(|v| v.connected) {
                seen = Some(v);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        task.abort();
        seen
    }

    #[tokio::test]
    #[ignore = "starts a console-mode service and its agent"]
    async fn console_mode_is_trusted_only_by_development_builds() {
        let _console = Console::start();
        let view = first_view(true, Duration::from_secs(20))
            .await
            .expect("a development link connects to console mode");
        let name = view.status.unwrap().name;
        assert_eq!(name, std::env::var("COMPUTERNAME").unwrap());
        assert_eq!(first_view(false, Duration::from_secs(3)).await, None);
    }
}
