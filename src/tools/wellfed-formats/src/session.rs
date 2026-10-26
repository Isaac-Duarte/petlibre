//! The lifecycle of a flash client session's `report.json`: what the flash
//! client writes and the bench, the audit and the page read back. The rest
//! of a report is the flash client's own.

use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Prepared,
    Running,
    /// A read or inspection finished.
    Complete,
    /// A read stopped (the Python reader's name for it).
    Failed,
    /// A write session stopped.
    Stopped,
    /// A write session's transaction verified.
    Verified,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Running => "running",
            Self::Complete => "complete",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
            Self::Verified => "verified",
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How a session reached the helper. Evidence from one never continues the
/// other; Python reports name none and are serial.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    #[default]
    Serial,
    Emulator,
}

/// What a session did, as the Python reader and writer named it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// Receive only, from a running feeder.
    Observe,
    InspectRom,
    ReadSmall,
    ReadFull,
    ReadStatus,
    /// A development update's inspection, preparation and execution.
    Inspect,
    Prepare,
    Execute,
    /// A mode no current session writes. Read back so that the check that
    /// needs a mode refuses it, rather than the whole report failing to
    /// parse; never written.
    #[serde(other, skip_serializing)]
    Other,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::InspectRom => "inspect-rom",
            Self::ReadSmall => "read-small",
            Self::ReadFull => "read-full",
            Self::ReadStatus => "read-status",
            Self::Inspect => "inspect",
            Self::Prepare => "prepare",
            Self::Execute => "execute",
            Self::Other => "other",
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A report as the tools that only follow a session read it. A report
/// whose status is not one of [`Status`] does not parse.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(default)]
pub struct Lifecycle {
    pub status: Option<Status>,
    pub mode: Option<Mode>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_keep_the_python_names() {
        for (mode, name) in [
            (Mode::Observe, "observe"),
            (Mode::InspectRom, "inspect-rom"),
            (Mode::ReadSmall, "read-small"),
            (Mode::ReadFull, "read-full"),
            (Mode::ReadStatus, "read-status"),
            (Mode::Inspect, "inspect"),
            (Mode::Prepare, "prepare"),
            (Mode::Execute, "execute"),
        ] {
            assert_eq!(serde_json::to_value(mode).unwrap(), name);
            assert_eq!(mode.as_str(), name);
            let read: Mode = serde_json::from_value(name.into()).unwrap();
            assert_eq!(read, mode);
        }
        let other: Mode = serde_json::from_value("erase-all".into()).unwrap();
        assert_eq!(other, Mode::Other);
    }

    #[test]
    fn statuses_print_as_they_serialize() {
        for status in [
            Status::Prepared,
            Status::Running,
            Status::Complete,
            Status::Failed,
            Status::Stopped,
            Status::Verified,
        ] {
            assert_eq!(serde_json::to_value(status).unwrap(), status.as_str());
        }
    }
}
