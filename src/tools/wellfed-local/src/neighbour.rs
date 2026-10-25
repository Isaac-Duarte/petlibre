//! Finding the feeder on the hotspot: its MAC in the interface's neighbour
//! table, so the image server can admit that one peer without an address
//! from the operator.

use std::net::IpAddr;
use std::process::Command;

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

use wellfed_stock::config::Config;

/// One `ip -j neigh` entry; incomplete entries have no `lladdr`.
#[derive(Deserialize)]
struct Neighbour {
    dst: String,
    lladdr: Option<String>,
}

/// The feeder's address on the hotspot interface.
pub fn discover(config: &Config) -> Result<IpAddr> {
    let output = Command::new("ip")
        .args(["-j", "neigh", "show", "dev", &config.hotspot_interface])
        .output()
        .context("run ip neigh")?;
    ensure!(output.status.success(), "ip neigh failed");

    let neighbours: Vec<Neighbour> = serde_json::from_slice(&output.stdout)?;
    neighbours
        .into_iter()
        .filter(|entry| {
            entry
                .lladdr
                .as_deref()
                .is_some_and(|lladdr| lladdr.eq_ignore_ascii_case(&config.target_mac))
        })
        .find_map(|entry| entry.dst.parse::<IpAddr>().ok())
        .with_context(|| {
            format!(
                "feeder not in {}'s neighbour table; is it joined? Pass --peer",
                config.hotspot_interface
            )
        })
}
