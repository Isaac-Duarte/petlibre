//! The command journal: every write the server sends a feeder, recorded
//! before it is sent and kept forever.
//!
//! An entry is found by the `Idempotency-Key` it was sent under. A retry
//! with the same key reads the entry first: an answered write returns its
//! recorded outcome without reaching the feeder, and one whose outcome is
//! unknown is sent again under the same device request id. A key reused
//! for another request is refused `key_reused`.

use jiff::Timestamp;
use wellfed_model::Settings;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{FeederId, FeederRefusal, UserId};

/// What a journalled write asked of the feeder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandKind {
    /// A settings change.
    Settings,
    /// Locking or unlocking the panel.
    Lock,
}

impl CommandKind {
    /// The stored spelling.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Settings => "settings",
            Self::Lock => "lock",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "settings" => Some(Self::Settings),
            "lock" => Some(Self::Lock),
            _ => None,
        }
    }
}

/// Where a journalled write stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandState {
    /// Recorded and handed to the feeder; no answer yet.
    Sent,
    /// The feeder answered; `outcome` is its answer.
    Answered,
    /// No answer came, or the server stopped before one did. The feeder
    /// may have run it: only a retry under the same key may send it again.
    Unknown,
    /// An uncertain write settled from the feeder's history.
    Resolved,
}

impl CommandState {
    /// The stored spelling.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Sent => "sent",
            Self::Answered => "answered",
            Self::Unknown => "unknown",
            Self::Resolved => "resolved",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "sent" => Some(Self::Sent),
            "answered" => Some(Self::Answered),
            "unknown" => Some(Self::Unknown),
            "resolved" => Some(Self::Resolved),
            _ => None,
        }
    }
}

/// What the feeder answered a write.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum CommandOutcome {
    /// It took effect.
    Done,
    /// A settings change took effect, and these are the settings the
    /// server read back after it.
    Settings { settings: Settings },
    /// The feeder refused it; nothing ran.
    Refused { refusal: FeederRefusal },
}

/// One journalled write.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Command {
    /// The `Idempotency-Key` it was sent under; the server draws one for
    /// a write sent without.
    #[schema(value_type = String, format = Uuid)]
    pub key: Uuid,
    /// Who sent it.
    pub user: UserId,
    pub feeder: FeederId,
    pub kind: CommandKind,
    pub state: CommandState,
    /// The feeder's answer, once `answered`.
    pub outcome: Option<CommandOutcome>,
    /// A feed's number in the feeder's journal.
    pub feed: Option<u32>,
    /// The feeder's boot count when it was sent, as the server last read
    /// it; `None` before the server read the feeder's status.
    pub boot_count: Option<u32>,
    /// When it was recorded, before it was sent.
    pub sent: Timestamp,
    /// When its state last changed.
    pub updated: Timestamp,
}

/// A page of the journal, oldest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CommandPage {
    pub items: Vec<Command>,
    /// The `after` for the next page; `None` at the end.
    pub next: Option<String>,
}

/// Which entries to list. A member sees their own; an owner, everyone's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CommandQuery {
    /// One feeder's writes only.
    #[param(value_type = Option<String>, format = Uuid)]
    pub feeder: Option<FeederId>,
    /// Writes in this state only: `unknown` is the uncertain list.
    pub state: Option<CommandState>,
    /// The previous page's `next`, as it came.
    pub after: Option<String>,
    /// At most this many (1-1000, default 100).
    pub limit: Option<u32>,
}
