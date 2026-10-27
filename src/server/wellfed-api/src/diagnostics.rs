//! What the server reads from a feeder beside its history: each feed's
//! trace, the latest of each statistics section, and the debug log an
//! owner follows live.
//!
//! The traces and statistics are the feeder's own values, served as the
//! model has them; the server adds when it read them. The debug log is
//! diagnostics text, never stored and not a contract.

use jiff::Timestamp;
use wellfed_model::{Stats, StatsSection, Trace};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::FeederRefusal;

/// One feed's trace as the server fetched it after the feed's `Fed`
/// event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FeedTrace {
    /// The history the feed's `Fed` event belongs to.
    pub epoch: u32,
    pub feed: u32,
    /// When the server read it.
    pub fetched: Timestamp,
    pub record: TraceRecord,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TraceRecord {
    Stored {
        trace: Trace,
    },
    /// The feeder had no trace of the feed (its ring had moved on, or a
    /// restart lost it), and said so.
    Missing {
        refusal: FeederRefusal,
    },
}

/// Which history a feed number counts in.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TraceQuery {
    /// The epoch the feed belongs to; the newest one with a trace of that
    /// number when absent.
    pub epoch: Option<u32>,
}

/// The latest read of each statistics section the feeder serves the
/// server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FeederStats {
    pub sections: Vec<StatsRead>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct StatsRead {
    pub section: StatsSection,
    /// When the server read it.
    pub received: Timestamp,
    /// The feeder's boot it was read in, when known: a count "since this
    /// boot" starts again at each.
    pub boot_count: Option<u32>,
    pub stats: Stats,
}

/// One message of a followed debug log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LogEvent {
    /// Record lines the feeder logged, oldest first.
    Lines {
        /// The last line's number this boot.
        seq: u32,
        /// Lines the feeder overwrote before it sent them.
        dropped: u32,
        lines: Vec<String>,
    },
    /// This follower fell behind and missed `batches` batches of lines.
    Lagged { batches: u64 },
}
