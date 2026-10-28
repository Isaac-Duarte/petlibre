//! `wellfed-stock` command line: the bench's server for stock 1.1.15.
//!
//! `serve` runs the server: the MQTT broker stock connects to, as a child
//! process of this binary, the stock install path, and the loopback HTTP
//! API. Most other subcommands are clients of that API and read the bearer
//! token from the private configuration, so the token never appears in
//! shell arguments. Starting the service redirects no traffic; `network
//! apply` is the separate, privileged step that does.

#![forbid(unsafe_code)]
#![warn(clippy::print_stdout)]

use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use reqwest::Method;
use serde::Serialize;
use tokio::{
    process::Command,
    sync::{mpsc, watch},
};
use tracing::info;
use wellfed_client::install::{SETUP_ADDRESS, SetupFrame};
use wellfed_formats::bench::lease::Lease;
use wellfed_stock::{
    api::{self, ApiState},
    build_info::{self, BUILD},
    config::{self, State},
    controller::{self, Controller},
    enroll::Enrollment,
    mqtt::{self, broker, broker::DeviceAuth},
    neighbour, network, ota, plans,
};

#[derive(Parser)]
#[command(
    version,
    about = "The bench's server for stock PLAF103 1.1.15: its broker, the image server and the install trials"
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

    /// Write an installer's config: fresh secrets and the broker address,
    /// and no device. The first stock feeder that connects while `serve
    /// --enroll-minutes` runs becomes its device. Never replaces a file.
    InitInstaller {
        /// Specific local IPv4 address and port the feeder reaches: the
        /// broker the setup frame names.
        #[arg(long)]
        mqtt_listen: std::net::SocketAddr,
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
        /// With an installer's config and no feeder enrolled yet: trust the
        /// first stock feeder that connects within this many minutes.
        #[arg(long)]
        enroll_minutes: Option<u64>,
    },

    /// Run just the authenticated Rust MQTT broker (normally managed by serve).
    Broker {
        #[arg(long, default_value = "private/server.json")]
        config: PathBuf,
        /// As `serve --enroll-minutes`.
        #[arg(long)]
        enroll_minutes: Option<u64>,
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
    /// answer: the build identity and bank its device API reported, and a
    /// note of what answered, and when. Refused while stock still reports,
    /// or for another build or bank. Never sends a device command.
    OtaTrialAnswered {
        #[arg(long, default_value = "private/server.json")]
        config: PathBuf,
        #[arg(long)]
        request_id: uuid::Uuid,
        /// The build identity the device API reported (64 hex).
        #[arg(long)]
        build: String,
        /// The bank the device API reported; the trial closes only on 0.
        #[arg(long)]
        bank: u8,
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
        #[arg(long, default_value_t = std::net::SocketAddr::V4(SETUP_ADDRESS))]
        to: std::net::SocketAddr,
    },

    /// Read stock's stored feeding plans from a file (its two plan
    /// sectors, or a whole flash read) and show what an install would
    /// import: each plan converted, or refused by name. Touches no feeder.
    Plans { file: PathBuf },

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
#[expect(
    clippy::print_stdout,
    reason = "the CLI's results and reports are its stdout"
)]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "wellfed_stock=info,mqtt5=warn".into()),
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

        Action::InitInstaller {
            mqtt_listen,
            timezone,
            output,
        } => {
            config::init_installer(&output, mqtt_listen, timezone)?;
            println!(
                "Wrote an installer's configuration to {}; no feeder is enrolled yet.",
                output.display()
            );
        }

        Action::Broker {
            config,
            enroll_minutes,
        } => {
            let path = config.canonicalize()?;
            let (listen, auth) = match (config::state(&path)?, enroll_minutes) {
                (State::Device(config), _) => (config.mqtt_listen, DeviceAuth::device(&config)),
                (State::Unenrolled(config), Some(minutes)) => (
                    config.mqtt_listen,
                    DeviceAuth::enrolling(&config, &path, Duration::from_secs(minutes * 60)),
                ),
                (State::Unenrolled(_), None) => {
                    bail!("no feeder is enrolled; pass --enroll-minutes to admit one")
                }
            };
            broker::run(listen, auth).await?
        }
        Action::Serve {
            config,
            enable_ota_trial,
            enroll_minutes,
        } => {
            let trial = if enable_ota_trial {
                OtaTrial::Enabled
            } else {
                OtaTrial::Disabled
            };
            serve(config, trial, enroll_minutes).await?
        }
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
            use wellfed_stock::setup::send;
            let _lease = bench_lease(&config)?;
            let broker = match broker {
                Some(broker) => broker,
                None => match config::state(&config)? {
                    State::Device(config) => config.mqtt_listen,
                    State::Unenrolled(config) => config.mqtt_listen,
                },
            };
            println!("Passphrase for {ssid} (read from stdin):");
            let mut password = String::new();
            std::io::stdin().read_line(&mut password)?;
            let setup = SetupFrame {
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
            // Given, else where an enrolled feeder connected from, else the
            // bench feeder's MAC on the hotspot.
            let peer = match (peer, config.device_address) {
                (Some(peer), _) => peer,
                (None, Some(std::net::IpAddr::V4(enrolled))) => enrolled,
                (None, _) => match neighbour::discover(&config)? {
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
            build,
            bank,
            note,
        } => {
            api_request(
                config,
                Method::POST,
                "/api/ota/trial/answered",
                Some(&ota::trial::AnsweredInput {
                    request_id,
                    build: build.to_ascii_lowercase(),
                    bank,
                    note,
                }),
            )
            .await?;
        }

        Action::OtaTrial {
            config,
            request_id,
            server_token,
        } => {
            // No lease of its own: the trial-enabled `serve` that dispatches
            // the install holds the bench for its whole lifetime.
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

        Action::Plans { file } => {
            let bytes =
                std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
            let stock = plans::decode(&bytes)?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .context("the clock is before 1970")?;
            let now_ms = u64::try_from(now.as_millis()).context("the clock is past 2554")?;
            print!(
                "{}",
                plans::report(&wellfed_client::install::plans::convert(&stock, now_ms))
            );
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

/// The bench lease beside `config` (in a checkout, `private/`), for what
/// drives the feeder: `setup-send`, and a trial-enabled `serve` for its
/// whole lifetime, since it dispatches a multi-minute install. A plain
/// `serve`, `broker` and `ota-images` take none: they only answer, beside
/// the bench command that drives the feeder. Run by the bench (`itest`,
/// `stock-ota`), `serve` descends from the holder and runs on its lease.
fn bench_lease(config: &std::path::Path) -> Result<Lease> {
    // The directory the config is named in, not where a symlinked config
    // points: a worktree whose `private/server.json` links to the main
    // checkout's keeps its lease in its own `private/`, beside the bench
    // that runs from it.
    let absolute = std::path::absolute(config).with_context(|| format!("{}", config.display()))?;
    let named = absolute.parent().context("config directory missing")?;
    let directory = named
        .canonicalize()
        .with_context(|| format!("{}", named.display()))?;
    Lease::acquire_in(&directory).context("bench lease")
}

/// Whether `serve` may dispatch the stock-to-replacement install trial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OtaTrial {
    Enabled,
    Disabled,
}

/// How often `serve` looks for a feeder's enrollment while it waits.
const ENROLL_POLL: Duration = Duration::from_secs(1);

/// With no feeder enrolled, wait for the broker child to record one within
/// `minutes`, then the config with that device. Stops with the broker or a
/// signal.
async fn enrolled(
    path: &std::path::Path,
    minutes: u64,
    broker: &mut tokio::process::Child,
) -> Result<config::Config> {
    info!(
        minutes,
        "no feeder enrolled; the first stock feeder to connect is trusted"
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(minutes * 60);
    let record = Enrollment::path(path);
    loop {
        if record.exists() {
            return config::load(path);
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("no stock feeder connected within {minutes} min; nothing enrolled");
        }
        tokio::select! {
            _ = tokio::time::sleep(ENROLL_POLL) => {}
            result = broker.wait() => bail!("broker exited while waiting for a feeder: {result:?}"),
            result = tokio::signal::ctrl_c() => {
                result.context("shutdown signal failed")?;
                bail!("stopped before a feeder enrolled");
            }
            result = terminated() => {
                result.context("shutdown signal failed")?;
                bail!("stopped before a feeder enrolled");
            }
        }
    }
}

async fn serve(path: PathBuf, trial: OtaTrial, enroll_minutes: Option<u64>) -> Result<()> {
    // Trial dispatch must run a binary whose source can be reviewed: a clean
    // commit of every build input. Status and /health report the commit.
    if trial == OtaTrial::Enabled
        && !BUILD.reviewable()
        && std::env::var_os(build_info::ALLOW_UNREVIEWED_ENV).is_none()
    {
        bail!(
            "--enable-ota-trial requires a binary built from a clean commit (this build: {}, clean: {})",
            BUILD.commit,
            BUILD.clean
        );
    }

    // Taken before the API binds, so a trial cannot be asked of a server
    // that does not hold the bench; held until the server stops. Found
    // from the path as given, before the symlinks below are resolved.
    let _lease = match trial {
        OtaTrial::Enabled => Some(bench_lease(&path)?),
        OtaTrial::Disabled => None,
    };
    let path = path.canonicalize()?;
    let state = config::state(&path)?;
    let enrolling = match (&state, enroll_minutes) {
        (State::Device(_), _) => None,
        (State::Unenrolled(_), Some(minutes)) => Some(minutes),
        (State::Unenrolled(_), None) => bail!(
            "no feeder is enrolled in {}; pass --enroll-minutes during an install",
            path.display()
        ),
    };

    // The broker runs as a child process of this same binary; kill_on_drop
    // ties its lifetime to serve. It starts first: an installer's waits
    // for a feeder to enroll before anything else knows the device.
    let mut broker_command = Command::new(std::env::current_exe()?);
    broker_command.arg("broker").arg("--config").arg(&path);
    if let Some(minutes) = enrolling {
        broker_command
            .arg("--enroll-minutes")
            .arg(minutes.to_string());
    }
    let mut child = broker_command.kill_on_drop(true).spawn()?;

    let config = match (state, enrolling) {
        (State::Device(config), _) => *config,
        (State::Unenrolled(_), Some(minutes)) => enrolled(&path, minutes, &mut child).await?,
        (State::Unenrolled(_), None) => bail!("no feeder is enrolled"),
    };
    let config = Arc::new(config);
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
    if trial == OtaTrial::Enabled {
        controller.enable_ota_trial();
    }

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

#[expect(
    clippy::print_stdout,
    reason = "the API's answer is the command's stdout"
)]
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

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::{Action, Cli};

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }

    /// `wellfed-itest`'s live rig spawns this binary with exactly these
    /// arguments; it links nothing of this crate, so this is where a
    /// renamed command or flag would surface.
    #[test]
    fn the_suite_invocations_parse() {
        let serve = ["wellfed-stock", "serve", "--config", "private/server.json"];
        assert!(matches!(
            Cli::try_parse_from(serve).unwrap().command,
            Action::Serve {
                enable_ota_trial: false,
                ..
            }
        ));

        let trial = [&serve[..], &["--enable-ota-trial"]].concat();
        assert!(matches!(
            Cli::try_parse_from(trial).unwrap().command,
            Action::Serve {
                enable_ota_trial: true,
                ..
            }
        ));

        let images = [
            "wellfed-stock",
            "ota-images",
            "--config",
            "private/server.json",
            "--directory",
            "private/verification/review",
        ];
        assert!(matches!(
            Cli::try_parse_from(images).unwrap().command,
            Action::OtaImages { peer: None, .. }
        ));
    }
}
