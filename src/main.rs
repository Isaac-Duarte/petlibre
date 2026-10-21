use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use petlibre_local::{
    api::{self, ApiState},
    broker,
    build_info::{self, BUILD},
    config::{self, Config},
    controller::{Controller, FeedInput, ResolveInput},
    mqtt, network, ota_image, ota_trial, probe,
};
use tokio::{
    process::Command,
    sync::{mpsc, watch},
};
use tracing::info;
use uuid::Uuid;

#[derive(Parser)]
#[command(version, about = "Local stock-firmware PLAF103 server")]
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
        /// Allow only the fixed modified-stock/original experiment; disabled by default.
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
    /// Request a supervised feed. Reuse the same request ID for any retry.
    Feed {
        #[arg(long, default_value = "private/server.json")]
        config: PathBuf,
        #[arg(long)]
        request_id: Uuid,
        #[arg(long, default_value_t = 1)]
        portions: u8,
    },
    /// Resolve an uncertain operation after physically checking the feeder.
    Resolve {
        #[arg(long, default_value = "private/server.json")]
        config: PathBuf,
        #[arg(long)]
        request_id: Uuid,
        #[arg(long)]
        note: String,
    },
    /// Print an operation identifier without sending a command.
    NewRequestId,
    /// Serve a pinned experiment/original pair on the fixed trial port; never
    /// sends an OTA command. Writes a private token lease beside the config.
    OtaImages {
        #[arg(long, default_value = "private/server.json")]
        config: PathBuf,
        #[arg(long)]
        directory: PathBuf,
        #[arg(long)]
        peer: std::net::Ipv4Addr,
        #[arg(long, value_enum, default_value_t = ota_image::Profile::StockVersion)]
        profile: ota_image::Profile,
    },
    /// Explicitly run GET_CONFIG once on a confirmed C-probe installation.
    ProbeQuery {
        #[arg(long, default_value = "private/server.json")]
        config: PathBuf,
        #[arg(long)]
        request_id: Uuid,
    },
    /// Explicitly submit one fixed OTA trial or restore; never retries automatically.
    OtaTrial {
        #[arg(long, default_value = "private/server.json")]
        config: PathBuf,
        #[arg(long)]
        request_id: Uuid,
        #[arg(long)]
        image: ota_trial::Image,
        #[arg(long)]
        server_token: Uuid,
    },
    /// Close the latest uncertain/failed trial from fresh version evidence after
    /// the documented waiting periods; never sends a device command.
    OtaTrialResolve {
        #[arg(long, default_value = "private/server.json")]
        config: PathBuf,
        #[arg(long)]
        request_id: Uuid,
        #[arg(long)]
        note: String,
    },
    /// Add the replacement-firmware broker role to a stopped server's config and
    /// write the firmware build's private credential file; never rotates one.
    FirmwareCredential {
        #[arg(long, default_value = "private/server.json")]
        config: PathBuf,
        #[arg(long, default_value = "private/fw-mqtt.json")]
        output: PathBuf,
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
                .unwrap_or_else(|_| "petlibre_local=info,mqtt5=warn".into()),
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
        Action::Broker { config } => broker::run(Config::load(&config)?).await?,
        Action::Serve {
            config,
            enable_ota_trial,
        } => serve(config, enable_ota_trial).await?,
        Action::Status { config } => api_request(config, "GET", "/api/status", None).await?,
        Action::Feed {
            config,
            request_id,
            portions,
        } => {
            api_request(
                config,
                "POST",
                "/api/feed",
                Some(serde_json::to_value(FeedInput {
                    request_id,
                    portions,
                })?),
            )
            .await?
        }
        Action::Resolve {
            config,
            request_id,
            note,
        } => {
            api_request(
                config,
                "POST",
                "/api/feed/resolve",
                Some(serde_json::to_value(ResolveInput { request_id, note })?),
            )
            .await?
        }
        Action::NewRequestId => println!("{}", Uuid::new_v4()),
        Action::ProbeQuery { config, request_id } => {
            api_request(
                config,
                "POST",
                "/api/probe/query",
                Some(serde_json::to_value(probe::Input { request_id })?),
            )
            .await?
        }
        Action::OtaImages {
            config,
            directory,
            peer,
            profile,
        } => {
            let path = config.canonicalize()?;
            ota_image::serve(Config::load(&path)?, &path, &directory, peer, profile).await?;
        }
        Action::OtaTrialResolve {
            config,
            request_id,
            note,
        } => {
            api_request(
                config,
                "POST",
                "/api/ota/trial/resolve",
                Some(serde_json::to_value(ota_trial::ResolveInput {
                    request_id,
                    note,
                })?),
            )
            .await?;
        }
        Action::OtaTrial {
            config,
            request_id,
            image,
            server_token,
        } => {
            api_request(
                config,
                "POST",
                "/api/ota/trial",
                Some(serde_json::to_value(ota_trial::Input {
                    request_id,
                    image,
                    server_token,
                })?),
            )
            .await?;
        }
        Action::FirmwareCredential { config, output } => {
            config::add_firmware_credential(&config, &output)?;
            println!(
                "Added the replacement-firmware role to {} and wrote {}. Restart serve to use it.",
                config.display(),
                output.display()
            );
        }
        Action::Network { action } => match action {
            NetworkAction::Apply { config } => {
                network::apply(&Config::load(&config)?)?;
                println!(
                    "Test-feeder MQTT redirected locally; its forwarded WAN traffic is blocked."
                );
            }
            NetworkAction::Remove => {
                network::remove()?;
                println!("Local broker routing removed; prior hotspot forwarding remains.");
            }
            NetworkAction::Preview { config } => {
                println!("{}", network::rules(&Config::load(&config)?)?)
            }
        },
    }
    Ok(())
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
    let config = Arc::new(Config::load(&path)?);
    let listener = tokio::net::TcpListener::bind(config.http_listen)
        .await
        .context("bind loopback HTTP API")?;
    let mut controller = Controller::load(
        (*config).clone(),
        path.parent()
            .context("config directory missing")?
            .join("feed-ledger.json"),
    )?;
    if enable_ota_trial {
        controller.enable_ota_trial();
    }
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
    let mut http_finished = false;
    let outcome: Result<()> = tokio::select! {
        result = &mut worker => result.context("MQTT worker panicked").and_then(|r|r),
        result = &mut http => {http_finished=true; Err(anyhow::anyhow!("HTTP server exited unexpectedly: {result:?}"))},
        result = child.wait() => Err(anyhow::anyhow!("broker exited unexpectedly: {result:?}")),
        result = tokio::signal::ctrl_c() => result.context("shutdown signal failed"),
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
    method: &str,
    route: &str,
    body: Option<serde_json::Value>,
) -> Result<()> {
    let config = Config::load(&path)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .no_proxy()
        .build()?;
    let mut request = client
        .request(
            method.parse()?,
            format!("http://{}{route}", config.http_listen),
        )
        .bearer_auth(config.api_token);
    if let Some(body) = body {
        request = request.json(&body);
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
