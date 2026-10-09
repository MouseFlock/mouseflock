//! Relaying file contents between the session agent and members
//! (add-clipboard-files D4, D5, D7). The service never opens a file: bytes
//! the agent serves go to the member that asked, and bytes a member serves go
//! to the agent pasting them. Neither way waits in the host loop, which also
//! carries input:
//!
//! - **Out** (a member pastes this machine's file): each chunk from the agent
//!   goes to the node's file sink if it has room, or waits here until it
//!   does. The agent's window bounds what waits; a chunk is acknowledged to
//!   the agent only once the node has it.
//! - **In** (the agent pastes a member's file): the node's fetch is read only
//!   while the agent's window has room for a whole chunk, so a slow paste
//!   slows the member rather than filling memory here.

use std::collections::VecDeque;

use ipc::files::Credit;
use model::PeerId;
use tokio::sync::mpsc::OwnedPermit;
use transport::{FileChunk, FileFetch, FileSink};

/// Most file requests from members waiting for the agent at once; more are
/// dropped, and their pastes time out.
pub(super) const MAX_ASKED: usize = 64;

/// The largest chunk the node hands over: one frame.
const CHUNK: u64 = 65_536;

/// The relay's state. Reset whenever the session agent's link changes.
#[derive(Default)]
pub(super) struct Relay {
    /// Members' requests the agent was asked to serve, in order: who, which
    /// copy, which file.
    pub(super) asked: VecDeque<(PeerId, u64, u32)>,
    /// The file the agent is serving now.
    pub(super) out: Option<Out>,
    /// The file the agent is pasting now.
    pub(super) incoming: Option<In>,
    /// The agent's window for the file bytes sent to it, across files.
    pub(super) to_agent: Credit,
}

pub(super) struct Out {
    pub(super) index: u32,
    /// Where its bytes go; `None` once nobody wants them, when they are
    /// acknowledged and dropped.
    pub(super) sink: Option<FileSink>,
    /// Chunks the node had no room for yet, in order.
    pub(super) waiting: VecDeque<Vec<u8>>,
    /// The agent sent the whole file.
    pub(super) ended: bool,
}

pub(super) struct In {
    pub(super) index: u32,
    pub(super) fetch: FileFetch,
}

/// What the relay is ready for.
pub(super) enum Relayed {
    /// The node has room for the next waiting chunk.
    Room(OwnedPermit<Vec<u8>>),
    /// The member the waiting chunks were for is gone.
    Gone,
    /// The next piece of the file being pasted; `None` if the fetch ended.
    Piece(Option<FileChunk>),
}

impl Relay {
    /// Waits for whichever way can move; pends while neither can.
    pub(super) async fn next(&mut self) -> Relayed {
        let room = match &self.out {
            Some(out) if !out.waiting.is_empty() => out.sink.clone(),
            _ => None,
        };
        let can_take = self.to_agent.room() >= CHUNK;
        let fetch = match &mut self.incoming {
            Some(incoming) if can_take => Some(&mut incoming.fetch),
            _ => None,
        };
        tokio::select! {
            reserved = async {
                match room {
                    Some(sink) => sink.reserve_owned().await,
                    None => std::future::pending().await,
                }
            } => match reserved {
                Ok(permit) => Relayed::Room(permit),
                Err(_) => Relayed::Gone,
            },
            piece = async {
                match fetch {
                    Some(fetch) => fetch.recv().await,
                    None => std::future::pending().await,
                }
            } => Relayed::Piece(piece),
        }
    }
}
