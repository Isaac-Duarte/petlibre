//! Why a connection ended, and what the firmware logs for each step
//! (`PLAPI1`). No record carries a key or a secret.

use crate::UpdateKeyChange;
use crate::clients::Role;
use crate::pairing::{self};
use wellfed_core::named_enum;
use wellfed_core::ota::offer::KeyId;
use wellfed_proto::api::Fingerprint;

named_enum! {
    /// Why a connection ended or was refused.
    pub enum Reason {
        /// The client closed it.
        Closed = "closed",
        /// Every slot is taken.
        Full = "full",
        /// A frame longer than the limit, or bytes that do not decode.
        Malformed = "malformed",
        /// A handshake or transport message failed to open.
        Noise = "noise",
        Unknown = "unknown-key",
        Version = "version",
        /// No live invite with that id.
        Invite = "invite",
        /// The recovery window is closed (or not served here yet).
        Recovery = "recovery",
        Timeout = "timeout",
        Revoked = "revoked",
        /// The identity is not loaded, or the generator has no output.
        NotReady = "not-ready",
        /// An invite redemption finished.
        Joined = "joined",
    }
}

/// What the firmware logs (`PLAPI1`). No record carries a key or a secret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Accepted {
        slot: u8,
    },
    Opened {
        slot: u8,
        role: Role,
        client: Fingerprint,
    },
    Request {
        slot: u8,
        op: &'static str,
        result: &'static str,
    },
    Retried {
        slot: u8,
    },
    Ended {
        slot: u8,
        reason: Reason,
    },
    Pairing(pairing::Event),
    /// An update key changed; `key` is its key id (a hash, not the key).
    UpdateKey {
        slot: u8,
        change: UpdateKeyChange,
        key: KeyId,
    },
}
