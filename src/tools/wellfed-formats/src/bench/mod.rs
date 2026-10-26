//! The relay bench's files: its config, the evidence layout of a session,
//! the progress document its page follows, and the integration suite's run
//! records and catalog.
//!
//! The bench and the suite write these; the page (`wellfed-dashboard`) only
//! reads them, and links neither the relay nor the flash client, so the
//! shapes they share live here.

pub mod cancel;
pub mod catalog;
pub mod progress;
pub mod run;

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

pub use cancel::Cancel;

/// Root-relative: the bench config.
pub const CONFIG: &str = "private/bench.json";
/// Root-relative: the progress document's directory ([`progress`]).
pub const STATUS_DIR: &str = "private/bench-status";
pub const CAPTURES: &str = "private/captures";
pub const FLASH_BACKUPS: &str = "private/flash-backups";
pub const VERIFICATION: &str = "private/verification";

/// `private/bench.json`: the ports and pinned evidence the bench uses.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub relay_port: String,
    pub uart_port: String,
    /// Root-relative: the pinned full read.
    pub baseline: PathBuf,
    /// Root-relative: the ROM inspection report.
    pub rom_report: PathBuf,
    /// Analyzer channel names; recorded in transient manifests only.
    #[serde(default)]
    pub analyzer_channels: Option<BTreeMap<String, u8>>,
}

/// Evidence locations for one session name.
#[derive(Debug, Clone)]
pub struct Paths {
    pub loader: PathBuf,
    pub bounded: PathBuf,
    pub full: PathBuf,
    pub inspect: PathBuf,
    pub plan: PathBuf,
    pub execute: PathBuf,
    pub boot: PathBuf,
}

impl Paths {
    pub fn new(root: &Path, name: &str) -> Self {
        Self {
            loader: root.join(CAPTURES).join(format!("{name}-loader")),
            bounded: root.join(FLASH_BACKUPS).join(format!("{name}-bounded")),
            full: root.join(FLASH_BACKUPS).join(format!("{name}-full")),
            inspect: root.join(VERIFICATION).join(format!("{name}-inspect")),
            plan: root.join(VERIFICATION).join(format!("{name}-plan.json")),
            execute: root.join(VERIFICATION).join(format!("{name}-execute")),
            boot: root.join(CAPTURES).join(format!("{name}-boot")),
        }
    }
}

/// A session name is one plain path component, so its evidence stays
/// inside the directory it is named under.
pub fn plain_name(name: &str) -> bool {
    let mut components = Path::new(name).components();
    matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
        && !name.contains(['/', '\\'])
}

/// `path` under `root`, as the progress document and the page record it.
pub fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Seconds since the Unix epoch, as the progress document records times.
pub fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}
