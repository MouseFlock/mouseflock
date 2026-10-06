//! The app's link to the service (design D2): connect, check the server's
//! session, run the UI link, and reconnect when it ends. Events become view
//! data for a [`Sink`]; commands go through [`Link`].
//!
//! Nothing here knows about Tauri, so tests drive it over `tokio::io::duplex`
//! with a real `ServiceUiEnd`.

use std::future::Future;
use std::io;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use ipc::link::{ServiceUiHandle, Stream, UiEnd};
use ipc::ui::{Status, UiCommand, UiEvent};
use serde::Serialize;

use crate::checks::{normalize_code, trust};
use crate::view::{Notice, View, notice, parse_key};

/// How long to wait before trying the service again.
pub const RETRY: Duration = Duration::from_secs(1);

/// Where view data goes: the windows and the tray, or a test's record.
pub trait Sink: Send + Sync + 'static {
    fn view(&self, view: &View);
    fn notice(&self, notice: &Notice);
}

/// Why a command was not sent. Serialized as a tag React maps to a sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CommandError {
    NotConnected,
    InvalidCode,
    InvalidKey,
}

#[derive(Default)]
struct State {
    handle: Option<ServiceUiHandle>,
    latest: Option<Status>,
}

/// The connection to the service, shared by the link task and the commands.
pub struct Link {
    state: Mutex<State>,
    sink: Box<dyn Sink>,
}

impl Link {
    pub fn new(sink: impl Sink) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State::default()),
            sink: Box::new(sink),
        })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The current view: disconnected until the first `Status`.
    pub fn view(&self) -> View {
        let state = self.state();
        match (&state.handle, &state.latest) {
            (Some(_), Some(status)) => View::connected(status),
            _ => View::disconnected(),
        }
    }

    pub fn send(&self, command: UiCommand) -> Result<(), CommandError> {
        let state = self.state();
        let (Some(handle), Some(_)) = (&state.handle, &state.latest) else {
            return Err(CommandError::NotConnected);
        };
        handle.send(command).map_err(|_| CommandError::NotConnected)
    }

    /// Sends a typed code, without its whitespace, if the link can carry it.
    pub fn join(&self, typed: &str) -> Result<(), CommandError> {
        let code = normalize_code(typed).map_err(|_| CommandError::InvalidCode)?;
        self.send(UiCommand::Join(code))
    }

    pub fn remove(&self, key: &str) -> Result<(), CommandError> {
        let key = parse_key(key).ok_or(CommandError::InvalidKey)?;
        self.send(UiCommand::RemovePeer(key))
    }

    fn attach(&self, handle: ServiceUiHandle) {
        let mut state = self.state();
        state.handle = Some(handle);
        state.latest = None;
    }

    fn detach(&self) {
        let was_connected = {
            let mut state = self.state();
            let was = state.latest.is_some();
            *state = State::default();
            was
        };
        if was_connected {
            self.sink.view(&View::disconnected());
        }
    }

    fn on_event(&self, event: UiEvent) {
        match event {
            UiEvent::Status(status) => {
                let view = View::connected(&status);
                self.state().latest = Some(status);
                self.sink.view(&view);
            }
            other => {
                let n = notice(&other, self.state().latest.as_ref());
                if let Some(n) = n {
                    self.sink.notice(&n);
                }
            }
        }
    }
}

