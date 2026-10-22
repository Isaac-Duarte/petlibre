//! `private/bench.json`: the ports and pinned evidence the bench uses.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{Error, Result};

pub const CONFIG: &str = "private/bench.json";
pub const STATUS_DIR: &str = "private/bench-status";
pub const CAPTURES: &str = "private/captures";
pub const FLASH_BACKUPS: &str = "private/flash-backups";
pub const VERIFICATION: &str = "private/verification";

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub relay_port: String,
    pub uart_port: String,
    /// Root-relative: the pinned full read.
    pub baseline: PathBuf,
    /// Root-relative: the ROM inspection report.
    pub rom_report: PathBuf,
    /// FW30 analyzer channel names; recorded in transient manifests only.
    #[serde(default)]
    pub analyzer_channels: Option<BTreeMap<String, u8>>,
}

impl Config {
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(CONFIG);
        let text = fs::read_to_string(&path)
            .map_err(|error| Error::Bench(format!("{CONFIG}: {error}")))?;
        serde_json::from_str(&text).map_err(|error| Error::Bench(format!("{CONFIG}: {error}")))
    }
}
