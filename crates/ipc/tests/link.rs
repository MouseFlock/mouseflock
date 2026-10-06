//! Both ends of the agent link over in-memory duplex streams (specs:
//! ipc-protocol, agent-link).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use ipc::frame::{self, Decoded};
use ipc::link::{AgentEnd, FromAgent, FromService, LinkError, MAX_QUEUED, ServiceEnd};
use ipc::message::{Hello, Role, ToAgent, ToService};
use model::{CaptureMode, InputAction, InputEvent, Key, Millis, Origin, Point};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

/// A clock the test sets.
#[derive(Clone, Default)]
struct Clock(Arc<AtomicU64>);

impl Clock {
    fn set(&self, ms: u64) {
        self.0.store(ms, Ordering::SeqCst);
    }

    fn reader(&self) -> impl Fn() -> Millis + Send + Sync + 'static {
        let c = self.0.clone();
        move || Millis(c.load(Ordering::SeqCst))
    }
}

/// An agent driven by hand: raw frames in and out.
struct Raw {
    s: DuplexStream,
    buf: Vec<u8>,
}

impl Raw {
    async fn send(&mut self, bytes: &[u8]) {
        self.s.write_all(bytes).await.unwrap();
    }

    async fn hello(&mut self, hello: Hello) {
        let mut out = Vec::new();
        hello.encode_frame(&mut out).unwrap();
        self.send(&out).await;
    }

    async fn report(&mut self, m: ToService) {
        let mut out = Vec::new();
        m.encode_frame(&mut out).unwrap();
        self.send(&out).await;
    }

