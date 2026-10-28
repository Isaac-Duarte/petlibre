//! The meal call: one clip the owner uploads, which a plan plays before
//! its meal ([`crate::Plan::call`]).

use serde::{Deserialize, Serialize};

pub use wellfed_proto::api::{CALL_BYTES_MAX, CALL_CHUNK_MAX, CALL_PLAYS_MAX, CALL_RATE_HZ};

/// The meal call as the feeder stores it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallView {
    /// Whether the store can take a write now.
    pub writable: bool,
    pub clip: CallClip,
}

/// The stored clip, or the upload under way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum CallClip {
    None,
    /// An upload of `bytes` stream bytes with CRC `crc` has `received` of
    /// them; a restart of the feeder forgets it.
    Receiving {
        bytes: u32,
        crc: u32,
        received: u32,
    },
    /// A clip of `bytes` stream bytes, [`CallClip::ms`] a play.
    Stored {
        bytes: u32,
        crc: u32,
    },
}

impl CallClip {
    /// How long the stored clip plays once, in milliseconds.
    pub const fn ms(self) -> Option<u32> {
        match self {
            Self::Stored { bytes, .. } => Some(bytes.div_ceil(4)),
            Self::None | Self::Receiving { .. } => None,
        }
    }
}
