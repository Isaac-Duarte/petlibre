//! What the feeding owner saw at one poll of a feed, for a trace.
//!
//! The owner takes one [`Observation`] per poll while a feed is prepared or
//! driven and hands it to the task that steps it once the step is over, so
//! whatever keeps the trace runs after the poll's motor and stop decisions,
//! never inside them. Every figure is one the poll read anyway.

use super::controller::MotorCommand;

/// One poll of a feed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Observation {
    /// The supervisor's clock at the poll's acquisition.
    pub at_ms: u32,
    /// The supervisor's clock when the drive started; `None` while the feed
    /// is still being prepared.
    pub drive_at_ms: Option<u32>,
    /// The current channel in stock's scaled unit, when the poll read it.
    pub current: Option<u32>,
    /// The battery pack in millivolts; `None` with no pack fitted or no
    /// reading.
    pub battery_mv: Option<u32>,
    /// The beams, read only while the emitters are on: the outlet beam
    /// clear, and food in the food beam.
    pub beams: Option<Beams>,
    /// Feedback edges since capture opened for the drive, each way, and the
    /// last one's time and the level after it.
    pub rising: u16,
    pub falling: u16,
    pub last_edge: Option<(u32, bool)>,
    /// The motor command the controller holds after this poll.
    pub motor: MotorCommand,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Beams {
    pub outlet_clear: bool,
    pub food: bool,
}
