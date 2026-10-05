//! The agent's pipe: one instance, local clients only, SYSTEM and one user.

use std::io;
use std::os::windows::io::AsRawHandle;

use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Pipes::GetNamedPipeClientProcessId;
use windows::core::PCWSTR;

use crate::handle::{Local, wide};
use crate::sid::Sid;

/// The pipe's DACL: SYSTEM gets everything, `user` may read and write.
/// Protected, so nothing is inherited.
///
/// Generic write includes the right to create a pipe instance. That is
/// harmless: the pipe is created first with at most one instance, so no
/// second instance of its name can exist.
pub fn agent_pipe_sddl(user: &Sid) -> String {
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
            r"\\.\pipe\deskhop-test-session-{}-{tag}",
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
