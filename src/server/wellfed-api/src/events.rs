//! The feeder's durable history, as the server mirrors it.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::{Generation, Hex};

/// One event the feeder recorded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FeederEvent {
    /// The feeder's sequence number: monotonic across its reboots.
    pub seq: u64,
    /// The feeder's clock when it happened; `None` while that was invalid.
    pub at: Option<Timestamp>,
    /// When the server received it.
    pub received: Timestamp,
    pub body: EventBody,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventBody {
    Boot {
        bank: u8,
        reset_flags: u32,
    },
    Lock {
        locked: bool,
        by: Source,
    },
    Settings {
        generation: Generation,
        by: Source,
    },
    Wifi {
        generation: Generation,
        by: Source,
    },
    Enrolled {
        /// The new client's fingerprint (its key's first 4 bytes).
        #[schema(value_type = String)]
        client: Hex<4>,
        role: ClientRole,
        path: EnrollPath,
    },
    Revoked {
        #[schema(value_type = String)]
        client: Hex<4>,
        by: Source,
    },
    ClockSync {
        by: Source,
        /// How far the clock moved, when it was valid before.
        jump_ms: Option<i32>,
    },
    /// Events the feeder could not record (its queue overflowed).
    Lost {
        count: u16,
    },
    /// A feed ended.
    Fed {
        /// The feeder's number for the feed.
        feed: u32,
        requested: u8,
        /// The portions the feedback sensor confirmed.
        completed: u8,
        outcome: FeedOutcome,
        reason: FeedReason,
        by: Source,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FeedOutcome {
    Done,
    Cancelled,
    /// It never started: the feeder could not get ready in time.
    Expired,
    Failed,
    /// The feeder stopped without recording a result: anything up to
    /// `requested` may have been dispensed. Never retried automatically.
    Uncertain,
}

/// Why a feed ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FeedReason {
    Done,
    Cancelled,
    /// The feeder's clock jumped.
    Clock,
    /// The feeder could not watch the motor often enough.
    ObservationGap,
    /// The whole feed ran past its limit.
    Timeout,
    /// No portion was confirmed for too long: empty, jammed or a sensor fault.
    NoFeedback,
    Power,
    Sensor,
    /// The outlet beam read blocked.
    Outlet,
    Overcurrent,
    /// The feedback sensor's edges could not be trusted.
    FeedbackStream,
    /// An output failed to apply or verify.
    Output,
}

impl EventBody {
    pub const fn kind(&self) -> EventKind {
        match self {
            Self::Boot { .. } => EventKind::Boot,
            Self::Lock { .. } => EventKind::Lock,
            Self::Settings { .. } => EventKind::Settings,
            Self::Wifi { .. } => EventKind::Wifi,
            Self::Enrolled { .. } => EventKind::Enrolled,
            Self::Revoked { .. } => EventKind::Revoked,
            Self::ClockSync { .. } => EventKind::ClockSync,
            Self::Lost { .. } => EventKind::Lost,
            Self::Fed { .. } => EventKind::Fed,
        }
    }
}

/// An event's kind, for filtering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Boot,
    Lock,
    Settings,
    Wifi,
    Enrolled,
    Revoked,
    ClockSync,
    Lost,
    Fed,
}

impl EventKind {
    /// The stored spelling, the same as the JSON one.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Boot => "boot",
            Self::Lock => "lock",
            Self::Settings => "settings",
            Self::Wifi => "wifi",
            Self::Enrolled => "enrolled",
            Self::Revoked => "revoked",
            Self::ClockSync => "clock_sync",
            Self::Lost => "lost",
            Self::Fed => "fed",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        [
            Self::Boot,
            Self::Lock,
            Self::Settings,
            Self::Wifi,
            Self::Enrolled,
            Self::Revoked,
            Self::ClockSync,
            Self::Lost,
            Self::Fed,
        ]
        .into_iter()
        .find(|kind| kind.code() == code)
    }
}

/// Who caused an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    Panel,
    Client {
        /// The client's fingerprint (its key's first 4 bytes).
        #[schema(value_type = String)]
        client: Hex<4>,
    },
    Feeder,
}

/// A role in the feeder's own client table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClientRole {
    Owner,
    Member,
    Server,
    UpdateSigner,
}

impl ClientRole {
    /// The stored spelling, the same as the JSON one.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Member => "member",
            Self::Server => "server",
            Self::UpdateSigner => "update_signer",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        [Self::Owner, Self::Member, Self::Server, Self::UpdateSigner]
            .into_iter()
            .find(|role| role.code() == code)
    }
}

/// How a client enrolled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnrollPath {
    Setup,
    Invite,
    Recovery,
}

/// Events the feeder dropped from its ring before the server read them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Gap {
    pub from: u64,
    pub to: u64,
    pub received: Timestamp,
}

/// A page of history, oldest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct EventPage {
    pub events: Vec<FeederEvent>,
    /// The gaps that end inside the page's span.
    pub gaps: Vec<Gap>,
    /// The `after` for the next page; `None` at the newest.
    pub next: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct EventQuery {
    /// Events after this sequence number; from the oldest when absent.
    pub after: Option<u64>,
    /// At most this many (1-500, default 100).
    pub limit: Option<u32>,
    pub kind: Option<EventKind>,
}
