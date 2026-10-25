//! `wellfed-server`: the owner's server, as one process.
//!
//! Everything it keeps is in `--data`: the database (`server.db`) and
//! `config.json`, which holds the key the server's feeder keys are sealed
//! with. The first start makes that key; keep the two files together, and
//! back up both: the database alone cannot open the feeder keys.

use std::io::Write;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::Parser;
use jiff::tz::TimeZone;
use wellfed_api::Hex;
use wellfed_service::{Config, Discovery, Random, Service, SystemClock, SystemRandom, Timing};
use wellfed_store::Store;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

/// The label the server enrolls under on each feeder.
const LABEL: &str = "wellfed-server";

#[derive(Parser)]
#[command(version, about = "The owner's Wellfed server")]
struct Cli {
    /// The directory that keeps the database and the key file.
    #[arg(long)]
    data: PathBuf,
    /// Where the HTTP API listens: this computer only, unless the
    /// household's other devices should reach it (`0.0.0.0:8080`).
    #[arg(long, default_value_t = SocketAddr::from((Ipv4Addr::LOCALHOST, 8080)))]
    listen: SocketAddr,
    /// Never look feeders up over mDNS: adopt each by address.
    #[arg(long)]
    no_discovery: bool,
    /// A name the household reaches the server by (`feeder.lan`); IP
    /// addresses and `localhost` always work. Repeat for more names.
    #[arg(long = "host")]
    hosts: Vec<String>,
}

/// `config.json`.
#[derive(Serialize, Deserialize)]
struct ConfigFile {
    /// Seals the server's feeder keys.
    key: Hex<32>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let cli = Cli::parse();

    std::fs::create_dir_all(&cli.data).with_context(|| format!("create {}", cli.data.display()))?;
    let key = config_key(&cli.data.join("config.json"))?;
    let store = Store::open(&cli.data.join("server.db"))
        .await
        .context("open the database")?;
    let config = Config {
        key,
        zone: TimeZone::system(),
        label: LABEL.into(),
        discovery: if cli.no_discovery {
            Discovery::Off
        } else {
            Discovery::Mdns
        },
        timing: Timing::default(),
    };
    let service = Service::start(store, config, Arc::new(SystemClock), Arc::new(SystemRandom))
        .await
        .context("start the feeder connections")?;

    let shutdown = CancellationToken::new();
    let hosts = wellfed_server::Hosts::new(cli.hosts);
    let app = wellfed_server::app(service.clone(), shutdown.clone(), hosts);
    let listener = tokio::net::TcpListener::bind(cli.listen)
        .await
        .with_context(|| format!("listen on {}", cli.listen))?;
    tracing::info!("serving on http://{}", listener.local_addr()?);

    let stop = shutdown.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            stopped().await;
            stop.cancel();
        })
        .await
        .context("serve")?;
    service.shutdown().await;
    Ok(())
}

/// Ctrl-C, or SIGTERM from a service manager: either way the feeder
/// connections stop and the database closes cleanly.
async fn stopped() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = terminate.recv() => {}
                }
                return;
            }
            Err(error) => tracing::error!("listen for SIGTERM: {error}"),
        }
    }
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!("wait for Ctrl-C: {error}");
    }
}

/// The sealing key from `path`, made on the first start. The file is
/// written once, readable by this user only.
fn config_key(path: &Path) -> Result<[u8; 32]> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let file: ConfigFile = serde_json::from_slice(&bytes)
                .with_context(|| format!("read {}", path.display()))?;
            return Ok(file.key.0);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => bail!("read {}: {error}", path.display()),
    }

    let mut key = [0; 32];
    SystemRandom.fill(&mut key)?;
    let json = serde_json::to_vec_pretty(&ConfigFile { key: Hex(key) })?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options
        .open(path)
        .with_context(|| format!("create {}", path.display()))?;
    file.write_all(&json)
        .and_then(|()| file.sync_all())
        .with_context(|| format!("write {}", path.display()))?;
    tracing::info!("made a new key in {}", path.display());
    Ok(key)
}
