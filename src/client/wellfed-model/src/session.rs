//! Opening a session: what a client claims, what the feeder answers, and
//! how an invite redemption ends.

use serde::{Deserialize, Serialize};

use crate::{ClientRole, Refusal};

/// The features a session runs on, beyond what major version 1 first
/// defined. Each is one bit on the wire; a session runs on the ones both
/// sides claim, so a client adapts to whatever feeder it meets, however
/// old.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// Feeding: the `Feed` and `CancelFeed` ops, the `Feeding` answer and
    /// `Fed` events.
    pub feed: bool,
    /// Updates: the update ops, the `Update` answer and refusals.
    pub update: bool,
    /// The time settings and the clock's state: `GetTime`, `PutTime` and
    /// the `Time` answer.
    pub time: bool,
}

impl Capabilities {
    /// Version 1 as first defined: none of the later features.
    pub const V1: Self = Self {
        feed: false,
        update: false,
        time: false,
    };
    /// Every feature this crate knows: what a current client claims.
    pub const KNOWN: Self = Self {
        feed: true,
        update: true,
        time: true,
    };

    /// The features both `self` and `other` claim.
    #[must_use]
    pub const fn both(self, other: Self) -> Self {
        Self {
            feed: self.feed && other.feed,
            update: self.update && other.update,
            time: self.time && other.time,
        }
    }
}

/// What a client says about itself when it opens a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hello {
    /// The protocol's major version.
    pub major: u16,
    pub capabilities: Capabilities,
}

impl Hello {
    /// This crate's protocol, claiming every feature it knows.
    pub const CURRENT: Self = Self {
        major: wellfed_proto::api::PROTOCOL_MAJOR,
        capabilities: Capabilities::KNOWN,
    };
}

/// Why the feeder refused to open a session, so a client can say which
/// side needs what.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenRefusal {
    /// The key is not enrolled (or was revoked).
    Unknown,
    /// The client's major version is one this feeder does not serve;
    /// `supported` is the newest it does.
    Version { supported: u16 },
    /// Every session slot is taken.
    Full,
}

/// How an invite redemption ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Joined {
    /// The new key is enrolled with this role; it opens sessions now.
    Enrolled {
        role: ClientRole,
    },
    Refused(Refusal),
}
