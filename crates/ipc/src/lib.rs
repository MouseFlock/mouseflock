//! Typed named-pipe protocol between service, agents, and UI.
//!
//! Service, agents, and UI can be mid-upgrade at different versions. A change
//! here bumps [`IPC_VERSION`] and updates `docs/protocol.md` in the same PR.
//!
//! - [`frame`]: `len: u32le | type: u16le | payload`, at most 65,536 bytes.
//! - [`message`]: `Hello` with role and version, and the agent link's
//!   messages, with their directions.
//! - [`clock`]: aligning the agent's clock with the service's.
//! - [`ui`]: the UI link's messages: commands from a UI, and the service's
//!   status and notices.
//! - [`link`]: both ends of the agent link and of the UI link as async
//!   connections over any byte stream.
//!
//! The service trusts nothing an agent sends. Decoding never panics and never
//! allocates from a length the other end claims; the lints below enforce it.

#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::arithmetic_side_effects
    )
)]

pub mod clipboard;
pub mod clock;
pub mod error;
pub mod frame;
pub mod link;
pub mod message;
pub mod ui;

pub use error::{EncodeError, ProtocolError};

/// Highest IPC version this release speaks, exchanged in `Hello`.
pub const IPC_VERSION: u16 = 2;

/// Lowest IPC version this release speaks.
pub const MIN_IPC_VERSION: u16 = 1;
