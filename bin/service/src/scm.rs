//! Running under the Service Control Manager (design D9).
//!
//! The control handler runs on the SCM's thread and only forwards
//! [`Control`]s; for `Suspend` it waits up to 1 second for the goodbye.

use std::ffi::OsString;
use std::process::ExitCode;
use std::time::Duration;

use tokio::sync::mpsc;
use windows_service::service::{
    PowerEventParam, ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState,
    ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::{define_windows_service, service_dispatcher};

use crate::host::{GOODBYE_WAIT, HostCommand, Sas};
use crate::log::Log;
use crate::supervisor::SupervisorCommand;
use crate::{error, info, store};

pub const SERVICE_NAME: &str = "DeskHop";

/// What a service control asks of the service.
#[derive(Debug, PartialEq, Eq)]
pub enum Control {
    Stop,
    SessionsChanged,
    Suspend,
    Resume,
}

/// The control's meaning, or `None` for controls the service ignores.
pub fn classify(control: &ServiceControl) -> Option<Control> {
    match control {
        ServiceControl::Stop | ServiceControl::Shutdown => Some(Control::Stop),
        ServiceControl::SessionChange(_) => Some(Control::SessionsChanged),
        ServiceControl::PowerEvent(PowerEventParam::Suspend) => Some(Control::Suspend),
        ServiceControl::PowerEvent(
            PowerEventParam::ResumeAutomatic | PowerEventParam::ResumeSuspend,
        ) => Some(Control::Resume),
        _ => None,
    }
}

define_windows_service!(ffi_service_main, service_main);

/// Hands this thread to the SCM's dispatcher. Fails at once when the
/// process was not started by the SCM.
pub fn run() -> ExitCode {
    match service_dispatcher::start(SERVICE_NAME, ffi_service_main) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let cause = std::error::Error::source(&e)
                .map(|s| format!(": {s}"))
                .unwrap_or_default();
            eprintln!(
                "deskhop-service: {e}{cause}\n\
                 It runs as the Windows service `{SERVICE_NAME}` (see tools/dev/service.ps1), \
                 or in a terminal with --console."
            );
            ExitCode::FAILURE
        }
    }
}

fn status(state: ServiceState, accept: ServiceControlAccept, wait: Duration) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: accept,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: wait,
        process_id: None,
    }
}

fn service_main(_args: Vec<OsString>) {
    // Nothing to log to until the data folder is prepared.
    let _ = run_service();
}

enum Event {
    Control(Control),
    Suspend(std::sync::mpsc::Sender<()>),
}

fn run_service() -> windows_service::Result<()> {
    let (tx, mut events) = mpsc::unbounded_channel();
    let handler = move |control: ServiceControl| match classify(&control) {
        Some(Control::Suspend) => {
            let (done, wait) = std::sync::mpsc::channel();
            if tx.send(Event::Suspend(done)).is_ok() {
                let _ = wait.recv_timeout(GOODBYE_WAIT);
            }
            ServiceControlHandlerResult::NoError
        }
        Some(c) => {
            let _ = tx.send(Event::Control(c));
            ServiceControlHandlerResult::NoError
        }
        None if matches!(control, ServiceControl::Interrogate) => {
            ServiceControlHandlerResult::NoError
        }
        None => ServiceControlHandlerResult::NotImplemented,
    };
    let status_handle = service_control_handler::register(SERVICE_NAME, handler)?;
    status_handle.set_service_status(status(
        ServiceState::StartPending,
        ServiceControlAccept::empty(),
        Duration::from_secs(10),
    ))?;

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(_) => {
            status_handle.set_service_status(status(
                ServiceState::Stopped,
                ServiceControlAccept::empty(),
                Duration::ZERO,
            ))?;
            return Ok(());
        }
    };
    runtime.block_on(async {
        let dir = store::service_dir();
        let log = match store::prepare(&dir, true, &Log::memory()).and_then(|()| {
            Log::file(
                &dir.join(store::LOG),
                Some(win32_session::create_admin_readable),
            )
        }) {
            Ok(log) => log,
            Err(_) => {
                let _ = status_handle.set_service_status(status(
                    ServiceState::Stopped,
                    ServiceControlAccept::empty(),
                    Duration::ZERO,
                ));
                return;
            }
        };
        info!(log, "starting, version {}", env!("CARGO_PKG_VERSION"));
        let service = match crate::start(
            false,
            dir,
            transport::PORT,
            log.clone(),
            None,
            Sas::Service,
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                error!(log, "cannot start: {e}");
                let _ = status_handle.set_service_status(status(
                    ServiceState::Stopped,
                    ServiceControlAccept::empty(),
                    Duration::ZERO,
                ));
                return;
            }
        };
        let _ = status_handle.set_service_status(status(
            ServiceState::Running,
            ServiceControlAccept::STOP
                | ServiceControlAccept::SHUTDOWN
                | ServiceControlAccept::SESSION_CHANGE
                | ServiceControlAccept::POWER_EVENT,
            Duration::ZERO,
        ));
        while let Some(event) = events.recv().await {
            match event {
                Event::Control(Control::Stop) => break,
                Event::Control(Control::SessionsChanged) => {
                    let _ = service.supervisor.send(SupervisorCommand::SessionsChanged);
                }
                Event::Control(Control::Resume) => {
                    let _ = service.host.send(HostCommand::Resume);
                }
                Event::Control(Control::Suspend) => {}
                Event::Suspend(done) => {
                    let _ = service.host.send(HostCommand::Suspend(done));
                }
            }
        }
        let _ = status_handle.set_service_status(status(
            ServiceState::StopPending,
            ServiceControlAccept::empty(),
            Duration::from_secs(5),
        ));
        crate::stop(service).await;
        info!(log, "stopped");
    });
    status_handle.set_service_status(status(
        ServiceState::Stopped,
        ServiceControlAccept::empty(),
        Duration::ZERO,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_map_to_what_the_service_does() {
        assert_eq!(classify(&ServiceControl::Stop), Some(Control::Stop));
        assert_eq!(classify(&ServiceControl::Shutdown), Some(Control::Stop));
        assert_eq!(
            classify(&ServiceControl::PowerEvent(PowerEventParam::Suspend)),
            Some(Control::Suspend)
        );
        for resume in [
            PowerEventParam::ResumeAutomatic,
            PowerEventParam::ResumeSuspend,
        ] {
            assert_eq!(
                classify(&ServiceControl::PowerEvent(resume)),
                Some(Control::Resume)
            );
        }
        assert_eq!(
            classify(&ServiceControl::PowerEvent(PowerEventParam::BatteryLow)),
            None
        );
        assert_eq!(classify(&ServiceControl::Interrogate), None);
        assert_eq!(classify(&ServiceControl::Pause), None);
    }
}
