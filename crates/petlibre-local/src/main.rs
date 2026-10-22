//! `petlibre-local` command line.
//!
//! `serve` runs the stock-firmware stack: the MQTT broker as a child process
//! of this binary, the device protocol worker and the loopback HTTP API. Most
//! other subcommands are clients of that API and read the bearer token from
//! the private configuration, so the token never appears in shell arguments.
//! Starting the service redirects no traffic; `network apply` is the separate,
//! privileged step that does.

use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use petlibre_local::{
    api::{self, ApiState},
    broker,
    build_info::{self, BUILD},
    config::{self, Config},
    controller::{Controller, FeedInput, ResolveInput},
    firmware_ota, mqtt, network, ota_image, ota_trial, probe,
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

    /// Add a request-signing key to a stopped server's firmware role and the
    /// firmware build's credential file; never rotates one or prints it.
    FirmwareRequestKey {
        #[arg(long, default_value = "private/server.json")]
        config: PathBuf,
        #[arg(long, default_value = "private/fw-mqtt.json")]
        credential: PathBuf,
    },

    /// Create the replacement firmware's update-signing key (FW10); never
    /// replaces one. Prints the public key the firmware build embeds.
    OtaKeygen {
        #[arg(long, default_value = "private/ota-key.json")]
        output: PathBuf,
    },

    /// Sign a reviewed replacement image's update manifest (FW10).
    OtaSign {
        #[arg(long, default_value = "private/ota-key.json")]
        key: PathBuf,
        /// The reviewed OTA container (a frozen review's image.bin).
        #[arg(long)]
        image: PathBuf,
        /// Must exceed the installed image's version.
        #[arg(long)]
        version: u32,
        /// The image's startup build ID (64 hex characters).
        #[arg(long)]
        build_id: String,
        #[arg(long)]
        output: PathBuf,
    },

    /// Offer a signed replacement update to the running server (FW10), or
    /// clear the offer. Paths are relative and under private/; nothing is
    /// sent to the firmware until it asks.
    OtaOffer {
        #[arg(long, default_value = "private/server.json")]
        config: PathBuf,
        #[arg(long, requires = "image", conflicts_with = "clear")]
        manifest: Option<String>,
        #[arg(long, requires = "manifest", conflicts_with = "clear")]
        image: Option<String>,
        #[arg(long)]
        clear: bool,
    },

    /// Play an audio file (any format ffmpeg reads, or - for stdin) on a
    /// feeder running the speaker variant. Talks to the feeder directly.
    Speak {
        #[arg(long, default_value = "private/server.json")]
        config: PathBuf,
        file: PathBuf,
        /// The feeder's address; found by its MAC on the hotspot otherwise.
        #[arg(long)]
        host: Option<std::net::IpAddr>,
        /// Sample rate: 8000, 16000, 32000 or 48000.
        #[arg(long, default_value_t = 32_000)]
        rate: u32,
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
        Action::Speak {
            config,
            file,
            host,
            rate,
        } => {
            let config = Config::load(&config)?;
            let host = match host {
                Some(host) => host,
                None => petlibre_local::speak::discover(&config)?,
            };
            tokio::task::spawn_blocking(move || {
                petlibre_local::speak::play(&config, &file, host, rate)
            })
            .await??;
        }

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

        Action::FirmwareRequestKey { config, credential } => {
            config::add_request_key(&config, &credential)?;
            println!(
                "Added a request key to {} and {}. Rebuild the firmware and restart serve to use it.",
                credential.display(),
                config.display()
            );
        }

        Action::OtaKeygen { output } => {
            let public = firmware_ota::keygen(&output)?;
            println!("Wrote {}; public key {public}", output.display());
        }

        Action::OtaSign {
            key,
            image,
            version,
            build_id,
            output,
        } => {
            let manifest = firmware_ota::sign(
                &firmware_ota::load_key(&key)?,
                &std::fs::read(&image)?,
                version,
                firmware_ota::parse_build_id(&build_id)?,
            )?;
            anyhow::ensure!(!output.exists(), "refusing to replace {}", output.display());
            std::fs::write(&output, manifest)?;
            println!(
                "Signed manifest for version {version} in {}",
                output.display()
            );
        }

        Action::OtaOffer {
            config,
            manifest,
            image,
            clear,
        } => {
            anyhow::ensure!(
                clear || manifest.is_some(),
                "give --manifest and --image, or --clear"
            );
            api_request(
                config,
                "POST",
                "/api/replacement/ota/offer",
                Some(serde_json::json!({"manifest": manifest, "image": image})),
            )
            .await?;
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
