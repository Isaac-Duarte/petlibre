//! Installs: an owner's request to install one of the server's releases on
//! one feeder, or to restore the feeder's stock firmware from the server's
//! backup, and where it stands. One runs at a time per server.

use jiff::Timestamp;
use wellfed_model::{Hex, UpdateRefusal, UpdateStop};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{FeederId, FeederRefusal, InstallId, UserId};

/// `POST /feeders/{id}/install`: the release to install.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct InstallRequest {
    /// A release's id, as `GET /releases` lists it.
    #[schema(value_type = String)]
    pub release: Hex<32>,
}

/// One install, as it stands. Its progress and its end reach the event
/// stream as `install` updates carrying this body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Install {
    pub id: InstallId,
    pub feeder: FeederId,
    /// What it writes into bank 1.
    pub target: InstallTarget,
    /// The version its manifest offers.
    pub version: u32,
    /// The owner who asked for it.
    pub by: UserId,
    pub started: Timestamp,
    pub state: InstallState,
    /// Bytes bank 0 has written and read back, of `image_len`.
    pub written: u32,
    pub image_len: u32,
    /// Why it failed, once it has.
    pub failure: Option<InstallFailure>,
    /// When it ended.
    pub finished: Option<Timestamp>,
}

/// What an install writes into bank 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InstallTarget {
    /// One of the server's releases.
    Release {
        /// The release's id.
        #[schema(value_type = String)]
        release: Hex<32>,
    },
    /// The feeder's stock firmware, from the backup the server keeps
    /// ("Restore stock"). Once activated, the feeder runs stock, which
    /// serves no device API: the server stops reaching it.
    Stock {
        /// SHA-256 of the kept backup file.
        #[schema(value_type = String)]
        sha256: Hex<32>,
    },
}

impl InstallTarget {
    /// The stored spelling of a release's install.
    pub const RELEASE: &'static str = "release";
    /// The stored spelling of a stock restore.
    pub const STOCK: &'static str = "stock";

    /// The stored spelling of its kind.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Release { .. } => Self::RELEASE,
            Self::Stock { .. } => Self::STOCK,
        }
    }

    /// The bytes that name it: the release's id, or the backup's SHA-256.
    pub const fn name(self) -> Hex<32> {
        match self {
            Self::Release { release } => release,
            Self::Stock { sha256 } => sha256,
        }
    }

    /// The target a stored kind and name spell; `None` for a kind this
    /// build does not know.
    pub fn from_code(code: &str, name: Hex<32>) -> Option<Self> {
        match code {
            Self::RELEASE => Some(Self::Release { release: name }),
            Self::STOCK => Some(Self::Stock { sha256: name }),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum InstallState {
    /// The server is pushing the release, or waiting for the feeder to
    /// restart into bank 0 to take it.
    Running,
    /// Every byte is verified and bank 1 activated: the feeder restarts
    /// into the release, whose trial confirms it, or into stock.
    Activated,
    /// Bank 1 already ran the release's build when the install began, so
    /// nothing was offered and nothing written.
    AlreadyRunning,
    /// It ended before activation; `failure` says why. The feeder keeps
    /// running the build it had.
    Failed,
}

impl InstallState {
    /// The stored spelling, the same as the JSON one.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Activated => "activated",
            Self::AlreadyRunning => "already_running",
            Self::Failed => "failed",
        }
    }

    /// The state a stored spelling names; `None` for one this build does
    /// not know.
    pub fn from_code(code: &str) -> Option<Self> {
        [
            Self::Running,
            Self::Activated,
            Self::AlreadyRunning,
            Self::Failed,
        ]
        .into_iter()
        .find(|state| state.code() == code)
    }
}

/// Why an install failed. Nothing is offered again on the owner's behalf:
/// another install is another request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum InstallFailure {
    /// The feeder refused an update request; `refusal` says why. (`same`
    /// answers an offer of the build bank 1 runs, which the server never
    /// sends: that install ends `already_running` instead.)
    UpdateRefused { refusal: UpdateRefusal },
    /// The feeder refused for a reason not particular to updates
    /// (`forbidden`: the server's key is not an owner's on this feeder).
    Refused { refusal: FeederRefusal },
    /// The install stopped on the feeder before activation.
    Stopped { stop: UpdateStop },
    /// Bank 0 is no longer installing this release.
    Lost,
    /// The feeder answered something that does not fit an update.
    Unexpected,
    /// The feeder stopped answering. With `offered`, the offer went out,
    /// so bank 1 may be partly written, or activated with its answer lost:
    /// the feeder's status tells.
    Unreachable { offered: bool },
    /// The server failed (its random source); its log says why.
    Server,
    /// The server stopped while it ran. The feeder stops a transfer that
    /// gets no chunk for a minute, and another install resumes one still
    /// receiving this release.
    Interrupted,
}
