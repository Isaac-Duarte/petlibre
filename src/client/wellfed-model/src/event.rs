//! The feeder's durable history and its live samples, as a session
//! receives them.
//!
//! Event numbers count within an epoch: the feeder's ring draws one when it
//! is created, and a ring re-created by a storage wipe counts from 1 again
//! under a new one. Each epoch is its own history.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{Generation, Hex};

/// What the feeder pushes to a subscribed session, or to one that asked
/// for its log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pushed {
    Event(Event),
    Gap(Gap),
    Sample(Sample),
    Log(LogLines),
}

/// Record lines the feeder logged, as `TailLog` streams them. The text is
/// diagnostics, not a contract: any build may change it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLines {
    /// The last line's number this boot: the cursor to resume after.
    pub seq: u32,
    /// Lines after the previous cursor the feeder overwrote before sending
    /// them.
    pub dropped: u32,
    /// The lines, without their endings.
    pub lines: Vec<String>,
}

/// One event the feeder recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// The history the event belongs to.
    pub epoch: u32,
    /// The feeder's sequence number: monotonic across its reboots, within
    /// the epoch.
    pub seq: u64,
    /// The feeder's clock when it happened; `None` while that was invalid.
    pub at: Option<Timestamp>,
    pub body: EventBody,
}

/// Events `from..=to` of `epoch` left the feeder's ring before this session
/// read them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gap {
    pub epoch: u32,
    pub from: u64,
    pub to: u64,
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
    /// When a power cut lost the reservation itself, `requested` is the
    /// feeder's cap on one feed, not what was asked.
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

/// A live measurement, sent only to subscribers and never stored on the
/// feeder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sample {
    /// The feeder's uptime when it measured.
    pub uptime_ms: u32,
    pub value: SampleValue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "metric", rename_all = "snake_case")]
pub enum SampleValue {
    Power {
        adapter: bool,
        battery_mv: Option<u32>,
    },
    Rssi {
        dbm: i32,
    },
}

impl SampleValue {
    pub const fn metric(&self) -> MetricKind {
        match self {
            Self::Power { .. } => MetricKind::Power,
            Self::Rssi { .. } => MetricKind::Rssi,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MetricKind {
    Power,
    Rssi,
}

impl MetricKind {
    /// The stored and query spelling, the same as the JSON one.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Power => "power",
            Self::Rssi => "rssi",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        [Self::Power, Self::Rssi]
            .into_iter()
            .find(|metric| metric.code() == code)
    }
}
