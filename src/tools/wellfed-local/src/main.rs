//! `wellfed-local` command line.
//!
//! `serve` runs the server: the MQTT broker stock connects to, as a child
//! process of this binary, the stock install path, and the loopback HTTP
//! API. Most
//! other subcommands are clients of that API and read the bearer token from
//! the private configuration, so the token never appears in shell arguments.
//! Starting the service redirects no traffic; `network apply` is the separate,
//! privileged step that does.

#![forbid(unsafe_code)]
#![warn(clippy::print_stdout)]

use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use wellfed_local::{
    api::{self, ApiState},
    build_info::{self, BUILD},
    config, neighbour, network,
};
use wellfed_stock::{
    controller::{self, Controller},
    mqtt::{self, broker},
    ota,
};
use reqwest::Method;
use serde::Serialize;
use tokio::{
    process::Command,
    sync::{mpsc, watch},
};
use tracing::info;

#[derive(Parser)]
#[command(
    version,
    about = "The bench's local PLAF103 server: stock's broker and its install trials"
)]
struct Cli {
    #[command(subcommand)]
    command: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Import captured credentials and plans; never overwrites existing config.
    Init {
        #[arg(long)]
        credentials: PathBuf,
        #[arg(long)]
        capture: PathBuf,
        /// Specific local IPv4 address and port reachable by the feeder.
        #[arg(long)]
        mqtt_listen: std::net::SocketAddr,
        /// Interface receiving the feeder's traffic.
        #[arg(long)]
        hotspot_interface: String,
        /// IANA timezone used for device clock replies.
        #[arg(long, default_value = "UTC")]
        timezone: chrono_tz::Tz,
        #[arg(long, default_value = "private/server.json")]
        output: PathBuf,
    },

    /// Start the Rust broker, device protocol service and loopback HTTP API.
    Serve {
        #[arg(long, default_value = "private/server.json")]
        config: PathBuf,
        /// Allow the stock-to-replacement install trial; disabled by default.
        #[arg(long)]
        enable_ota_trial: bool,
    },

    /// Run just the authenticated Rust MQTT broker (normally managed by serve).
    Broker {
        #[arg(long, default_value = "private/server.json")]
        config: PathBuf,
    },

    /// Inspect authenticated local status without printing credentials.
    Status {
        #[arg(long, default_value = "private/server.json")]
        config: PathBuf,
    },

    /// Serve a reviewed replacement build on the fixed trial port for stock
    /// to install; never sends an OTA command. Writes a private token lease
    /// beside the config.
    OtaImages {
        #[arg(long, default_value = "private/server.json")]
        config: PathBuf,
        /// The build's frozen review (its `review.json` and `image.bin`).
        #[arg(long)]
        directory: PathBuf,
        /// The feeder's address; found by its MAC on the hotspot otherwise.
        #[arg(long)]
        peer: Option<std::net::Ipv4Addr>,
    },

    /// Ask stock once to install the build `ota-images` serves; never retries.
    OtaTrial {
        #[arg(long, default_value = "private/server.json")]
        config: PathBuf,
        #[arg(long)]
        request_id: uuid::Uuid,
        #[arg(long)]
        server_token: uuid::Uuid,
    },

    /// Close the latest uncertain/failed trial from fresh version evidence after
    /// the documented waiting periods; never sends a device command.
    OtaTrialResolve {
        #[arg(long, default_value = "private/server.json")]
        config: PathBuf,
        #[arg(long)]
        request_id: uuid::Uuid,
        #[arg(long)]
        note: String,
    },

    /// Close the latest submitted or uncertain trial on the replacement's
    /// answer: the note says what answered over the device API, and when.
    /// Never sends a device command.
    OtaTrialAnswered {
        #[arg(long, default_value = "private/server.json")]
        config: PathBuf,
        #[arg(long)]
        request_id: uuid::Uuid,
        #[arg(long)]
        note: String,
    },

    /// Point a stock feeder in setup mode at a network and this
    /// server with stock's own type-3 setup frame, sent once to its setup
    /// access point (join that network first). The passphrase is read from
    /// stdin, never the command line.
    SetupSend {
        #[arg(long, default_value = "private/server.json")]
        config: PathBuf,
        /// The network stock should join; its passphrase comes from stdin.
        #[arg(long)]
        ssid: String,
        /// The account ID stock keeps beside the broker (ASCII digits).
        #[arg(long)]
        id: String,
        /// The broker; this server's MQTT address otherwise.
        #[arg(long)]
        broker: Option<std::net::SocketAddr>,
        #[arg(long, default_value = wellfed_stock::setup::SETUP_ADDRESS)]
        to: std::net::SocketAddr,
    },

