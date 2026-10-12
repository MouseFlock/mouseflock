//! The agent's pipe (one instance, SYSTEM and one user) and the UI pipe (up
//! to [`MAX_UI_INSTANCES`], SYSTEM and interactive users or one user). Both
//! refuse remote clients.

use std::io;
use std::os::windows::io::AsRawHandle;

use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Pipes::{GetNamedPipeClientProcessId, GetNamedPipeClientSessionId};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::core::PCWSTR;

use crate::handle::{Local, wide};
use crate::sid::{SYSTEM, Sid};
use crate::token::SessionId;

/// Most UI pipe instances, and so UIs connected, at once.
pub const MAX_UI_INSTANCES: usize = 8;

/// Who may open the UI pipe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiAdmit {
    /// The registered service: SYSTEM and interactive users (`IU`). The
    /// service then accepts only the console session.
    InteractiveUsers,
    /// Console mode: SYSTEM and this one user.
    User(Sid),
}

/// The UI pipe's DACL, protected so nothing is inherited.
pub fn ui_pipe_sddl(admit: &UiAdmit) -> String {
    match admit {
        UiAdmit::InteractiveUsers => "D:P(A;;GA;;;SY)(A;;GRGW;;;IU)".to_owned(),
        UiAdmit::User(user) => agent_pipe_sddl(user),
    }
}

/// Creates an instance of the UI pipe `name`. With `first`, it must be the
/// first instance of that name and fails if the name exists. At most
/// [`MAX_UI_INSTANCES`] instances can exist; Windows enforces the first
/// creator's limit on every later one.
pub fn create_ui_pipe(name: &str, first: bool, admit: &UiAdmit) -> io::Result<NamedPipeServer> {
    let descriptor = Descriptor::from_sddl(&ui_pipe_sddl(admit))?;
    let mut attributes = descriptor.attributes();
    let mut options = ServerOptions::new();
    options
        .first_pipe_instance(first)
        .reject_remote_clients(true)
        .max_instances(MAX_UI_INSTANCES);
    // SAFETY: `attributes` is a valid SECURITY_ATTRIBUTES whose descriptor
    // outlives the call; tokio passes it straight to CreateNamedPipeW.
    unsafe { options.create_with_security_attributes_raw(name, (&raw mut attributes).cast()) }
}

/// The session of the client connected to `pipe`.
pub fn client_session_id(pipe: &NamedPipeServer) -> io::Result<SessionId> {
    let mut session = 0u32;
    // SAFETY: the handle stays open for the borrow of `pipe`; `session` is a
    // valid out-pointer.
    unsafe { GetNamedPipeClientSessionId(HANDLE(pipe.as_raw_handle()), &mut session) }?;
    Ok(SessionId(session))
}

/// This process's session.
pub fn own_session() -> io::Result<SessionId> {
    let mut session = 0u32;
    // SAFETY: `session` is a valid out-pointer.
    unsafe { ProcessIdToSessionId(std::process::id(), &mut session) }?;
    Ok(SessionId(session))
}

/// The pipe's DACL: SYSTEM gets everything, `user` may read and write.
/// Protected, so nothing is inherited. For an agent running as SYSTEM (the
/// winlogon agent), SYSTEM is the only entry.
///
/// Generic write includes the right to create a pipe instance. That is
/// harmless: the pipe is created first with at most one instance, so no
/// second instance of its name can exist.
pub fn agent_pipe_sddl(user: &Sid) -> String {
    if user.as_str() == SYSTEM {
        return "D:P(A;;GA;;;SY)".to_owned();
    }
    format!("D:P(A;;GA;;;SY)(A;;GRGW;;;{user})")
}

/// A security descriptor built from SDDL, freed on drop.
pub(crate) struct Descriptor(Local);

impl Descriptor {
    pub(crate) fn from_sddl(sddl: &str) -> io::Result<Self> {
        let text = wide(sddl);
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `text` is NUL-terminated and outlives the call; `sd`
        // receives LocalAlloc memory that `Local` frees.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(text.as_ptr()),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )
        }?;
        Ok(Self(Local(sd.0)))
    }

    /// Attributes pointing at this descriptor, not inheritable.
    pub(crate) fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0.0,
            bInheritHandle: false.into(),
        }
    }
}

/// Creates the pipe `name` for an agent running as `user`: the first and
/// only instance of that name, refusing remote clients, with
/// [`agent_pipe_sddl`]. Fails if the name already exists.
pub fn create_agent_pipe(name: &str, user: &Sid) -> io::Result<NamedPipeServer> {
    let descriptor = Descriptor::from_sddl(&agent_pipe_sddl(user))?;
    let mut attributes = descriptor.attributes();
    let mut options = ServerOptions::new();
    options
        .first_pipe_instance(true)
        .reject_remote_clients(true)
        .max_instances(1);
    // SAFETY: `attributes` is a valid SECURITY_ATTRIBUTES whose descriptor
    // outlives the call; tokio passes it straight to CreateNamedPipeW.
    unsafe { options.create_with_security_attributes_raw(name, (&raw mut attributes).cast()) }
}

