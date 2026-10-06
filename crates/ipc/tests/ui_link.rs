//! Both ends of the UI link over in-memory streams (spec: ui-link).

use std::time::{Duration, Instant};

use ipc::link::{LinkError, MAX_UI_QUEUED, ServiceUiEnd, UiEnd};
use ipc::message::{Hello, Role};
use ipc::ui::{FocusOf, Pairing, Status, UiCommand, UiEvent};
use tokio::io::AsyncWriteExt;

fn status(n: u32) -> UiEvent {
    UiEvent::Status(Status {
        key: [1; 32],
        name: "A".into(),
        agent_linked: true,
        pairing: Pairing::Open {
            code: "12-3456789".into(),
            left_ms: n,
        },
        focus: FocusOf::Local,
        members: Vec::new(),
    })
}

#[tokio::test]
async fn ui_handshake_agrees_version_1() {
    let (a, b) = tokio::io::duplex(1 << 16);
    let (service, ui) = tokio::join!(ServiceUiEnd::accept(a), UiEnd::connect(b));
    let (to_ui, mut commands) = service.unwrap();
    let (to_service, mut events) = ui.unwrap();
    to_service.send(UiCommand::Undo).unwrap();
    assert_eq!(commands.recv().await, Some(UiCommand::Undo));
    to_ui.send(status(1)).unwrap();
    assert_eq!(events.recv().await, Some(status(1)));
}

#[tokio::test]
async fn agent_role_on_ui_end_is_refused() {
    let (a, mut b) = tokio::io::duplex(1 << 16);
    let mut hello = Vec::new();
    Hello::this_release(Role::SessionAgent)
        .encode_frame(&mut hello)
        .unwrap();
    b.write_all(&hello).await.unwrap();
    let Err(e) = ServiceUiEnd::accept(a).await else {
        panic!("accepted an agent on the UI end");
    };
    assert!(matches!(e, LinkError::WrongRole(Role::SessionAgent)), "{e}");
}

#[tokio::test]
async fn events_arrive_in_order() {
    let (a, b) = tokio::io::duplex(1 << 16);
    let (service, ui) = tokio::join!(ServiceUiEnd::accept(a), UiEnd::connect(b));
    let (to_ui, _commands) = service.unwrap();
    let (_to_service, mut events) = ui.unwrap();
    for i in 0..100 {
        to_ui.send(status(i)).unwrap();
    }
    for i in 0..100 {
        assert_eq!(events.recv().await, Some(status(i)));
    }
}

#[tokio::test]
async fn commands_arrive_in_order() {
    let (a, b) = tokio::io::duplex(1 << 16);
    let (service, ui) = tokio::join!(ServiceUiEnd::accept(a), UiEnd::connect(b));
    let (_to_ui, mut commands) = service.unwrap();
    let (to_service, _events) = ui.unwrap();
    let sent = [
        UiCommand::OpenPairing,
        UiCommand::Join("12-3456789".into()),
        UiCommand::RemovePeer([3; 32]),
        UiCommand::ForgetLayout,
        UiCommand::ClosePairing,
    ];
    for c in &sent {
        to_service.send(c.clone()).unwrap();
    }
    for c in sent {
        assert_eq!(commands.recv().await, Some(c));
    }
}

#[tokio::test]
async fn stalled_ui_is_cut_off() {
    // A UI that says Hello and never reads again, over a tiny pipe.
    let (a, mut b) = tokio::io::duplex(64);
    let mut hello = Vec::new();
    Hello::this_release(Role::Ui)
        .encode_frame(&mut hello)
        .unwrap();
    b.write_all(&hello).await.unwrap();
    let (to_ui, mut commands) = ServiceUiEnd::accept(a).await.unwrap();

    let start = Instant::now();
    let refused = (0..1_000)
        .filter(|&i| to_ui.send(status(i)).is_err())
        .count();
    assert!(
        start.elapsed() < Duration::from_millis(100),
        "{:?}",
        start.elapsed()
    );
    assert!(
        refused >= 1_000 - MAX_UI_QUEUED - 1,
        "only {refused} sends refused"
    );
    assert!(to_ui.is_closed());
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), commands.recv())
            .await
            .unwrap(),
        None
    );
    drop(b);
}
