//! Updates over the device API as a host reads them: where an install
//! stands and why one was refused.

use serde::{Deserialize, Serialize};

use crate::Hex;

/// One page of bank 1, as the bank-0 recovery role answers `ReadBank1`:
/// `bytes` from `offset`, of the `total` it serves
/// (`wellfed_client::install::backup::Bank1Reader` asks for every page).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bank1Page {
    pub offset: u32,
    pub total: u32,
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for Bank1Page {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bank1Page")
            .field("offset", &self.offset)
            .field("total", &self.total)
            .field("len", &self.bytes.len())
            .finish()
    }
}

/// Where an update stands. The install does not depend on the session: a
/// client that lost its connection reconnects, reads the state and sends
/// what [`Self::wanted`] names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateView {
    pub phase: UpdatePhase,
    /// The bank this answer comes from.
    pub bank: u8,
    /// The highest confirmed version: an offer at or below it needs a
    /// downgrade statement.
    pub min_version: u32,
    /// The manifest being checked, installed or given way for, if any.
    pub digest: Option<Hex<32>>,
    pub version: u32,
    pub image_len: u32,
    /// Bytes written and read back so far.
    pub written: u32,
    /// The chunk offsets wanted now, in order; empty between sectors or
    /// outside an install.
    pub wanted: Vec<u32>,
    /// Why the last install stopped before activation, if it did.
    pub stopped: Option<UpdateStop>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePhase {
    Idle,
    /// The offer is being checked: ask again.
    Checking,
    /// Bank 1 recorded the offer and restarts into bank 0: reconnect and
    /// offer the same manifest again.
    GivingWay,
    /// Bank 0 is writing bank 1: send the wanted chunks.
    Receiving,
    /// Every byte verified and the marker written: the feeder restarts
    /// into the new build, which confirms itself once healthy.
    Activated,
}

/// Why an install stopped. Bank 1's marker was never written, so the
/// feeder keeps running bank 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateStop {
    /// No chunk came for a minute.
    Timeout,
    Cancelled,
    /// A chunk that was not wanted, or of the wrong length.
    Order,
    /// The image's first bytes are not the loader's marker.
    Marker,
    /// A sector did not read back as written.
    Write,
    /// The whole image's SHA-256 differs from the manifest's.
    Hash,
    /// The marker did not read back.
    Activate,
    /// The update store could not be written.
    Store,
}

/// Why an update op was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateRefusal {
    /// Not a version 2 manifest, or an unknown image kind.
    Manifest,
    /// No trusted key has the manifest's (or a statement's) key id.
    Key,
    /// That key was retired.
    Revoked,
    Signature,
    /// Not for bank 1, or outside the reviewed size.
    Target,
    /// Bank 1 already runs that build.
    Same,
    /// The version is not above the floor (or the image is stock): a
    /// downgrade statement must come with it.
    Downgrade,
    /// The downgrade or key statement does not check out.
    Statement,
    /// Bank 0 installs only the manifest bank 1 gave way for.
    Pending,
    /// This manifest was activated and failed its trial.
    Failed,
    /// Bank 0 holds no build that can install it, so bank 1 does not
    /// give way.
    Fallback,
    /// An install reset the feeder this power cycle.
    Attempt,
    /// Another update is in progress.
    Installing,
    /// No install is receiving chunks.
    NotInstalling,
    /// The update store cannot be read or written.
    Store,
    /// Nobody pressed Dispense in time to confirm the key.
    Unconfirmed,
}