/// The process ID of the client connected to `pipe`.
pub fn client_process_id(pipe: &NamedPipeServer) -> io::Result<u32> {
    let mut pid = 0u32;
    // SAFETY: the handle stays open for the borrow of `pipe`; `pid` is a
    // valid out-pointer.
    unsafe { GetNamedPipeClientProcessId(HANDLE(pipe.as_raw_handle()), &mut pid) }?;
    Ok(pid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sid::GUESTS;
    use tokio::net::windows::named_pipe::ClientOptions;

    fn name(tag: &str) -> String {
        format!(
            r"\\.\pipe\mouseflock-test-session-{}-{tag}",
            std::process::id()
        )
    }

    #[test]
    fn sddl_names_system_and_the_user() {
        let user = Sid::parse("S-1-5-21-1-2-3-1001").unwrap();
        assert_eq!(
            agent_pipe_sddl(&user),
            "D:P(A;;GA;;;SY)(A;;GRGW;;;S-1-5-21-1-2-3-1001)"
        );
        Descriptor::from_sddl(&agent_pipe_sddl(&user)).unwrap();
    }

    #[test]
    fn sddl_for_a_system_agent_names_only_system() {
        let sddl = agent_pipe_sddl(&Sid::parse(SYSTEM).unwrap());
        assert_eq!(sddl, "D:P(A;;GA;;;SY)");
        assert_eq!(sddl.matches("(A;").count(), 1);
        Descriptor::from_sddl(&sddl).unwrap();
    }

    #[tokio::test]
    async fn system_only_pipe_denies_this_user() {
        // Tests run as a normal user (or an administrator), never SYSTEM.
        let name = name("system");
        let _server = create_agent_pipe(&name, &Sid::parse(SYSTEM).unwrap()).unwrap();
        let err = ClientOptions::new().open(&name).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");
    }

    #[test]
    fn ui_sddl() {
        assert_eq!(
            ui_pipe_sddl(&UiAdmit::InteractiveUsers),
            "D:P(A;;GA;;;SY)(A;;GRGW;;;IU)"
        );
        Descriptor::from_sddl(&ui_pipe_sddl(&UiAdmit::InteractiveUsers)).unwrap();
        let me = crate::own_sid().unwrap();
        assert_eq!(
            ui_pipe_sddl(&UiAdmit::User(me.clone())),
            agent_pipe_sddl(&me)
        );
    }

    #[tokio::test]
    async fn ui_pipe_for_own_user_opens() {
        let name = name("ui-own");
        let server =
            create_ui_pipe(&name, true, &UiAdmit::User(crate::own_sid().unwrap())).unwrap();
        let _client = ClientOptions::new().open(&name).unwrap();
        server.connect().await.unwrap();
        assert_eq!(client_session_id(&server).unwrap(), own_session().unwrap());
    }

    #[tokio::test]
    async fn interactive_users_may_open_the_ui_pipe() {
        // Tests run in an interactive logon, so this user is in `IU`.
        let name = name("ui-iu");
        let server = create_ui_pipe(&name, true, &UiAdmit::InteractiveUsers).unwrap();
        let _client = ClientOptions::new().open(&name).unwrap();
        server.connect().await.unwrap();
    }

    #[tokio::test]
    async fn ninth_ui_client_is_busy() {
        let name = name("ui-nine");
        let admit = UiAdmit::User(crate::own_sid().unwrap());
        let mut held = Vec::new();
        for i in 0..MAX_UI_INSTANCES {
            let server = create_ui_pipe(&name, i == 0, &admit).unwrap();
            let client = ClientOptions::new().open(&name).unwrap();
            server.connect().await.unwrap();
            held.push((server, client));
        }
        // No ninth instance, so a ninth client finds every instance busy.
        assert!(create_ui_pipe(&name, false, &admit).is_err());
        let err = ClientOptions::new().open(&name).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(231), "{err}"); // ERROR_PIPE_BUSY
    }

    #[tokio::test]
    async fn first_ui_instance_twice_fails() {
        let name = name("ui-first");
        let admit = UiAdmit::User(crate::own_sid().unwrap());
        let _first = create_ui_pipe(&name, true, &admit).unwrap();
        assert!(create_ui_pipe(&name, true, &admit).is_err());
        // A later instance of the same name is fine.
        let _second = create_ui_pipe(&name, false, &admit).unwrap();
    }

    #[tokio::test]
    async fn pipe_for_own_user_opens() {
        let name = name("own");
        let server = create_agent_pipe(&name, &crate::own_sid().unwrap()).unwrap();
        let _client = ClientOptions::new().open(&name).unwrap();
        server.connect().await.unwrap();
        assert_eq!(client_process_id(&server).unwrap(), std::process::id());
    }

    #[tokio::test]
    async fn pipe_for_other_sid_denies() {
        let name = name("other");
        let _server = create_agent_pipe(&name, &Sid::parse(GUESTS).unwrap()).unwrap();
        let err = ClientOptions::new().open(&name).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");
    }

    #[tokio::test]
    async fn second_instance_fails() {
        let name = name("second");
        let me = crate::own_sid().unwrap();
        let _first = create_agent_pipe(&name, &me).unwrap();
        assert!(create_agent_pipe(&name, &me).is_err());
        // Not even tokio's default options can add an instance.
        assert!(ServerOptions::new().create(&name).is_err());
    }
}
