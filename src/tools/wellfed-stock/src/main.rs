//! `wellfed-stock` command line: the bench's server for stock 1.1.15.
//!
//! `serve` runs the server ([`wellfed_stock::server`]): the MQTT broker
//! stock connects to, the stock install path, and the loopback HTTP API, in
//! this one process. Most other subcommands are clients of that API and read
//! the bearer token from the private configuration, so the token never
//! appears in shell arguments. Starting the service redirects no traffic;
//! `network apply` is the separate, privileged step that does.

#![forbid(unsafe_code)]
#![warn(clippy::print_stdout)]

use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use reqwest::Method;
use serde::Serialize;
use tracing::info;
use wellfed_client::install::{SETUP_ADDRESS, SetupFrame};
use wellfed_formats::bench::lease::Lease;
use wellfed_stock::{
    build_info::BUILD,
    config::{self, State},
    mqtt::{broker, broker::DeviceAuth},
    neighbour, network, ota,
    server::{self, OtaTrial, Server},
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
        /// `off` keeps the WAN block and the input accepts but redirects
        /// nothing: stock reaches the broker only where a setup frame named it.
        #[arg(long, value_enum, default_value_t = network::Redirect::Broker)]
        redirect: network::Redirect,
    },
    Remove,
    Preview {
        #[arg(long, default_value = "private/server.json")]
        config: PathBuf,
        #[arg(long, value_enum, default_value_t = network::Redirect::Broker)]
        redirect: network::Redirect,
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
            let stop = async {
                let _ = tokio::signal::ctrl_c().await;
            };
            broker::run(listen, auth, stop).await?
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
            let mut images = ota::image::start(&config, &path, &directory, peer).await?;
            let std::net::IpAddr::V4(host) = config.mqtt_listen.ip() else {
                bail!("OTA image host must use IPv4");
            };
            println!(
                "Image server only; no update requested. Stops accepting at {} or on Ctrl-C or SIGTERM; active transfers drain.",
                images.lease.expires_at
            );
            println!("Server token: {}", images.lease.token);
            println!("Build: {}", images.url(host));

            // The bench stops it with SIGTERM; either way its lease is removed.
            let ended = tokio::select! {
                result = images.finished() => Some(result),
                _ = tokio::signal::ctrl_c() => None,
                _ = terminated() => None,
            };
            images.stop(DRAIN).await;
            ended.transpose()?;
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

        Action::Network { action } => match action {
            NetworkAction::Apply { config, redirect } => {
                network::apply(&config::load(&config)?, redirect)?;
                match redirect {
                    network::Redirect::Broker => println!(
                        "Test-feeder MQTT redirected locally; its forwarded WAN traffic is blocked."
                    ),
                    network::Redirect::Off => println!(
                        "Test-feeder MQTT not redirected (it reaches the broker only where a setup frame named it); its forwarded WAN traffic is blocked."
                    ),
                }
            }
            NetworkAction::Remove => {
                network::remove()?;
                println!("Local broker routing removed; prior hotspot forwarding remains.");
            }
            NetworkAction::Preview { config, redirect } => {
                println!("{}", network::rules(&config::load(&config)?, redirect)?)
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

/// How long a stopped `ota-images` lets a transfer under way finish: its
/// connection deadline, and a little more.
const DRAIN: Duration = Duration::from_secs(310);

async fn serve(path: PathBuf, trial: OtaTrial, enroll_minutes: Option<u64>) -> Result<()> {
    // Trial dispatch must run a binary whose source can be reviewed: a clean
    // commit of every build input. Status and /health report the commit.
    // Checked here before the lease, and again where the server starts.
    if trial == OtaTrial::Enabled {
        BUILD
            .require_reviewed()
            .context("--enable-ota-trial refused")?;
    }

    // Taken before the API binds, so a trial cannot be asked of a server
    // that does not hold the bench; held until the server stops. Found
    // from the path as given, before the symlinks below are resolved.
    let _lease = match trial {
        OtaTrial::Enabled => Some(bench_lease(&path)?),
        OtaTrial::Disabled => None,
    };
    let path = path.canonicalize()?;
    let mut server = Server::start(
        &path,
        server::Options {
            trial,
            enroll: enroll_minutes.map(|minutes| Duration::from_secs(minutes * 60)),
            api: server::Api::Loopback,
            bind: server::Bind::Configured,
        },
    )?;

    // A SIGTERM (kill, timeout) must stop the broker too, through the same
    // stop as Ctrl-C.
    let outcome: Result<()> = tokio::select! {
        result = server.finished() => result,
        result = tokio::signal::ctrl_c() => result.context("shutdown signal failed"),
        result = terminated() => result.context("shutdown signal failed"),
    };
    server.stop().await;

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
