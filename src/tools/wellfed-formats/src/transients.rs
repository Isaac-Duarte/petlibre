//! A driver-transient session's `manifest.json`: the bench writes it
//! after every power cycle, `wellfed-audit transients` reads it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransientMode {
    Boot,
    Loader,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransientsManifest {
    pub schema_version: u32,
    pub mode: TransientMode,
    pub samplerate: u64,
    pub seconds: f64,
    pub arming_seconds: f64,
    /// Logic-analyzer channel of each driver line (INA, INB, OUTA, OUTB).
    pub channels: BTreeMap<String, u64>,
    pub cycles: Vec<TransientCycle>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransientCycle {
    pub file: String,
    pub bytes: u64,
    pub power_on_after_launch_s: f64,
}
