//! `GET /api/v1/stream`: Server-Sent Events for every feeder the user sees.
//!
//! Each SSE `id` is the server's own position. A client that reconnects
//! sends it back as `Last-Event-ID` and gets what it missed, or a
//! [`StreamEvent::Resync`] when the server no longer holds that far back
//! (or has restarted), after which it refetches.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::FeederId;
use crate::events::{FeederEvent, Gap};
use crate::feeders::Connection;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    Event {
        feeder: FeederId,
        event: FeederEvent,
    },
    Gap {
        feeder: FeederId,
        gap: Gap,
    },
    Sample {
        feeder: FeederId,
        sample: FeederSample,
    },
    Connection {
        feeder: FeederId,
        connection: Connection,
    },
    /// The stream cannot resume from the client's `Last-Event-ID`:
    /// refetch what it shows.
    Resync,
}

/// A live measurement. The feeder sends these only while the server is
/// subscribed and never stores them; the server keeps every one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FeederSample {
    pub received: Timestamp,
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct StreamQuery {
    /// The sample metrics to include, comma-separated (`power,rssi`); none
    /// when absent.
    pub samples: Option<String>,
}
