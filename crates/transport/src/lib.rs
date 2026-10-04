//! QUIC transport and mDNS discovery. Datagrams for motion, streams for the rest.
//!
//! Runs `pairing`'s state machines on real connections: identity certificates
//! pinned by key, member and pairing connections, liveness, reconnecting, and
//! finding peers by mDNS or by a pairing code's locator (ADR 0004, Amendment
//! 2).

#![forbid(unsafe_code)]

mod conn;
pub mod discovery;
pub mod endpoint;
mod forward;
pub mod motion;
pub mod net;
mod node;
pub mod tls;

pub use endpoint::{IDLE_TIMEOUT, KEEPALIVE, PORT, StartError};
pub use node::{
    Backoff, Config, Connection, DIAL_TIMEOUT, Down, Event, MAX_STRANGERS, NoNetwork, NodeHandle,
    STRANGER_TIMEOUT, records_done_frame, start,
};
