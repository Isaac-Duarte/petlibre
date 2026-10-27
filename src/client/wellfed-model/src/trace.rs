//! One feed's trace as a host reads it: the summary of what the drive
//! measured, and the body's entries decoded, each timed from the drive's
//! start.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{FeedOutcome, FeedReason};

/// What a trace's current readings count in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CurrentUnit {
    /// The current channel as stock scales it: proportional to the motor
    /// current, not yet calibrated to milliamps.
    #[default]
    Scaled,
    MilliAmps,
}

/// What a feed's drive measured, from every poll.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TraceSummary {
    pub feed: u32,
    pub requested: u8,
    pub completed: u8,
    pub outcome: FeedOutcome,
    pub reason: FeedReason,
    pub drive_ms: u32,
    /// Polls that read the current while the motor was driven.
    pub samples: u32,
    pub unit: CurrentUnit,
    pub current_peak: u32,
    pub current_mean: Option<u32>,
    pub rising: u16,
    pub falling: u16,
    pub first_edge_ms: Option<u32>,
    pub beam_breaks: u16,
    pub reversals: u16,
    pub battery_rest_mv: Option<u16>,
    pub battery_min_mv: Option<u16>,
    /// The body keeps every `stride`th poll's current reading.
    pub stride: u16,
    /// The body ends before the drive did.
    pub truncated: bool,
}

/// One page of a trace's body, as the feeder answers `FeedTrace`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TracePage {
    pub summary: TraceSummary,
    /// The trace is in the feeder's storage.
    pub stored: bool,
    pub offset: u16,
    pub total: u16,
    pub bytes: Vec<u8>,
}

/// The motor command as the feeder's controller gave it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TraceMotor {
    Stop,
    /// A single bowl's feeding direction.
    Drive0,
    Drive1,
}

/// One entry of a trace's body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TraceEntry {
    /// The motor current at one poll.
    Current {
        at_ms: u32,
        value: u32,
    },
    /// Feedback edges since the last mark entry; `at_ms` is the last one's.
    Mark {
        at_ms: u32,
        edges: u32,
        high: bool,
    },
    /// The beams as one poll read them, when they changed.
    Beams {
        at_ms: u32,
        outlet_clear: bool,
        food: bool,
    },
    /// The battery pack, when it moved.
    Battery {
        at_ms: u32,
        mv: u32,
    },
    Motor {
        at_ms: u32,
        motor: TraceMotor,
    },
}

/// A whole trace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Trace {
    pub summary: TraceSummary,
    pub stored: bool,
    pub entries: Vec<TraceEntry>,
}

/// Why a trace's body did not read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the trace's body is malformed")]
pub struct Malformed;