    /// Explicit feeder-only gateway routing and WAN isolation; needs sudo.
    Network {
        #[command(subcommand)]
        action: NetworkAction,
    },
}

#[derive(Subcommand)]
enum NetworkAction {
    Apply {
        #[arg(long, default_value = "private/server.json")]
        config: PathBuf,
    },
    Remove,
    Preview {
        #[arg(long, default_value = "private/server.json")]
        config: PathBuf,
    },
}

#[tokio::main]
#[allow(clippy::print_stdout)]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "wellfed_local=info,wellfed_stock=info,mqtt5=warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    match Cli::parse().command {
        Action::Init {
            credentials,
            capture,
            mqtt_listen,
            hotspot_interface,
            timezone,
            output,
        } => {
            config::import(
                &credentials,
                &capture,
                &output,
                config::ImportOptions {
                    mqtt_listen,
                    hotspot_interface,
                    timezone,
                },
            )?;
            println!(
                "Imported private configuration to {}. No device commands sent.",
                output.display()
            );
        }

        Action::Broker { config } => broker::run(config::load(&config)?).await?,
        Action::Serve {
            config,
            enable_ota_trial,
        } => serve(config, enable_ota_trial).await?,
        Action::Status { config } => {
            api_request(config, Method::GET, "/api/status", None::<&()>).await?
        }
        Action::SetupSend {
            config,
            ssid,
            id,
            broker,
            to,
        } => {
            use wellfed_stock::setup::{Setup, send};
            let broker = broker.unwrap_or(config::load(&config)?.mqtt_listen);
            println!("Passphrase for {ssid} (read from stdin):");
            let mut password = String::new();
            std::io::stdin().read_line(&mut password)?;
            let setup = Setup {
                ssid,
                password: password.trim_end_matches(['\r', '\n']).to_owned(),
                host: broker.ip().to_string(),
                port: broker.port(),
                id,
            };
            match send(&setup, to, std::time::Duration::from_secs(5))? {
                Some(answer) => println!(
                    "Sent once to {to}; the feeder answered {} bytes: {}",
                    answer.len(),
                    wellfed_formats::hex::encode(&answer)
                ),
                None => println!("Sent once to {to}; no answer within 5 s (not resent)"),
            }
        }
        Action::OtaImages {
            config,
            directory,
            peer,
        } => {
            let path = config.canonicalize()?;
            let config = config::load(&path)?;
            let peer = match peer {
                Some(peer) => peer,
                None => match neighbour::discover(&config)? {
                    std::net::IpAddr::V4(peer) => peer,
                    std::net::IpAddr::V6(_) => {
                        anyhow::bail!("the feeder has no IPv4 neighbour entry")
                    }
                },
            };
            // The bench stops it with SIGTERM; either way its lease is removed.
            let stop = async {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {},
                    _ = terminated() => {},
                }
            };
            ota::image::serve(config, &path, &directory, peer, stop).await?;
        }

        Action::OtaTrialResolve {
            config,
            request_id,
            note,
        } => {
            api_request(
                config,
                Method::POST,
                "/api/ota/trial/resolve",
                Some(&ota::trial::ResolveInput { request_id, note }),
            )
            .await?;
        }

        Action::OtaTrialAnswered {
            config,
            request_id,
            note,
        } => {
            api_request(
                config,
                Method::POST,
                "/api/ota/trial/answered",
                Some(&ota::trial::ResolveInput { request_id, note }),
            )
            .await?;
        }

        Action::OtaTrial {
            config,
            request_id,
            server_token,
        } => {
            api_request(
                config,
                Method::POST,
                "/api/ota/trial",
                Some(&ota::trial::Input {
                    request_id,
                    server_token,
                }),
            )
            .await?;
        }

        Action::Network { action } => match action {
            NetworkAction::Apply { config } => {
                network::apply(&config::load(&config)?)?;
                println!(
                    "Test-feeder MQTT redirected locally; its forwarded WAN traffic is blocked."
                );
            }
            NetworkAction::Remove => {
                network::remove()?;
                println!("Local broker routing removed; prior hotspot forwarding remains.");
            }
            NetworkAction::Preview { config } => {
                println!("{}", network::rules(&config::load(&config)?)?)
            }
        },
    }

    Ok(())
}