    /// The next frame, or `None` at the end of the stream.
    async fn frame(&mut self) -> Option<(u16, Vec<u8>)> {
        loop {
            if let Ok(Decoded::Frame { frame, consumed }) = frame::decode(&self.buf) {
                let out = (frame.ty, frame.payload.to_vec());
                self.buf.drain(..consumed);
                return Some(out);
            }
            let mut chunk = [0u8; 1024];
            let n = self.s.read(&mut chunk).await.unwrap_or(0);
            if n == 0 {
                return None;
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }

    async fn command(&mut self) -> Option<ToAgent> {
        let (ty, payload) = self.frame().await?;
        Some(ToAgent::decode(ty, &payload, 1).unwrap())
    }
}

/// A service end on one side of a duplex, and a raw agent that has sent
/// its `Hello` and read the service's.
async fn service_with_raw_agent(
    capacity: usize,
    clock: &Clock,
) -> (
    ipc::link::AgentHandle,
    tokio::sync::mpsc::UnboundedReceiver<FromAgent>,
    Raw,
) {
    let (a, b) = tokio::io::duplex(capacity);
    let mut raw = Raw {
        s: b,
        buf: Vec::new(),
    };
    raw.hello(Hello::this_release(Role::SessionAgent)).await;
    let (handle, events) = ServiceEnd::accept(a, clock.reader()).await.unwrap();
    let (ty, _) = raw.frame().await.unwrap();
    assert_eq!(ty, ipc::message::HELLO);
    (handle, events, raw)
}

fn key(at: u64, usage: u16, down: bool) -> ToService {
    ToService::Input {
        at: Millis(at),
        input: InputEvent::Key {
            key: Key(usage),
            down,
        },
        origin: Origin::Physical,
    }
}

// ---- 2.1 -----------------------------------------------------------------

#[tokio::test]
async fn handshake_agrees_version_1() {
    let (a, b) = tokio::io::duplex(1024);
    let (service, agent) = tokio::join!(
        ServiceEnd::accept(a, || Millis(0)),
        AgentEnd::connect(b, || Millis(0)),
    );
    assert!(service.is_ok());
    assert!(agent.is_ok());
}

#[tokio::test]
async fn wrong_role_is_refused() {
    let (a, b) = tokio::io::duplex(1024);
    let mut raw = Raw {
        s: b,
        buf: Vec::new(),
    };
    raw.hello(Hello::this_release(Role::Ui)).await;
    let result = ServiceEnd::accept(a, || Millis(0)).await;
    assert!(matches!(result, Err(LinkError::WrongRole(Role::Ui))));
}

#[tokio::test]
async fn versions_do_not_overlap() {
    let (a, b) = tokio::io::duplex(1024);
    let mut raw = Raw {
        s: b,
        buf: Vec::new(),
    };
    raw.hello(Hello {
        min: 2,
        max: 3,
        role: Role::SessionAgent,
    })
    .await;
    let result = ServiceEnd::accept(a, || Millis(0)).await;
    let Err(e @ LinkError::VersionMismatch { .. }) = result else {
        panic!("expected a version mismatch");
    };
    assert!(e.to_string().contains("this end is older"), "{e}");
}

#[tokio::test(start_paused = true)]
async fn silent_peer_times_out() {
    let (a, _b) = tokio::io::duplex(1024);
    let result = ServiceEnd::accept(a, || Millis(0)).await;
    assert!(matches!(result, Err(LinkError::Timeout)));
}

#[tokio::test]
async fn commands_arrive_in_order() {
    let (a, b) = tokio::io::duplex(1024);
    let (service, agent) = tokio::join!(
        ServiceEnd::accept(a, || Millis(0)),
        AgentEnd::connect(b, || Millis(0)),
    );
    let ((handle, _events), (_reports, mut commands)) = (service.unwrap(), agent.unwrap());
    let sent = [
        ToAgent::SetCapture(CaptureMode::WithholdAll),
        ToAgent::Inject(InputAction::Key {
            key: Key(0x73),
            down: true,
        }),
        ToAgent::Inject(InputAction::Motion { dx: 3, dy: -4 }),
        ToAgent::Inject(InputAction::Key {
            key: Key(0x73),
            down: false,
        }),
        ToAgent::Warp(Point::new(100, 200)),
    ];
    for cmd in sent {
        handle.send(cmd).unwrap();
    }
    for want in sent {
        assert_eq!(commands.recv().await, Some(FromService::Command(want)));
    }
}

#[tokio::test]
async fn protocol_error_closes_the_link() {
    let clock = Clock::default();
    let (handle, mut events, mut raw) = service_with_raw_agent(1024, &clock).await;
    // An agent may not send a service command.
    let mut bad = Vec::new();
    ToAgent::SetCapture(CaptureMode::PassAll)
        .encode_frame(&mut bad)
        .unwrap();
    raw.send(&bad).await;
    assert_eq!(events.recv().await, None);
    // The raw agent sees the stream end (after any Ping already written).
    while raw.frame().await.is_some() {}
    assert!(handle.send(ToAgent::Warp(Point::new(0, 0))).is_err());
}

#[tokio::test]
async fn dropping_the_handle_closes_the_agent_end() {
    let (a, b) = tokio::io::duplex(1024);
    let (service, agent) = tokio::join!(
        ServiceEnd::accept(a, || Millis(0)),
        AgentEnd::connect(b, || Millis(0)),
    );
    let ((handle, _events), (_reports, mut commands)) = (service.unwrap(), agent.unwrap());
    drop(handle);
    assert_eq!(commands.recv().await, None);
}

// ---- 2.2 -----------------------------------------------------------------

#[tokio::test]
async fn events_are_converted_to_the_service_clock() {
    let clock = Clock::default();
    clock.set(1_000);
    let (_handle, mut events, mut raw) = service_with_raw_agent(1024, &clock).await;
    assert_eq!(raw.command().await, Some(ToAgent::Ping(Millis(1_000))));
    clock.set(1_004);
    raw.report(ToService::Pong {
        service: Millis(1_000),
        agent: Millis(51_002),
    })
    .await;
    raw.report(key(51_500, 0x04, true)).await;
    assert_eq!(
        events.recv().await,
        Some(FromAgent::Input {
            at: Millis(1_500),
            input: InputEvent::Key {
                key: Key(0x04),
                down: true
            },
            origin: Origin::Physical,
        })
    );
}

#[tokio::test]
async fn events_wait_for_the_first_pong() {
    let clock = Clock::default();
    clock.set(2_000);
    let (_handle, mut events, mut raw) = service_with_raw_agent(1024, &clock).await;
    raw.report(key(10, 0x04, true)).await;
    raw.report(key(11, 0x04, false)).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(events.try_recv().is_err(), "reported before the first Pong");

    raw.report(ToService::Pong {
        service: Millis(2_000),
        agent: Millis(5),
    })
    .await;
    // Offset: 5 − 2,000 = −1,995.
    for (at, down) in [(2_005, true), (2_006, false)] {
        let Some(FromAgent::Input {
            at: got,
            input: InputEvent::Key { down: d, .. },
            ..
        }) = events.recv().await
        else {
            panic!("expected a key");
        };
        assert_eq!((got, d), (Millis(at), down));
    }
}

#[tokio::test(start_paused = true)]
async fn pings_every_ten_seconds() {
    let clock = Clock::default();
    let (_handle, _events, mut raw) = service_with_raw_agent(1024, &clock).await;
    assert!(matches!(raw.command().await, Some(ToAgent::Ping(_))));
    let before = tokio::time::Instant::now();
    assert!(matches!(raw.command().await, Some(ToAgent::Ping(_))));
    let waited = before.elapsed();
    assert!(
        waited >= Duration::from_millis(9_990) && waited <= Duration::from_millis(10_010),
        "{waited:?}"
    );
}

#[tokio::test]
async fn agent_end_answers_ping_with_its_clock() {
    let (a, b) = tokio::io::duplex(1024);
    let agent_clock = Clock::default();
    agent_clock.set(777);
    let service_clock = Clock::default();
    service_clock.set(10);
    let (service, agent) = tokio::join!(
        ServiceEnd::accept(a, service_clock.reader()),
        AgentEnd::connect(b, agent_clock.reader()),
    );
    let ((_handle, mut events), (reports, _commands)) = (service.unwrap(), agent.unwrap());
    // Offset 767: an event at agent time 800 is service time 33.
    reports.send(key(800, 0x04, true)).unwrap();
    let Some(FromAgent::Input { at, .. }) = events.recv().await else {
        panic!("expected an input");
    };
    assert_eq!(at, Millis(33));
}

// ---- 2.3 -----------------------------------------------------------------

#[tokio::test]
async fn stalled_agent_does_not_block() {
    let clock = Clock::default();
    let (handle, mut events, mut raw) = service_with_raw_agent(64, &clock).await;
    // The agent never reads again, but answers a Ping it never read.
    raw.report(ToService::Pong {
        service: Millis(0),
        agent: Millis(0),
    })
    .await;

    let start = std::time::Instant::now();
    for i in 0..1_000 {
        let cmd = ToAgent::Inject(InputAction::Motion { dx: i, dy: 0 });
        handle.send(cmd).unwrap();
    }
    assert!(start.elapsed() < Duration::from_millis(100));

    // Its reports still arrive.
    raw.report(key(5, 0x04, true)).await;
    assert!(matches!(events.recv().await, Some(FromAgent::Input { .. })));

    // Past the cap, the link closes.
    let mut closed = false;
    for _ in 0..MAX_QUEUED {
        if handle.send(ToAgent::Warp(Point::new(0, 0))).is_err() {
            closed = true;
            break;
        }
    }
    assert!(
        closed,
        "the link stayed open past {MAX_QUEUED} queued commands"
    );
    assert_eq!(events.recv().await, None);
}
