//! The adopted feeders and everything sent to them: adoption, the commands
//! and the journal every write goes through, feeds, the feeders' clients
//! and the phones' own keys, what an owner administers, debug logs, and
//! each feeder's updates forwarded from the mirror into the hub.

mod admin;
mod clients;
pub(crate) mod commands;
mod diagnostics;
mod feeder;
mod feeds;
pub(crate) mod journal;
mod keys;
mod parity;
pub(crate) mod phones;
mod updates;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use wellfed_api::{Connection as Link, ErrorCode, FeederId, FeederRefusal};
use wellfed_mirror::{InviteText, Unreachable};

pub use clients::ClientError;
pub use commands::CommandError;
pub use diagnostics::LogFollow;
pub use parity::{Method, NoRoute, Parity, parity};

use crate::{Reply, Service, lock};

/// What the service keeps of the feeders between requests.
#[derive(Clone, Default)]
pub(crate) struct State {
    /// Each feeder's link as the stream last told it, so a list never
    /// shows a state the stream has not.
    links: Arc<Mutex<HashMap<FeederId, Link>>>,
    /// How each mirrored feeder's history names the server.
    fingerprints: Arc<Mutex<HashMap<FeederId, [u8; 4]>>>,
    /// Held while a feeder's feeds are settled from its history, so its
    /// watcher and a feed's answer take turns.
    resolving: Arc<Mutex<HashMap<FeederId, Arc<tokio::sync::Mutex<()>>>>>,
    revocations: phones::Revocations,
    adopting: Arc<tokio::sync::Mutex<()>>,
}

impl Service {
    fn links(&self) -> MutexGuard<'_, HashMap<FeederId, Link>> {
        lock(&self.feeders.links)
    }

    /// How the feeder's history names the server, once it is mirrored.
    fn fingerprint(&self, id: FeederId) -> Option<[u8; 4]> {
        lock(&self.feeders.fingerprints).get(&id).copied()
    }
}

/// Adopting feeders and reading what the server holds of them.
#[derive(Debug, thiserror::Error)]
pub enum FeederError {
    #[error("no such feeder")]
    NotFound,
    #[error("that feeder is already adopted")]
    AlreadyAdopted,
    #[error("the feeder was removed: adopt it again to send it anything")]
    Archived,
    #[error(transparent)]
    Invite(#[from] InviteText),
    #[error("the feeder dropped the invite: unknown, used or expired")]
    InviteExpired,
    #[error(transparent)]
    Unreachable(#[from] Unreachable),
    #[error("the feeder refused: {0:?}")]
    Refused(FeederRefusal),
    #[error("the mDNS lookup stopped before it answered")]
    Lookup,
    #[error("stored event {0} does not decode")]
    Undecodable(u64),
    #[error("a stored sample does not decode")]
    UndecodableSample,
    #[error("a feeder key does not open with this server's key")]
    Unseal,
    #[error("the server holds no trace of that feed")]
    NoTrace,
    #[error("the feeder did not answer the log request in time")]
    LogUnanswered,
}

impl FeederError {
    pub(crate) fn reply(&self) -> Reply {
        let code = match self {
            Self::NotFound | Self::NoTrace => ErrorCode::NotFound,
            Self::AlreadyAdopted => ErrorCode::AlreadyAdopted,
            Self::Archived => ErrorCode::FeederArchived,
            Self::Invite(_) => ErrorCode::Invalid,
            Self::InviteExpired => ErrorCode::InviteExpired,
            Self::Unreachable(_) | Self::LogUnanswered => ErrorCode::Unreachable,
            Self::Refused(_) => ErrorCode::FeederRefused,
            Self::Lookup | Self::Undecodable(_) | Self::UndecodableSample | Self::Unseal => {
                return Reply::Internal;
            }
        };
        Reply::Code(code)
    }
}
