//! The replacement firmware's health message, as far as the bench checks it
//! and the page shows it.
//!
//! The message is the firmware's own JSON (`wellfed-core`'s transport writes
//! it), which `wellfed-local` relays unchanged inside its status document
//! ([`crate::local::LocalStatus`]). Only the fields the bench reads are typed;
//! each defaults when absent, so a firmware that adds fields still reads, and
//! a missing one fails the check that needs it rather than the whole document.

use serde::{Deserialize, Serialize};

/// The firmware's health message.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct Health {
    /// The startup identity, hex in either case.
    pub build: String,
    pub bank: Option<u64>,
    pub seq: Option<u64>,
    pub outputs: Option<String>,
    pub faults: Vec<String>,
    /// Absent from builds without the wall clock.
    pub clock: Option<Clock>,
    pub wifi: Wifi,
    pub transport: Transport,
    /// Only the bank-0 recovery role reports one, while it waits for an
    /// update.
    pub role: Option<HealthRole>,
}

/// Who reports health, when it is not the normal role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum HealthRole {
    Recovery,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct Clock {
    pub syncs: u64,
    /// Absent until the clock is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unix_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct Wifi {
    /// Which credentials joined this boot: `build`, `stored` or `fallback`.
    pub source: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct Transport {
    pub connects: u64,
}