/// Resolves on SIGTERM; never elsewhere.
async fn terminated() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?
            .recv()
            .await;
        Ok(())
    }
    #[cfg(not(unix))]
    std::future::pending().await
}

async fn serve(path: PathBuf, enable_ota_trial: bool) -> Result<()> {
    // Trial dispatch must run a binary whose source can be reviewed: a clean
    // commit of every build input. Status and /health report the commit.
    if enable_ota_trial
        && !BUILD.reviewable()
        && std::env::var_os(build_info::ALLOW_UNREVIEWED_ENV).is_none()
    {
        bail!(
            "--enable-ota-trial requires a binary built from a clean commit (this build: {}, clean: {})",
            BUILD.commit,
            BUILD.clean
        );
    }

    let path = path.canonicalize()?;
    let config = Arc::new(config::load(&path)?);
    let listener = tokio::net::TcpListener::bind(config.http_listen)
        .await
        .context("bind loopback HTTP API")?;

    let mut controller = Controller::load(
        (*config).clone(),
        path.parent()
            .context("config directory missing")?
            .join(controller::LEDGER_FILE),
        BUILD,
    )?;
    if enable_ota_trial {
        controller.enable_ota_trial();
    }

    // The broker runs as a child process of this same binary; kill_on_drop
    // ties its lifetime to serve.
    let mut child = Command::new(std::env::current_exe()?)
        .arg("broker")
        .arg("--config")
        .arg(&path)
        .kill_on_drop(true)
        .spawn()?;

    let (requests, receiver) = mpsc::channel(16);
    let (status, status_rx) = watch::channel(controller.status.clone());
    let (shutdown, mut shutdown_rx) = watch::channel(false);
    let mut worker = tokio::spawn(mqtt::run(config.clone(), controller, receiver, status));

    let app = api::router(ApiState {
        token: Arc::from(config.api_token.as_str()),
        requests,
        status: status_rx,
        config_path: Arc::from(path.as_path()),
        image_host: config.mqtt_listen.ip(),
    });
    let mut http = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.changed().await;
            })
            .await
    });

    info!(mqtt=%config.mqtt_listen,http=%config.http_listen,"local service starting; no traffic redirection applied");
    // select! may consume `http`; a JoinHandle must not be awaited again after
    // it completes, so the shutdown wait below is skipped in that case.
    let mut http_finished = false;
    let outcome: Result<()> = tokio::select! {
        result = &mut worker => result.context("MQTT worker panicked").and_then(|r|r),
        result = &mut http => {http_finished=true; Err(anyhow::anyhow!("HTTP server exited unexpectedly: {result:?}"))},
        result = child.wait() => Err(anyhow::anyhow!("broker exited unexpectedly: {result:?}")),
        result = tokio::signal::ctrl_c() => result.context("shutdown signal failed"),
        // A SIGTERM (kill, timeout) must also stop the broker child; ending
        // without this cleanup orphaned it (observed).
        result = terminated() => result.context("shutdown signal failed"),
    };

    let _ = shutdown.send(true);
    worker.abort();
    let _ = child.kill().await;
    if !http_finished {
        let _ = tokio::time::timeout(Duration::from_secs(3), http).await;
    }

    info!("service stopped; use network remove to restore cloud routing if enabled");
    outcome
}

#[allow(clippy::print_stdout)]
async fn api_request(
    path: PathBuf,
    method: Method,
    route: &str,
    body: Option<&impl Serialize>,
) -> Result<()> {
    let config = config::load(&path)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .no_proxy()
        .build()?;

    let mut request = client
        .request(method, format!("http://{}{route}", config.http_listen))
        .bearer_auth(&config.api_token);
    if let Some(body) = body {
        request = request.json(body);
    }

    let response = request.send().await?;
    let status = response.status();
    let body: serde_json::Value = response.json().await?;
    println!("{}", serde_json::to_string_pretty(&body)?);
    if !status.is_success() {
        bail!("local API returned {status}");
    }

    Ok(())
}
