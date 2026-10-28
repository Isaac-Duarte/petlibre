//! The command journal: every write the server sends a feeder, recorded
//! before it is sent and kept forever.
//!
//! An entry is found by the `Idempotency-Key` it was sent under. A retry
//! with the same key reads the entry first: an answered write returns its
//! recorded outcome without reaching the feeder, and one whose outcome is
//! unknown is sent again under the same device request id when its kind
//! allows (a feed never is). A key reused for another request is refused
//! `key_reused`.
//!
//! A feed is answered once the feeder has reserved it (`feeding`, with its
//! number), and resolved from the feeder's history: its `Fed` event, or,
//! when the feeder restarted and holds no record of it, `unrecorded`.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;
use wellfed_model::Settings;

use crate::{FeedOutcome, FeedReason, FeederId, FeederRefusal, TokenId, UserId};

/// What a journalled write asked of the feeder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandKind {
    /// A settings change.
    Settings,
    /// Locking or unlocking the panel.
    Lock,
    /// A feed. One whose outcome is unknown is never sent again: the
    /// feeder's memory of request ids does not outlive a reboot, so an
    /// uncertain feed is resolved from its history instead.
    Feed,
    /// Stopping a feed that runs.
    Cancel,
    /// An invite for the caller's own phone.
    Invite,
    /// Revoking one of the feeder's clients.
    Revoke,
    /// The feeder's Wi-Fi credentials.
    Wifi,
    /// Restarting the feeder.
    Reboot,
    /// Playing a tune.
    Play,
    /// Revoking the server's own key as the feeder is removed.
    Remove,
}

impl CommandKind {
    /// The stored spelling.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Settings => "settings",
            Self::Lock => "lock",
            Self::Feed => "feed",
            Self::Cancel => "cancel",
            Self::Invite => "invite",
            Self::Revoke => "revoke",
            Self::Wifi => "wifi",
            Self::Reboot => "reboot",
            Self::Play => "play",
            Self::Remove => "remove",
        }
    }

    /// Every kind, in the order they were added.
    pub const ALL: [Self; 10] = [
        Self::Settings,
        Self::Lock,
        Self::Feed,
        Self::Cancel,
        Self::Invite,
        Self::Revoke,
        Self::Wifi,
        Self::Reboot,
        Self::Play,
        Self::Remove,
    ];

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.code() == code)
    }
}

/// Where a journalled write stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandState {
    /// Recorded and handed to the feeder; no answer yet. A retry while it
    /// is in flight is answered unknown and sends nothing.
    Sent,
    /// The feeder answered; `outcome` is its answer.
    Answered,
    /// No answer came, or the server stopped before one did. The feeder
    /// may have run it: only a retry under the same key may send it again.
    Unknown,
    /// Settled from the feeder's history: a feed by its `Fed` event, or as
    /// `unrecorded` once the feeder restarted with no record of it. Final,
    /// except that a `Fed` event found after `unrecorded` replaces it.
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

/// What the feeder answered a write, or what its history says of it.
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
    /// The feeder made the invite under this number, for this many
    /// seconds. Its secret went to the caller alone and is not kept.
    Invited { invite: u32, seconds: u16 },
    /// The feeder reserved the feed under this number and runs it; its
    /// result follows as `fed`.
    Feeding { feed: u32 },
    /// The feed ended, as its `Fed` event in the feeder's history says.
    /// An `uncertain` outcome may have dispensed anything up to
    /// `requested`, and is never retried.
    Fed {
        feed: u32,
        requested: u8,
        /// The portions the feeder confirmed.
        completed: u8,
        outcome: FeedOutcome,
        reason: FeedReason,
    },
    /// The feeder restarted since the feed was sent, and its history holds
    /// no record of it: it never ran, or its record was lost. Whether food
    /// was dispensed is not known, and it is never sent again.
    Unrecorded,
}

impl CommandOutcome {
    /// The feeder's number for the feed this outcome names.
    pub const fn feed(&self) -> Option<u32> {
        match self {
            Self::Feeding { feed } | Self::Fed { feed, .. } => Some(*feed),
            Self::Done
            | Self::Settings { .. }
            | Self::Refused { .. }
            | Self::Invited { .. }
            | Self::Unrecorded => None,
        }
    }
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
    /// The token it came through; `None` for a browser session.
    pub token: Option<TokenId>,
    pub feeder: FeederId,
    pub kind: CommandKind,
    pub state: CommandState,
    /// The feeder's answer once `answered`; its history's once `resolved`.
    pub outcome: Option<CommandOutcome>,
    /// A feed's number in the feeder's journal, once it is known.
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

/// A feed to send.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FeedRequest {
    /// Portions to dispense (1-48; the feeder checks its own cap).
    pub portions: u8,
}
