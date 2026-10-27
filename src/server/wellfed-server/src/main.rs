//! `wellfed-server`: the owner's server, as one process.
//!
//! Everything it keeps is in `--data`: the database (`server.db`), the
//! settings (`server.toml`, optional; flags and environment variables
//! override it) and the key the server's feeder keys are sealed with
//! (`keys/server.key`), the verified releases kept for installs
//! (`releases/`) and the database's backups (`backups/`). The first start
//! makes that key; back it up apart from the database's backups, which
//! leave it out: the database alone cannot open the feeder keys.
//!
//! `wellfed-server reset-owner` gives a locked-out owner a new temporary
//! password, straight on the database, and prints it.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use jiff::tz::TimeZone;
use wellfed_server::config::{BACKUPS, DATABASE, Overrides, RELEASES, ServerConfig};
use wellfed_service::{
    Backups, Clock, Config, ReleaseFeed, ReleaseKeys, ReleasesConfig, Service, SystemClock,
    SystemRandom, Timing,
};
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
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Give an owner who is locked out a new temporary password, and end
    /// their sessions. Runs on the database whether or not the server is.
    ResetOwner {
        /// The owner's name; needed only when the household has several.
        #[arg(long)]
        name: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let cli = Cli::parse();
    if let Some(Command::ResetOwner { name }) = cli.command {
        return reset_owner(&cli.data, name.as_deref()).await;
    }

    std::fs::create_dir_all(&cli.data).with_context(|| format!("create {}", cli.data.display()))?;
    let settings = ServerConfig::load(&cli.data, cli.overrides)?;
    let (key, _) = wellfed_server::key::sealing_key(&cli.data, &SystemRandom)?;
    let store = Store::open(&cli.data.join(DATABASE), SystemClock.now())
        .await
        .context("open the database")?;
    let config = Config {
        key,
        // Offered at setup only: the household's zone is the feeders'.
        host_zone: TimeZone::system().iana_name().map(str::to_owned),
        label: settings.label.into(),
        discovery: settings.discovery.into(),
        timing: Timing::default(),
        releases: ReleasesConfig {
            dir: cli.data.join(RELEASES),
            keys: ReleaseKeys {
                project: settings
                    .releases
                    .project_keys
                    .iter()
                    .map(|key| key.0)
                    .collect(),
                owner: settings
                    .releases
                    .owner_keys
                    .iter()
                    .map(|key| key.0)
                    .collect(),
            },
            fetch: settings.releases.fetch.then(ReleaseFeed::default),
        },
        raw_sample_days: settings.retention.raw_sample_days,
        backups: Backups {
            dir: cli.data.join(BACKUPS),
            keep: settings.backups.keep,
        },
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
    let jobs = wellfed_server::jobs::spawn(service.clone(), service.clock(), shutdown.clone());

    let stop = shutdown.clone();
    let served = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            stopped().await;
            stop.cancel();
        })
        .await;

    // The jobs stop before the service, so none is cut off by the store
    // closing under it.
    shutdown.cancel();
    if let Err(error) = jobs.await {
        tracing::error!("the jobs' task: {error}");
    }
    service.shutdown().await;
    served.context("serve")
}

/// Print a new password for the owner, on an existing database only.
async fn reset_owner(data: &std::path::Path, name: Option<&str>) -> Result<()> {
    let path = data.join(DATABASE);
    anyhow::ensure!(path.exists(), "no database at {}", path.display());
    let store = Store::open(&path, SystemClock.now())
        .await
        .context("open the database")?;

    let reset = wellfed_service::reset_owner(&store, name, &SystemRandom).await;
    store.close().await;
    let (owner, password) = reset.context("reset the owner's password")?;
    println!("{}'s temporary password: {password}", owner.name);
    println!("Log in with it, then set your own on your account.");
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