/// Connects, checks, links and reconnects, forever. `connect` yields a
/// stream to the service and the session of the process serving it.
pub async fn run<S, F, Fut>(link: Arc<Link>, mut connect: F, own: u32, dev: bool, retry: Duration)
where
    S: Stream,
    F: FnMut() -> Fut,
    Fut: Future<Output = io::Result<(S, u32)>>,
{
    loop {
        // An untrusted server is dropped unread: no `Hello`, nothing shown.
        if let Ok((stream, server)) = connect().await
            && trust(server, own, dev)
            && let Ok((handle, mut events)) = UiEnd::connect(stream).await
        {
            link.attach(handle);
            while let Some(event) = events.recv().await {
                link.on_event(event);
            }
            link.detach();
        }
        tokio::time::sleep(retry).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ipc::link::{ServiceUiEnd, UiHandle};
    use ipc::ui::{FocusOf, Member, Pairing};
    use model::Side;
    use tokio::io::{AsyncReadExt, DuplexStream};
    use tokio::sync::{Notify, mpsc};
    use tokio::time::timeout;

    use crate::view::{FocusView, StatusView};

    const WAIT: Duration = Duration::from_secs(2);
    const OWN: u32 = 1;

    #[derive(Debug, Clone, PartialEq)]
    enum Rec {
        View(View),
        Notice(Notice),
    }

    #[derive(Clone, Default)]
    struct Recorder {
        recs: Arc<Mutex<Vec<Rec>>>,
        changed: Arc<Notify>,
    }

    impl Sink for Recorder {
        fn view(&self, view: &View) {
            self.recs.lock().unwrap().push(Rec::View(view.clone()));
            self.changed.notify_waiters();
        }
        fn notice(&self, notice: &Notice) {
            self.recs.lock().unwrap().push(Rec::Notice(notice.clone()));
            self.changed.notify_waiters();
        }
    }

    impl Recorder {
        /// Waits for a record matching `f`, and returns it.
        async fn wait(&self, f: impl Fn(&Rec) -> bool) -> Rec {
            timeout(WAIT, async {
                loop {
                    let changed = self.changed.notified();
                    if let Some(r) = self.recs.lock().unwrap().iter().find(|r| f(r)) {
                        return r.clone();
                    }
                    changed.await;
                }
            })
            .await
            .expect("no matching record")
        }
    }

    fn key(n: u8) -> [u8; 32] {
        [n; 32]
    }

    fn status(members: Vec<Member>) -> Status {
        Status {
            key: key(1),
            name: "DESK".into(),
            agent_linked: true,
            pairing: Pairing::Closed,
            focus: FocusOf::Local,
            members,
        }
    }

    fn office() -> Member {
        Member {
            key: key(2),
            name: "OFFICE".into(),
            up: true,
        }
    }

    type Offer = (DuplexStream, u32);

    /// A link whose connector takes streams from a channel, failing with
    /// not-found when none is waiting.
    fn start(dev: bool) -> (Arc<Link>, Recorder, mpsc::UnboundedSender<Offer>) {
        let rec = Recorder::default();
        let link = Link::new(rec.clone());
        let (tx, rx) = mpsc::unbounded_channel::<Offer>();
        let rx = Arc::new(tokio::sync::Mutex::new(rx));
        let connect = move || {
            let rx = rx.clone();
            async move {
                rx.lock()
                    .await
                    .try_recv()
                    .map_err(|_| io::Error::from(io::ErrorKind::NotFound))
            }
        };
        tokio::spawn(run(
            link.clone(),
            connect,
            OWN,
            dev,
            Duration::from_millis(10),
        ));
        (link, rec, tx)
    }

    /// Offers a stream from session `server`, and accepts it as the service.
    async fn serve(
        tx: &mpsc::UnboundedSender<Offer>,
        server: u32,
    ) -> (UiHandle, mpsc::UnboundedReceiver<UiCommand>) {
        let (ui, service) = tokio::io::duplex(64 * 1024);
        tx.send((ui, server)).unwrap();
        ServiceUiEnd::accept(service).await.unwrap()
    }

    fn connected(r: &Rec) -> bool {
        matches!(
            r,
            Rec::View(View {
                connected: true,
                ..
            })
        )
    }

    #[tokio::test]
    async fn status_becomes_view() {
        let (link, rec, tx) = start(false);
        assert_eq!(link.view(), View::disconnected());
        let (ui, _cmds) = serve(&tx, 0).await;
        ui.send(UiEvent::Status(status(vec![office()]))).unwrap();
        let Rec::View(view) = rec.wait(connected).await else {
            unreachable!()
        };
        let s: StatusView = view.status.clone().unwrap();
        assert_eq!(s.name, "DESK");
        assert_eq!(s.members[0].name, "OFFICE");
        assert_eq!(s.focus, FocusView::Local);
        assert_eq!(link.view(), view);
    }

    #[tokio::test]
    async fn join_sends_normalized_code() {
        let (link, rec, tx) = start(false);
        let (ui, mut cmds) = serve(&tx, 0).await;
        ui.send(UiEvent::Status(status(vec![]))).unwrap();
        rec.wait(connected).await;
        link.join(" 137 - 482 9153 ").unwrap();
        let cmd = timeout(WAIT, cmds.recv()).await.unwrap().unwrap();
        assert_eq!(cmd, UiCommand::Join("137-4829153".into()));
        link.remove(&"02".repeat(32)).unwrap();
        let cmd = timeout(WAIT, cmds.recv()).await.unwrap().unwrap();
        assert_eq!(cmd, UiCommand::RemovePeer(key(2)));
    }

    #[tokio::test]
    async fn invalid_code_sends_nothing() {
        let (link, rec, tx) = start(false);
        let (ui, mut cmds) = serve(&tx, 0).await;
        ui.send(UiEvent::Status(status(vec![]))).unwrap();
        rec.wait(connected).await;
        assert_eq!(link.join(&"1".repeat(40)), Err(CommandError::InvalidCode));
        assert_eq!(link.join("  "), Err(CommandError::InvalidCode));
        assert_eq!(link.remove("02"), Err(CommandError::InvalidKey));
        link.send(UiCommand::Undo).unwrap();
        // The next command through is the Undo: nothing was sent before it.
        let cmd = timeout(WAIT, cmds.recv()).await.unwrap().unwrap();
        assert_eq!(cmd, UiCommand::Undo);
    }

    #[tokio::test]
    async fn service_close_reports_not_connected_then_reconnects() {
        let (link, rec, tx) = start(false);
        let (ui, cmds) = serve(&tx, 0).await;
        ui.send(UiEvent::Status(status(vec![]))).unwrap();
        rec.wait(connected).await;

        drop((ui, cmds));
        rec.wait(|r| matches!(r, Rec::View(v) if !v.connected))
            .await;
        assert_eq!(link.view(), View::disconnected());
        assert_eq!(link.send(UiCommand::Undo), Err(CommandError::NotConnected));

        rec.recs.lock().unwrap().clear();
        let (ui, _cmds) = serve(&tx, 0).await;
        ui.send(UiEvent::Status(status(vec![office()]))).unwrap();
        rec.wait(connected).await;
        assert!(link.view().connected);
    }

    #[tokio::test]
    async fn untrusted_server_gets_no_hello() {
        let (link, _rec, tx) = start(false);
        let (ui, mut service) = tokio::io::duplex(64 * 1024);
        tx.send((ui, 5)).unwrap();
        // The app drops the stream unread: the service sees end of stream
        // with no `Hello`.
        let mut buf = [0u8; 16];
        let n = timeout(WAIT, service.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(n, 0);
        assert_eq!(link.view(), View::disconnected());

        // The own session is trusted only by a development build.
        let (link, rec, tx) = start(true);
        let (ui, _cmds) = serve(&tx, OWN).await;
        ui.send(UiEvent::Status(status(vec![]))).unwrap();
        rec.wait(connected).await;
        assert!(link.view().connected);
    }

    #[tokio::test]
    async fn learned_names_the_member() {
        let (_link, rec, tx) = start(false);
        let (ui, _cmds) = serve(&tx, 0).await;
        ui.send(UiEvent::Status(status(vec![office()]))).unwrap();
        ui.send(UiEvent::Learned {
            key: key(2),
            side: Side::Right,
            undo_ms: 10_000,
        })
        .unwrap();
        let rec = rec.wait(|r| matches!(r, Rec::Notice(_))).await;
        let Rec::Notice(Notice::Learned {
            name,
            side,
            left_ms,
            ..
        }) = rec
        else {
            panic!("not learned: {rec:?}");
        };
        assert_eq!(name, "OFFICE");
        assert_eq!(serde_json::to_value(side).unwrap(), "right");
        assert_eq!(left_ms, 10_000);
    }

    #[tokio::test]
    async fn commands_while_down_fail() {
        let (link, _rec, _tx) = start(false);
        assert_eq!(
            link.send(UiCommand::OpenPairing),
            Err(CommandError::NotConnected)
        );
        assert_eq!(link.join("137-4829153"), Err(CommandError::NotConnected));
        assert_eq!(
            link.remove(&"02".repeat(32)),
            Err(CommandError::NotConnected)
        );
    }
}
