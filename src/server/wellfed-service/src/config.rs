//! What the server is started with: its key, label, timing and what it
//! keeps.

use std::num::NonZeroU32;
use std::time::Duration;

use crate::{Backups, Diagnostics, Discovery, ReleasesConfig};

pub struct Config {
    /// Seals the server's feeder keys at rest.
    pub key: [u8; 32],
    /// The host's IANA zone, offered at setup as the household's.
    pub host_zone: Option<String>,
    /// The label the server enrolls under on each feeder (1-16 bytes).
    pub label: String,
    pub discovery: Discovery,
    pub timing: Timing,
    /// Whether each feeder's feed traces and statistics are read.
    pub diagnostics: Diagnostics,
    /// The release cache, the keys a release may be signed with, and
    /// whether releases are fetched.
    pub releases: ReleasesConfig,
    /// How many days raw samples are kept; forever when `None`. Hourly
    /// rollups are always kept.
    pub raw_sample_days: Option<NonZeroU32>,
    pub backups: Backups,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("host_zone", &self.host_zone)
            .field("label", &self.label)
            .field("discovery", &self.discovery)
            .field("timing", &self.timing)
            .field("diagnostics", &self.diagnostics)
            .field("releases", &self.releases)
            .field("raw_sample_days", &self.raw_sample_days)
            .field("backups", &self.backups)
            .finish_non_exhaustive()
    }
}

impl Config {
    /// What the mirror needs of it: no key, no label, no logins.
    pub(crate) fn mirror(&self) -> wellfed_mirror::Config {
        let timing = self.timing;
        wellfed_mirror::Config {
            discovery: self.discovery,
            timing: wellfed_mirror::Timing {
                retry_min: timing.retry_min,
                retry_max: timing.retry_max,
                time_sync: timing.time_sync,
                answer: timing.answer,
                sample_wait: timing.sample_wait,
            },
            diagnostics: self.diagnostics,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// The first reconnect delay, doubled after each failure.
    pub retry_min: Duration,
    pub retry_max: Duration,
    /// How often a connected feeder's clock is set again.
    pub time_sync: Duration,
    /// How long a caller waits for a command's answer: the connection's
    /// own answer timeout, and some room to queue behind another command.
    pub answer: Duration,
    /// The longest a received sample waits to be written with others.
    pub sample_wait: Duration,
    /// How long a login lasts after its last use.
    pub session: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        let mirror = wellfed_mirror::Timing::default();
        Self {
            retry_min: mirror.retry_min,
            retry_max: mirror.retry_max,
            time_sync: mirror.time_sync,
            answer: mirror.answer,
            sample_wait: mirror.sample_wait,
            session: Duration::from_secs(30 * 24 * 3600),
        }
    }
}
