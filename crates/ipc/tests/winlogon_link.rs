//! Agent links per role: each end refuses the other agent's role, and the
//! messages only one kind of agent may send (specs: agent-link,
//! ipc-protocol).

use std::time::Duration;

use ipc::clipboard::NotShared;
use ipc::link::{
    AgentEnd, AgentHandle, AgentRole, FromAgent, FromService, LinkError, ServiceEnd, ServiceHandle,
    SetClipboardError,
};
use ipc::message::{Hello, Role, ToAgent, ToService};
use model::{CaptureMode, Millis};
use tokio::io::{AsyncWriteExt, DuplexStream};
use tokio::sync::mpsc::UnboundedReceiver;

const WAIT: Duration = Duration::from_secs(2);

async fn linked(
    role: AgentRole,
) -> (
    AgentHandle,
    UnboundedReceiver<FromAgent>,
    ServiceHandle,
    UnboundedReceiver<FromService>,
) {
    let (a, b) = tokio::io::duplex(1 << 16);
    let (service, agent) = tokio::join!(
        ServiceEnd::accept(a, role, || Millis(0)),
        AgentEnd::connect(b, role, || Millis(0)),
    );
    let ((handle, events), (reports, commands)) = (service.unwrap(), agent.unwrap());
    (handle, events, reports, commands)
}

/// A service end expecting `expect`, and a raw client claiming `claims`.
async fn accept_claiming(
    expect: AgentRole,
    claims: Role,
) -> Result<(AgentHandle, UnboundedReceiver<FromAgent>), LinkError> {
    let (a, mut b): (DuplexStream, DuplexStream) = tokio::io::duplex(1024);
    let mut hello = Vec::new();
    Hello::this_release(claims)
        .encode_frame(&mut hello)
        .unwrap();
    b.write_all(&hello).await.unwrap();
    ServiceEnd::accept(a, expect, || Millis(0)).await
}

/// Waits for the link to end, skipping anything delivered first.
async fn ends(events: &mut UnboundedReceiver<FromAgent>) -> bool {
    tokio::time::timeout(WAIT, async { while events.recv().await.is_some() {} })
        .await
        .is_ok()
}

#[tokio::test]
async fn both_roles_link() {
    for role in [AgentRole::Session, AgentRole::Winlogon] {
        let (handle, ..) = linked(role).await;
        assert_eq!(handle.role(), role);
    }
}

#[tokio::test]
async fn session_agent_on_a_winlogon_pipe_is_refused() {
    let result = accept_claiming(AgentRole::Winlogon, Role::SessionAgent).await;
    assert!(matches!(
        result,
        Err(LinkError::WrongRole(Role::SessionAgent))
    ));
}

#[tokio::test]
async fn winlogon_agent_on_a_session_pipe_is_refused() {
    let result = accept_claiming(AgentRole::Session, Role::WinlogonAgent).await;
    assert!(matches!(
        result,
        Err(LinkError::WrongRole(Role::WinlogonAgent))
    ));
}

#[tokio::test]
async fn input_desktop_arrives_from_the_winlogon_agent() {
    let (_handle, mut events, reports, _commands) = linked(AgentRole::Winlogon).await;
    for active in [true, false] {
        reports.send(ToService::InputDesktop(active)).unwrap();
        let got = tokio::time::timeout(WAIT, events.recv()).await.unwrap();
        assert_eq!(got, Some(FromAgent::InputDesktop(active)));
    }
}

#[tokio::test]
async fn input_desktop_from_a_session_agent_closes_the_link() {
    let (_handle, mut events, reports, _commands) = linked(AgentRole::Session).await;
    reports.send(ToService::InputDesktop(true)).unwrap();
    assert!(ends(&mut events).await, "the link stayed up");
}

#[tokio::test]
async fn clipboard_from_a_winlogon_agent_closes_the_link() {
    for report in [
        ToService::ClipboardNotShared(NotShared::Private),
        ToService::ClipboardText(0),
    ] {
        let (_handle, mut events, reports, _commands) = linked(AgentRole::Winlogon).await;
        reports.send(report).unwrap();
        assert!(ends(&mut events).await, "the link stayed up");
    }
}

#[tokio::test]
async fn no_clipboard_text_on_a_winlogon_link() {
    let (handle, _events, _reports, mut commands) = linked(AgentRole::Winlogon).await;
    assert_eq!(
        handle.set_clipboard("secret"),
        Err(SetClipboardError::NoClipboard)
    );
    // Nothing was sent: the next thing the agent sees is the next command.
    handle
        .send(ToAgent::SetCapture(CaptureMode::WithholdAll))
        .unwrap();
    let got = tokio::time::timeout(WAIT, commands.recv()).await.unwrap();
    assert_eq!(
        got,
        Some(FromService::Command(ToAgent::SetCapture(
            CaptureMode::WithholdAll
        )))
    );
}
