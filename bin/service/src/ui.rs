//! The UI pipe server (add-ui-link design D5, D7).
//!
//! One instance always listens, so the name never falls free while the
//! service runs; up to 8 are connected. A connecting client is checked
//! against the console session before anything is read or sent, then the
//! `Hello` exchange runs and the link goes to the host.

use std::time::Duration;

use ipc::link::ServiceUiEnd;
use tokio::net::windows::named_pipe::NamedPipeServer;
use tokio::sync::mpsc;
use win32_session::{SessionId, UiAdmit};

use crate::host::UiLink;
use crate::log::Log;
use crate::{info, warn};

/// How often to try again while another process holds the name.
pub const RETRY: Duration = Duration::from_secs(5);

/// How often to try again while 8 UIs are connected.
const BUSY_RETRY: Duration = Duration::from_millis(250);

/// Which session a UI must run in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// The registered service: whatever session is at the console now.
    Console,
    /// Console mode: its own session.
    Session(SessionId),
}

impl Place {
    fn expected(self) -> Option<SessionId> {
        match self {
            Self::Console => win32_session::active_console_session(),
            Self::Session(s) => Some(s),
        }
    }
}

/// Whether a client in `client` may use the UI link.
pub fn session_allowed(client: SessionId, expected: Option<SessionId>) -> bool {
    expected == Some(client)
}

/// Serves `name` until the host goes away.
pub async fn serve(
    name: String,
    admit: UiAdmit,
    place: Place,
    uis: mpsc::UnboundedSender<UiLink>,
    log: Log,
    retry: Duration,
) {
    let mut waited = false;
    let mut listening = loop {
        match win32_session::create_ui_pipe(&name, true, &admit) {
            Ok(pipe) => break pipe,
            Err(e) => {
                if !waited {
                    warn!(
                        log,
                        "UI pipe {name} is in use ({e}); is the registered service running? \
                         Trying again every {} s",
                        retry.as_secs()
                    );
                    waited = true;
                }
                tokio::time::sleep(retry).await;
                if uis.is_closed() {
                    return;
                }
            }
        }
    };
    info!(log, "serving UIs on {name}");
    loop {
        let connected = listening.connect().await;
        let pipe = listening;
        let Some(next) = next_instance(&name, &admit, &uis).await else {
            return;
        };
        listening = next;
        if connected.is_ok() {
            tokio::spawn(admit_ui(pipe, place, uis.clone(), log.clone()));
        }
    }
}

/// The next listening instance, once fewer than 8 UIs are connected.
async fn next_instance(
    name: &str,
    admit: &UiAdmit,
    uis: &mpsc::UnboundedSender<UiLink>,
) -> Option<NamedPipeServer> {
    loop {
        if uis.is_closed() {
            return None;
        }
        if let Ok(pipe) = win32_session::create_ui_pipe(name, false, admit) {
            return Some(pipe);
        }
        tokio::time::sleep(BUSY_RETRY).await;
    }
}

async fn admit_ui(
    pipe: NamedPipeServer,
    place: Place,
    uis: mpsc::UnboundedSender<UiLink>,
    log: Log,
) {
    let client = match win32_session::client_session_id(&pipe) {
        Ok(s) => s,
        Err(e) => {
            warn!(log, "UI refused: cannot read its session: {e}");
            return;
        }
    };
    if !session_allowed(client, place.expected()) {
        info!(
            log,
            "UI from session {client} refused: not the console session"
        );
        return;
    }
    match ServiceUiEnd::accept(pipe).await {
        Ok((handle, commands)) => {
            let _ = uis.send(UiLink { handle, commands });
        }
        Err(e) => info!(log, "UI link not set up: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ipc::link::UiEnd;
    use ipc::ui::UiEvent;
    use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};

    #[test]
    fn only_the_expected_session() {
        assert!(session_allowed(SessionId(1), Some(SessionId(1))));
        assert!(!session_allowed(SessionId(2), Some(SessionId(1))));
        // No console session at all: nobody.
        assert!(!session_allowed(SessionId(1), None));
    }

    fn test_pipe(tag: &str) -> String {
        format!(r"\\.\pipe\deskhop-ui-test-{}-{tag}", std::process::id())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn name_taken_is_retried() {
        let name = test_pipe("taken");
        let squatter = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&name)
            .unwrap();
        let (uis, mut links) = mpsc::unbounded_channel();
        let me = win32_session::own_sid().unwrap();
        let session = win32_session::own_session().unwrap();
        tokio::spawn(serve(
            name.clone(),
            UiAdmit::User(me),
            Place::Session(session),
            uis,
            Log::memory(),
            RETRY,
        ));
        tokio::time::sleep(Duration::from_millis(500)).await;
        drop(squatter);

        // Within the next retry, the server holds the name and links a UI.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
        let client = loop {
            match ClientOptions::new().open(&name) {
                Ok(c) => break c,
                Err(_) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(e) => panic!("no UI pipe within 6 s: {e}"),
            }
        };
        let (ui, server_side) = tokio::join!(UiEnd::connect(client), links.recv());
        let (_to_service, _events) = ui.unwrap();
        let link = server_side.unwrap();
        link.handle
            .send(UiEvent::JoinFailed(ipc::ui::JoinFailedReason::WrongCode))
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn other_session_is_closed_unread() {
        let name = test_pipe("session");
        let (uis, mut links) = mpsc::unbounded_channel();
        let me = win32_session::own_sid().unwrap();
        let own = win32_session::own_session().unwrap();
        // Expect a session this test does not run in.
        tokio::spawn(serve(
            name.clone(),
            UiAdmit::User(me),
            Place::Session(SessionId(own.0.wrapping_add(1000))),
            uis,
            Log::memory(),
            RETRY,
        ));
        let client = loop {
            match ClientOptions::new().open(&name) {
                Ok(c) => break c,
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        };
        let Err(e) = UiEnd::connect(client).await else {
            panic!("a UI in another session was linked");
        };
        assert!(
            matches!(
                e,
                ipc::link::LinkError::Closed | ipc::link::LinkError::Io(_)
            ),
            "{e}"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(200), links.recv())
                .await
                .is_err()
        );
    }
}
