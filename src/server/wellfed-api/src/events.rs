//! The feeder's durable history, as the server mirrors it.
//!
//! Event numbers count within an epoch: the feeder's ring draws one when it
//! is created, and a ring re-created by a storage wipe counts from 1 again
//! under a new one. Each epoch is its own history; a page reads one.

use jiff::Timestamp;
use wellfed_model::{Event, EventBody, EventKind};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// One event the feeder recorded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FeederEvent {
    /// The history the event belongs to.
    pub epoch: u32,
    /// The feeder's sequence number: monotonic across its reboots, within
    /// the epoch.
    pub seq: u64,
    /// The feeder's clock when it happened; `None` while that was invalid.
    pub at: Option<Timestamp>,
    /// When the server received it.
    pub received: Timestamp,
    pub body: EventBody,
}

impl FeederEvent {
    /// The feeder's `event`, as the server received it at `received`.
    pub const fn new(event: Event, received: Timestamp) -> Self {
        Self {
            epoch: event.epoch,
            seq: event.seq,
            at: event.at,
            received,
            body: event.body,
        }
    }
}

/// Events the feeder dropped from its ring before the server read them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Gap {
    pub epoch: u32,
    pub from: u64,
    pub to: u64,
    pub received: Timestamp,
}

/// A page of history, oldest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct EventPage {
    /// The history read; `None` before the server holds any.
    pub epoch: Option<u32>,
    pub events: Vec<FeederEvent>,
    /// The gaps that end inside the page's span.
    pub gaps: Vec<Gap>,
    /// The `after` for the next page; `None` at the newest.
    pub next: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct EventQuery {
    /// The history to read; the feeder's current one when absent.
    pub epoch: Option<u32>,
    /// Events after this sequence number; from the oldest when absent.
    pub after: Option<u64>,
    /// At most this many (1-500, default 100).
    pub limit: Option<u32>,
    pub kind: Option<EventKind>,
}
