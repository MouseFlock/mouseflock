//! Launching a process as a session's user, and waiting on or ending it.

use std::io;
use std::path::Path;

use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW, GetExitCodeProcess,
    INFINITE, PROCESS_INFORMATION, STARTUPINFOW, TerminateProcess, WaitForSingleObject,
};
use windows::core::{PCWSTR, PWSTR};

use crate::handle::{Owned, wide};
use crate::token::Token;

/// A launched process. Its handle stays open for this value's lifetime, so
/// its process ID cannot be reused by another process meanwhile.
#[derive(Debug)]
pub struct Process {
    handle: Owned,
    id: u32,
}

impl Process {
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Blocks until the process exits, and returns its exit code.
    pub fn wait(&self) -> io::Result<u32> {
        // SAFETY: the handle is open for `self`'s lifetime.
        let event = unsafe { WaitForSingleObject(self.handle.0, INFINITE) };
        if event != WAIT_OBJECT_0 {
            return Err(io::Error::last_os_error());
        }
        let mut code = 0u32;
        // SAFETY: as above; `code` is a valid out-pointer.
        unsafe { GetExitCodeProcess(self.handle.0, &mut code) }?;
        Ok(code)
    }

    /// Ends the process with exit code 1. Ending one that already exited
    /// is not an error.
    pub fn terminate(&self) -> io::Result<()> {
        // SAFETY: the handle is open for `self`'s lifetime.
        match unsafe { TerminateProcess(self.handle.0, 1) } {
            Ok(()) => Ok(()),
            // Access denied once the process has exited.
            Err(_) if self.has_exited() => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn has_exited(&self) -> bool {
        // SAFETY: the handle is open for `self`'s lifetime; 0 ms never blocks.
        unsafe { WaitForSingleObject(self.handle.0, 0) == WAIT_OBJECT_0 }
    }
}

/// Quotes `exe` and `args` into a command line the way the C runtime parses
/// it back into the same arguments.
pub fn command_line(exe: &Path, args: &[&str]) -> String {
    let mut line = String::new();
    quote(&exe.to_string_lossy(), &mut line);
    for arg in args {
        line.push(' ');
        quote(arg, &mut line);
    }
    line
}

fn quote(arg: &str, out: &mut String) {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        out.push_str(arg);
        return;
    }
    out.push('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
}

/// Launches `exe` with `args` as `token`'s user, on the interactive desktop
/// of the token's session, without a window and with that user's
/// environment. No handles are inherited.
pub fn launch_as(token: &Token, exe: &Path, args: &[&str], dir: &Path) -> io::Result<Process> {
    let app = wide(exe);
    let mut line = wide(command_line(exe, args));
    let cwd = wide(dir);
    let mut desktop = wide(r"winsta0\default");

    let mut env = std::ptr::null_mut();
    // SAFETY: `env` is a valid out-pointer; the block is destroyed below.
    unsafe { CreateEnvironmentBlock(&mut env, Some(token.handle()), false) }?;

    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        lpDesktop: PWSTR(desktop.as_mut_ptr()),
        ..Default::default()
    };
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: every string is NUL-terminated and outlives the call; the
    // command line is writable as the API requires; `env` is a Unicode
    // environment block, flagged as such.
    let launched = unsafe {
        CreateProcessAsUserW(
            Some(token.handle()),
            PCWSTR(app.as_ptr()),
            Some(PWSTR(line.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
            Some(env),
            PCWSTR(cwd.as_ptr()),
            &startup,
            &mut info,
        )
    };
    // SAFETY: `env` came from CreateEnvironmentBlock and is not used again.
    let _ = unsafe { DestroyEnvironmentBlock(env) };
    launched?;
    drop(Owned(info.hThread));
    Ok(Process {
        handle: Owned(info.hProcess),
        id: info.dwProcessId,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_arguments_are_not_quoted() {
        assert_eq!(
            command_line(
                Path::new(r"C:\x\deskhop-agent.exe"),
                &["--session", "--pipe", r"\\.\pipe\deskhop-agent-00ff"]
            ),
            r"C:\x\deskhop-agent.exe --session --pipe \\.\pipe\deskhop-agent-00ff"
        );
    }

    #[test]
    fn spaces_quotes_and_trailing_backslashes() {
        assert_eq!(
            command_line(
                Path::new(r"C:\Program Files\DeskHop Dev\a.exe"),
                &["", r#"say "hi""#, r"dir\"]
            ),
            r#""C:\Program Files\DeskHop Dev\a.exe" "" "say \"hi\"" dir\"#
        );
        assert_eq!(command_line(Path::new("a"), &[r"x y\"]), r#"a "x y\\""#);
        assert_eq!(
            command_line(Path::new("a"), &[r#"x\"y z"#]),
            r#"a "x\\\"y z""#
        );
    }

    #[test]
    fn launch_without_a_user_token_fails_cleanly() {
        // Our own token is a primary token, but CreateProcessAsUserW with it
        // needs privileges a normal user lacks, or it may succeed when
        // elevated; either way it returns rather than panicking.
        let token = crate::token::own_token().unwrap();
        if let Ok(p) = launch_as(
            &token,
            Path::new(r"C:\Windows\System32\whoami.exe"),
            &[],
            Path::new(r"C:\"),
        ) {
            assert_eq!(p.wait().unwrap(), 0);
        }
    }
}
