//! The feeder's statistics, one section at a time, as a host reads them.

use serde::{Deserialize, Serialize};

/// What a `Stats` request asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatsSection {
    Counters,
    Memory,
    Flash,
    Power,
    Food,
}

impl StatsSection {
    pub const ALL: [Self; 5] = [
        Self::Counters,
        Self::Memory,
        Self::Flash,
        Self::Power,
        Self::Food,
    ];
}

/// One section of the statistics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "section", rename_all = "snake_case")]
pub enum Stats {
    Counters(Counters),
    Memory(Memory),
    Flash(Flash),
    Power(PowerStats),
    Food(Food),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counters {
    /// Since this boot.
    pub boot: Counts,
    /// Why this boot started.
    pub reset: ResetCause,
    /// Over the feeder's life, this boot's included; `None` while the
    /// feeder has not read its store, or cannot.
    pub lifetime: Option<Counts>,
    /// Boots by their reset's cause, over the feeder's life.
    pub resets: Option<Resets>,
    pub handshake_max_ms: u32,
    pub handshake_mean_ms: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub uptime_s: u32,
    pub feeds: u32,
    pub feeds_failed: u32,
    pub portions: u32,
    pub presses: u32,
    pub sessions: u32,
    pub reconnects: u32,
    pub dhcp_renewals: u32,
    pub outages: u32,
    pub battery_s: u32,
}

/// Why a boot started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetCause {
    #[default]
    PowerOn,
    Watchdog,
    Software,
    Brownout,
    Other,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resets {
    pub power_on: u32,
    pub watchdog: u32,
    pub software: u32,
    pub brownout: u32,
    pub other: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Memory {
    pub heap_free: u32,
    pub heap_min: u32,
    pub tasks: Vec<TaskStack>,
    /// The tasks the RTOS runs: more than `tasks` lists when they did not
    /// all fit.
    pub tasks_total: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskStack {
    pub name: String,
    /// The fewest free stack bytes since the task started.
    pub free_min: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flash {
    pub boot: Erases,
    pub lifetime: Option<Erases>,
    pub events_kept: u32,
    pub events_capacity: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Erases {
    /// Each storage sector's erases, by its index in the region.
    pub storage: Vec<u32>,
    /// Bank 1's sectors, erased by updates.
    pub update: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowerStats {
    pub adapter: Option<bool>,
    pub adapter_changes: u32,
    pub outages: u32,
    pub battery_s: u32,
    pub battery_min_mv: Option<u32>,
    pub battery_max_mv: Option<u32>,
    /// The battery at the end of each period, oldest first.
    pub battery_mv: Vec<Option<u16>>,
    pub period_s: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Food {
    pub present: Option<bool>,
    pub emptied: u32,
    pub refilled: u32,
    pub feeds_since_refill: Option<u32>,
}
