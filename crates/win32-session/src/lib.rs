//! Sessions, tokens, launching agents, pipe ACLs, the data folder, `SendSAS`.
//!
//! One of four crates where `unsafe` is allowed (ADR 0006). Every `unsafe`
//! block carries a `// SAFETY:` comment. Linked only by `bin/service`, which
//! stays `#![forbid(unsafe_code)]`. Nothing here touches input.
//!
//! Each Win32 call sits behind a small safe function:
//!
//! | Function | For |
//! |---|---|
//! | [`active_console_session`], [`user_token`], [`Token::user_sid`], [`own_sid`] | which session and user get an agent |
//! | [`system_token`], [`Token::privileges`] | the winlogon agent's SYSTEM token, privileges removed |
//! | [`launch_as`], [`Desktop`], [`Process`] | starting an agent as its user, on its desktop |
//! | [`create_agent_pipe`], [`create_ui_pipe`], [`client_process_id`], [`client_session_id`] | the agent's and the UI's pipes, and who connected |
//! | [`secure_folder`], [`create_admin_readable`], [`remove_secure_folder`] | the service's data folder, its log, and removing it on uninstall |
//! | [`sas_policy`], [`send_sas`] | the secure attention screen |
//!
//! Calls that need SYSTEM (`user_token`, `system_token`, `launch_as` with
//! either token, `secure_folder`) fail with an error, never a panic, when
//! run as anyone else.

mod folder;
mod handle;
mod pipe;
mod process;
mod sas;
mod sid;
mod token;

pub use folder::{Prepared, Removal, create_admin_readable, remove_secure_folder, secure_folder};
pub use pipe::{
    MAX_UI_INSTANCES, UiAdmit, agent_pipe_sddl, client_process_id, client_session_id,
    create_agent_pipe, create_ui_pipe, own_session, ui_pipe_sddl,
};
pub use process::{Desktop, Process, command_line, launch_as};
pub use sas::{SAS_POLICY_KEY, SAS_POLICY_VALUE, allows_services, sas_policy, send_sas};
pub use sid::{ADMINISTRATORS, GUESTS, SYSTEM, Sid, is_trusted_owner};
pub use token::{
    SessionId, Token, TokenError, active_console_session, own_sid, system_token, user_token,
};
