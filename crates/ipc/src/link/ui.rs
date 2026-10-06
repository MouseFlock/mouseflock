//! Both ends of the UI link (add-ui-link design D3). The service end never
//! waits on a UI: events are queued, and a UI with more than
//! [`MAX_UI_QUEUED`] unsent events is cut off.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use tokio::io::{AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::mpsc;

use super::common::{Closed, FrameReader, LinkError, Shared, Stream, handshake, write_frame};
use crate::message::Role;
use crate::ui::{UiCommand, UiEvent};

/// Most events the service may queue for a UI that is not reading; more
/// closes that UI's link.
pub const MAX_UI_QUEUED: usize = 256;

// ---- service end ---------------------------------------------------------------

/// Sends events to one UI. Dropping it closes the link.
pub struct UiHandle {
    tx: mpsc::UnboundedSender<UiEvent>,
    shared: Arc<Shared>,
}

impl UiHandle {
    /// Queues an event. Never waits; more than [`MAX_UI_QUEUED`] unsent
    /// events closes the link.
    pub fn send(&self, event: UiEvent) -> Result<(), Closed> {
        if self.shared.stopped() {
            return Err(Closed);
        }
        let queued = self.shared.queued.fetch_add(1, Ordering::AcqRel);
        if queued >= MAX_UI_QUEUED {
            self.shared.stop();
            return Err(Closed);
        }
        self.tx.send(event).map_err(|_| Closed)
    }

    /// True once the link has closed, for any reason.
    pub fn is_closed(&self) -> bool {
        self.shared.stopped()
    }

    pub fn close(&self) {
        self.shared.stop();
    }
}

impl Drop for UiHandle {
    fn drop(&mut self) {
        self.shared.stop();
    }
}

/// The service's end of a UI's link.
pub struct ServiceUiEnd;

impl ServiceUiEnd {
    /// Exchanges `Hello` with a UI (role 3 only) and starts the link. The
    /// receiver yields the UI's commands and ends when the link closes.
    pub async fn accept<S: Stream>(
        stream: S,
    ) -> Result<(UiHandle, mpsc::UnboundedReceiver<UiCommand>), LinkError> {
        let (version, reader, writer) = handshake(stream, Role::Service, Role::Ui).await?;
        let shared = Shared::new();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        tokio::spawn(service_writer(writer, ev_rx, shared.clone()));
        tokio::spawn(service_reader(reader, version, cmd_tx, shared.clone()));
        Ok((UiHandle { tx: ev_tx, shared }, cmd_rx))
    }
}

async fn service_writer<S: Stream>(
    mut w: WriteHalf<S>,
    mut events: mpsc::UnboundedReceiver<UiEvent>,
    shared: Arc<Shared>,
) {
    loop {
        let event = tokio::select! {
            () = shared.wait() => break,
            e = events.recv() => match e {
                Some(e) => {
                    shared.queued.fetch_sub(1, Ordering::AcqRel);
                    e
                }
                None => break,
            },
        };
        let mut bytes = Vec::new();
        if event.encode_frame(&mut bytes).is_err() || !write_frame(&mut w, &bytes).await {
            break;
        }
    }
    shared.stop();
    let _ = w.shutdown().await;
}

async fn service_reader<S: Stream>(
    mut reader: FrameReader<ReadHalf<S>>,
    version: u16,
    commands: mpsc::UnboundedSender<UiCommand>,
    shared: Arc<Shared>,
) {
    loop {
        let next = tokio::select! {
            () = shared.wait() => break,
            next = reader.next() => next,
        };
        let Ok(Some((ty, payload))) = next else {
            break;
        };
        let Ok(command) = UiCommand::decode(ty, &payload, version) else {
            break;
        };
        if commands.send(command).is_err() {
            break;
        }
    }
    shared.stop();
}

// ---- UI end --------------------------------------------------------------------

/// Sends commands to the service. Dropping it closes the link.
pub struct ServiceUiHandle {
    tx: mpsc::UnboundedSender<UiCommand>,
    shared: Arc<Shared>,
}

impl ServiceUiHandle {
    /// Queues a command. Never waits.
    pub fn send(&self, command: UiCommand) -> Result<(), Closed> {
        if self.shared.stopped() {
            return Err(Closed);
        }
        self.tx.send(command).map_err(|_| Closed)
    }

    pub fn close(&self) {
        self.shared.stop();
    }
}

impl Drop for ServiceUiHandle {
    fn drop(&mut self) {
        self.shared.stop();
    }
}

/// A UI's end of its link to the service.
pub struct UiEnd;

impl UiEnd {
    /// Exchanges `Hello` with the service and starts the link. The receiver
    /// yields `Status` and notices, and ends when the link closes.
    pub async fn connect<S: Stream>(
        stream: S,
    ) -> Result<(ServiceUiHandle, mpsc::UnboundedReceiver<UiEvent>), LinkError> {
        let (version, reader, writer) = handshake(stream, Role::Ui, Role::Service).await?;
        let shared = Shared::new();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        tokio::spawn(ui_writer(writer, cmd_rx, shared.clone()));
        tokio::spawn(ui_reader(reader, version, ev_tx, shared.clone()));
        Ok((ServiceUiHandle { tx: cmd_tx, shared }, ev_rx))
    }
}

async fn ui_writer<S: Stream>(
    mut w: WriteHalf<S>,
    mut commands: mpsc::UnboundedReceiver<UiCommand>,
    shared: Arc<Shared>,
) {
    loop {
        let command = tokio::select! {
            () = shared.wait() => break,
            c = commands.recv() => match c {
                Some(c) => c,
                None => break,
            },
        };
        let mut bytes = Vec::new();
        // A command the protocol cannot carry, such as an over-long code, is
        // skipped rather than ending the link.
        if command.encode_frame(&mut bytes).is_err() {
            continue;
        }
        if !write_frame(&mut w, &bytes).await {
            break;
        }
    }
    shared.stop();
    let _ = w.shutdown().await;
}

async fn ui_reader<S: Stream>(
    mut reader: FrameReader<ReadHalf<S>>,
    version: u16,
    events: mpsc::UnboundedSender<UiEvent>,
    shared: Arc<Shared>,
) {
    loop {
        let next = tokio::select! {
            () = shared.wait() => break,
            next = reader.next() => next,
        };
        let Ok(Some((ty, payload))) = next else {
            break;
        };
        let Ok(event) = UiEvent::decode(ty, &payload, version) else {
            break;
        };
        if events.send(event).is_err() {
            break;
        }
    }
    shared.stop();
}
