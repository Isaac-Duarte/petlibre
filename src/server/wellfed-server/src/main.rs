//! `wellfed-server`: the owner's server, as one process.
//!
//! Everything it keeps is in `--data`: the database (`server.db`), the
//! settings (`server.toml`, optional; flags and environment variables
//! override it) and the key the server's feeder keys are sealed with
//! (`keys/server.key`). The first start makes that key; back it up apart
//! from the database's backups: the database alone cannot open the feeder
//! keys.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use jiff::tz::TimeZone;
use wellfed_server::config::{DATABASE, Overrides, ServerConfig};
use wellfed_service::{Clock, Config, Service, SystemClock, SystemRandom, Timing};
use wellfed_store::Store;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(version, about = "The owner's Wellfed server")]
struct Cli {
    /// The directory that keeps the database, the settings and the key.
    #[arg(long, env = "WELLFED_SERVER_DATA")]
    data: PathBuf,
    #[command(flatten)]
    overrides: Overrides,
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
    let settings = ServerConfig::load(&cli.data, cli.overrides)?;
    let (key, _) = wellfed_server::key::sealing_key(&cli.data, &SystemRandom)?;
    let store = Store::open(&cli.data.join(DATABASE), SystemClock.now())
        .await
        .context("open the database")?;
    let config = Config {
        key,
        zone: TimeZone::system(),
        label: settings.label.into(),
        discovery: settings.discovery.into(),
        timing: Timing::default(),
    };
    let service = Service::start(store, config, Arc::new(SystemClock), Arc::new(SystemRandom))
        .await
        .context("start the feeder connections")?;

    let shutdown = CancellationToken::new();
    let hosts = wellfed_server::Hosts::new(settings.hosts.into_iter().map(String::from));
    let app = wellfed_server::app(service.clone(), shutdown.clone(), hosts);
    let listener = tokio::net::TcpListener::bind(settings.listen)
        .await
        .with_context(|| format!("listen on {}", settings.listen))?;
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
