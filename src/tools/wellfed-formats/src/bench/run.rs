//! `NAME-itest.json`: a run's record, per-scenario verdicts beside the
//! captures they rest on. The suite writes it when a run ends; the page reads
//! every one back for its run history.

use serde::{Deserialize, Serialize};

/// The record's file name after the run's name, under `private/verification/`.
pub const SUFFIX: &str = "-itest.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RunRecord {
    pub name: String,
    /// Always `physical`: these verdicts come from the feeder on the bench.
    pub evidence: Evidence,
    pub scenarios: Vec<ScenarioRecord>,
    /// Why the suite stopped when the feeder went quiet; it then needs UART recovery.
    pub silent: Option<String>,
    /// Root-relative UART capture directories, in order.
    #[serde(default)]
    pub captures: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum Evidence {
    Physical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ScenarioRecord {
    pub scenario: String,
    pub verdict: ScenarioVerdict,
    pub reason: Option<String>,
    pub seconds: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum ScenarioVerdict {
    Pass,
    Fail,
    /// Not run: a flag it needs was missing, or an earlier failure stopped the run.
    Skip,
}

impl ScenarioVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Skip => "skip",
        }
    }
}
