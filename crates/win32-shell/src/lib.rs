//! The UI's Win32 calls.
//!
//! One of four crates where `unsafe` is allowed (ADR 0007). Every `unsafe`
//! block carries a `// SAFETY:` comment. Linked only by `mouseflock-desktop`,
//! which stays `#![forbid(unsafe_code)]`. Nothing here touches input or
//! needs privileges.
//!
//! | Function | For |
//! |---|---|
//! | [`pipe_server_session`], [`own_session`] | checking that the UI pipe's server is the service |

use std::io;
use std::os::windows::io::AsRawHandle;

use tokio::net::windows::named_pipe::NamedPipeClient;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Pipes::GetNamedPipeServerSessionId;
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;

/// The session of the process serving `pipe`. Only services run in
/// session 0.
pub fn pipe_server_session(pipe: &NamedPipeClient) -> io::Result<u32> {
    let mut session = 0u32;
    // SAFETY: the handle stays open for the borrow of `pipe`; `session` is a
    // valid out-pointer.
    unsafe { GetNamedPipeServerSessionId(HANDLE(pipe.as_raw_handle()), &mut session) }?;
    Ok(session)
}

/// This process's session.
pub fn own_session() -> io::Result<u32> {
    let mut session = 0u32;
    // SAFETY: `session` is a valid out-pointer.
    unsafe { ProcessIdToSessionId(std::process::id(), &mut session) }?;
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};

    /// A server in this process is in this process's session. Outside
    /// session 0 (a desktop, not a service-hosted CI runner), that is the
    /// case a release UI refuses.
    #[tokio::test]
    async fn server_session_matches_own() {
        let name = format!(r"\\.\pipe\mouseflock-test-shell-{}", std::process::id());
        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .reject_remote_clients(true)
            .create(&name)
            .unwrap();
        let client = ClientOptions::new().open(&name).unwrap();
        server.connect().await.unwrap();
        assert_eq!(
            pipe_server_session(&client).unwrap(),
            own_session().unwrap()
        );
    }
}
