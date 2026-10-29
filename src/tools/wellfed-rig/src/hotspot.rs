//! The bench hotspot: the NetworkManager connection that serves the
//! feeder's network, named in `private/hotspot.json`. Reading whether it is
//! up only lists the active connections; bringing it up or down is the
//! suite's, under the owner's standing authorization.

use std::process::{Command, Stdio};

use serde::Deserialize;

/// Root-relative: the hotspot's file.
pub const HOTSPOT: &str = "private/hotspot.json";

/// `private/hotspot.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct HotspotFile {
    /// The NetworkManager connection that serves the feeder's network.
    pub connection: String,
}

#[derive(Debug, thiserror::Error)]
pub enum HotspotError {
    #[error("nmcli: {0}")]
    Nmcli(std::io::Error),
    /// `stderr` as nmcli printed it, trimmed.
    #[error("nmcli could not list connections: {stderr}")]
    List { stderr: String },
}

/// Whether `connection` is among NetworkManager's active connections.
pub fn active(connection: &str) -> Result<bool, HotspotError> {
    let output = Command::new("nmcli")
        .args(["-t", "-f", "NAME", "connection", "show", "--active"])
        .stdin(Stdio::null())
        .output()
        .map_err(HotspotError::Nmcli)?;
    if !output.status.success() {
        return Err(HotspotError::List {
            stderr: String::from_utf8_lossy(&output.stderr).trim().into(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line == connection))
}
