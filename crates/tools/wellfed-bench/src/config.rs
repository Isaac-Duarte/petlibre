//! `private/bench.json`: the ports and pinned evidence the bench uses.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::Result;

pub(crate) const CONFIG: &str = "private/bench.json";
pub const STATUS_DIR: &str = "private/bench-status";
pub const CAPTURES: &str = "private/captures";
pub(crate) const FLASH_BACKUPS: &str = "private/flash-backups";
pub(crate) const VERIFICATION: &str = "private/verification";

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
        crate::load(root, Path::new(CONFIG))
    }
}
