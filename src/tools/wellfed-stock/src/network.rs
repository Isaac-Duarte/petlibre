//! Explicit, reversible gateway changes scoped to one test-device MAC.
//!
//! The host accepts two ports from that MAC only: the broker, and the
//! `ota-images` port stock downloads an update from. Both input rules carry
//! the same tag, so `remove` deletes them together.
//!
//! The broker redirect is one rule in its own chain, and it can be left out
//! ([`Redirect::Off`]): the feeder then reaches the broker only by the name
//! it resolves, as a consumer's DNS-first install has it, while its WAN stays
//! blocked. A failed DNS override then fails closed: stock cannot reach the
//! vendor's cloud through a public resolver's answer.
//!
//! Linux gateway only, run as root through `nft`. `rules` renders the
//! transaction for preview; `apply` refuses unless the broker is listening and
//! checks the complete transaction before committing it; `remove` deletes only
//! Wellfed's own table and its tagged input rule.
use std::process::Command;

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

use crate::config::Config;

// The names are what `apply` has already installed on bench hosts, under
// the server's earlier name, so `remove` must keep matching them exactly.
const TABLE: &str = "wellfed_local";
const COMMENT: &str = "wellfed-local-broker";

/// `nft -j list table` output: a list of objects, each holding one of
/// metainfo, table, chain or rule. Only rules are read.
#[derive(Deserialize)]
struct Ruleset {
    nftables: Vec<Item>,
}

#[derive(Deserialize)]
struct Item {
    rule: Option<Rule>,
}

#[derive(Deserialize)]
struct Rule {
    chain: String,
    handle: u64,
    comment: Option<String>,
}

/// Whether the feeder's TCP 1883 is redirected to the broker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Redirect {
    /// Whatever address stock connects to on 1883 reaches the broker.
    Broker,
    /// No redirect: stock reaches the broker only if its name resolves
    /// here. Its WAN stays blocked and the input accepts stay.
    Off,
}

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

    let data: Ruleset = serde_json::from_slice(&output.stdout).context("invalid nft response")?;
    for rule in data.nftables.into_iter().filter_map(|item| item.rule) {
        if rule.comment.as_deref() == Some(COMMENT) {
            nft(&[
                "delete",
                "rule",
                "inet",
                "computer_security",
                &rule.chain,
                "handle",
                &rule.handle.to_string(),
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

pub fn rules(config: &Config, redirect: Redirect) -> Result<String> {
    config.validate()?;
    ensure!(
        !config.mqtt_listen.ip().is_loopback(),
        "network redirection requires a hotspot broker address"
    );

    let (interface, mac) = config.hotspot()?;
    let ip = config.mqtt_listen.ip();
    let port = config.mqtt_listen.port();
    let prerouting = match redirect {
        Redirect::Broker => format!(
            r#"  chain prerouting {{
    type nat hook prerouting priority dstnat - 10; policy accept;
    iifname "{interface}" ether saddr {mac} tcp dport 1883 counter dnat ip to {ip}:{port}
  }}
"#
        ),
        Redirect::Off => String::new(),
    };

    Ok(format!(
        r#"table inet {TABLE} {{
{prerouting}  chain forward {{
    type filter hook forward priority filter - 20; policy accept;
    iifname "{interface}" ether saddr {mac} meta l4proto tcp counter reject with tcp reset
    iifname "{interface}" ether saddr {mac} counter reject
  }}
}}
insert rule inet computer_security input iifname "{interface}" ether saddr {mac} ip daddr {ip} tcp dport {port} counter accept comment "{COMMENT}"
insert rule inet computer_security input iifname "{interface}" ether saddr {mac} ip daddr {ip} tcp dport {images} counter accept comment "{COMMENT}"
"#,
        images = crate::ota::image::PORT,
    ))
}

pub fn apply(config: &Config, redirect: Redirect) -> Result<()> {
    // Fail before changing connectivity if the prepared broker is absent.
    std::net::TcpStream::connect_timeout(&config.mqtt_listen, std::time::Duration::from_secs(2))
        .context("local broker is not listening; start serve first")?;
    let rules = rules(config, redirect)?;

    // Check the complete transaction before removing any previous lab rules.
    let existing = Command::new("nft")
        .args(["list", "table", "inet", TABLE])
        .output()?;
    ensure!(
        !existing.status.success(),
        "local routing is already enabled; remove it before applying again"
    );

    use std::io::Write;
    // The first pass only checks (`nft --check`); the second commits the same rules.
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
