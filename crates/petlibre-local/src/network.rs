//! Explicit, reversible gateway changes scoped to one test-device MAC.
use std::process::Command;

use anyhow::{Context, Result, ensure};
use serde_json::Value;

use crate::config::Config;

const TABLE: &str = "petlibre_local";
const COMMENT: &str = "petlibre-local-broker";

fn nft(args: &[&str]) -> Result<()> {
    ensure!(
        Command::new("nft").args(args).status()?.success(),
        "nft command failed"
    );
    Ok(())
}

pub fn remove() -> Result<()> {
    let output = Command::new("nft")
        .args(["-j", "list", "table", "inet", "computer_security"])
        .output()?;
    ensure!(
        output.status.success(),
        "cannot read desktop firewall (run network commands with sudo)"
    );
    let data: Value = serde_json::from_slice(&output.stdout)?;
    for item in data["nftables"]
        .as_array()
        .context("invalid nft response")?
    {
        let rule = &item["rule"];
        if rule["comment"] == COMMENT {
            nft(&[
                "delete",
                "rule",
                "inet",
                "computer_security",
                rule["chain"].as_str().context("missing rule chain")?,
                "handle",
                &rule["handle"].to_string(),
            ])?;
        }
    }
    let exists = Command::new("nft")
        .args(["list", "table", "inet", TABLE])
        .output()?;
    if exists.status.success() {
        nft(&["delete", "table", "inet", TABLE])?;
    }
    Ok(())
}

pub fn rules(config: &Config) -> Result<String> {
    config.validate()?;
    ensure!(
        !config.mqtt_listen.ip().is_loopback(),
        "network redirection requires a hotspot broker address"
    );
    let interface = &config.hotspot_interface;
    let mac = &config.target_mac;
    let ip = config.mqtt_listen.ip();
    let port = config.mqtt_listen.port();
    Ok(format!(
        r#"table inet {TABLE} {{
  chain prerouting {{
    type nat hook prerouting priority dstnat - 10; policy accept;
    iifname "{interface}" ether saddr {mac} tcp dport 1883 counter dnat ip to {ip}:{port}
  }}
  chain forward {{
    type filter hook forward priority filter - 20; policy accept;
    iifname "{interface}" ether saddr {mac} meta l4proto tcp counter reject with tcp reset
    iifname "{interface}" ether saddr {mac} counter reject
  }}
}}
insert rule inet computer_security input iifname "{interface}" ether saddr {mac} ip daddr {ip} tcp dport {port} counter accept comment "{COMMENT}"
"#
    ))
}

pub fn apply(config: &Config) -> Result<()> {
    // Fail before changing connectivity if the prepared broker is absent.
    std::net::TcpStream::connect_timeout(&config.mqtt_listen, std::time::Duration::from_secs(2))
        .context("local broker is not listening; start serve first")?;
    let rules = rules(config)?;
    // Check the complete transaction before removing any previous lab rules.
    let existing = Command::new("nft")
        .args(["list", "table", "inet", TABLE])
        .output()?;
    ensure!(
        !existing.status.success(),
        "local routing is already enabled; remove it before applying again"
    );
    use std::io::Write;
    for check in [true, false] {
        let mut command = Command::new("nft");
        if check {
            command.arg("--check");
        }
        let mut child = command
            .args(["-f", "-"])
            .stdin(std::process::Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .context("nft stdin unavailable")?
            .write_all(rules.as_bytes())?;
        ensure!(
            child.wait()?.success(),
            "nft transaction failed; routing was not changed"
        );
    }
    Ok(())
}
