//! Listening for a feeder after an install request whose outcome is not
//! known: the request may have reached stock, so it is never sent again,
//! and only what the feeder does next can say what became of it.
//!
//! The listener runs stock's server with the trial disabled, so nothing in
//! it can send an install. A connected stock hears what any session with it
//! hears: read-only attribute queries and the replies to its own reports. It
//! watches for up to [`UNCERTAIN_WAIT`]: stock
//! coming back and reporting its version after the request means it did not
//! install, and the replacement's device API answering where stock
//! connected from means it did. A consumer image has no network until
//! Bluetooth setup gives it one, so it never answers here: the platform
//! also looks for it advertising setup, and stops the listener when it
//! hears it.

use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, Instant};

use wellfed_formats::stock::StockStatus;
use wellfed_proto::api::PORT;
use wellfed_stock::ota::trial::{ORIGINAL_VERSION, ResolveInput};
use wellfed_stock::server::{self, Api, Handle, OtaTrial, Server};

use crate::Error;
use crate::track::{Stop, Stopped};

/// How long an uncertain request is listened for: the stock server's ledger
/// resolves one no sooner than 20 minutes after it went out.
pub const UNCERTAIN_WAIT: Duration = Duration::from_secs(20 * 60);

/// How often the listener looks.
const POLL: Duration = Duration::from_secs(5);
/// How long a look at the replacement's port may take.
const CONNECT_WAIT: Duration = Duration::from_secs(2);

/// What the listener saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listened {
    /// The device API answers at `address`: stock installed the replacement.
    Replacement { address: SocketAddr },
    /// Stock reported its version 1.1.15 after the request, so it still
    /// runs. With `resolved`, the ledger closed the request and a new run
    /// may ask again; otherwise a new run is refused until it can.
    StockStayed { resolved: bool },
    /// Nothing, for the whole wait: still not known.
    Silent,
}

/// Where the listener is, each time it looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Listening {
    pub elapsed: Duration,
    /// Stock has reported its version since the request.
    pub stock: bool,
}

/// Listen for up to `wait` with the installer's `config`, until the feeder
/// says what became of the request or `stop` resolves. Whatever it sees, it
/// sends no install: only attribute queries and replies to stock's reports.
pub async fn listen(
    config: &Path,
    wait: Duration,
    mut tick: impl FnMut(Listening),
    stop: impl Future<Output = Stop>,
) -> Result<Listened, Error> {
    let path = config.canonicalize().map_err(|source| Error::Read {
        path: config.to_path_buf(),
        source,
    })?;
    let mut server = Server::start(
        &path,
        server::Options {
            trial: OtaTrial::Disabled,
            enroll: None,
            clock: wellfed_stock::clock::Policy::Bounded,
            api: Api::Off,
        },
    )
    .map_err(|error| Error::StockServer(error.into()))?;

    let ended = tokio::select! {
        listened = watch(&server, wait, &mut tick) => Ok(listened),
        by = stop => Err(by),
    };
    if matches!(ended, Ok(None)) {
        let error = match server.finished().await {
            Ok(()) => "it stopped".to_string(),
            Err(error) => format!("{error:#}"),
        };
        return Err(Error::StockServerFailed(error));
    }
    server.stop().await;

    match ended {
        Ok(listened) => listened.ok_or_else(|| Error::StockServerFailed(String::new())),
        Err(by) => Err(Error::Stopped(Stopped {
            by,
            phase: wellfed_client::install::Phase::AwaitInstall,
            unsettled: None,
        })),
    }
}

/// What the listener saw; `None` when the server died, since a dead server
/// hears nothing and would look like a quiet feeder for the whole wait.
async fn watch(
    server: &Server,
    wait: Duration,
    tick: &mut impl FnMut(Listening),
) -> Option<Listened> {
    let started = Instant::now();
    let mut stock = false;
    loop {
        if server.is_finished() {
            return None;
        }
        let handle = server.handle();
        let status = handle.as_ref().and_then(|handle| handle.status().ok());

        let enrolled = status
            .as_ref()
            .and_then(|status| status.enrollment.as_ref())
            .map(|enrollment| SocketAddr::new(enrollment.address, PORT));
        if let Some(address) = enrolled
            && answers(address).await
        {
            return Some(Listened::Replacement { address });
        }

        if let (Some(handle), Some(status)) = (&handle, &status)
            && stock_stayed(status)
        {
            stock = true;
            if resolve(handle, status).await {
                return Some(Listened::StockStayed { resolved: true });
            }
        }

        let elapsed = started.elapsed();
        tick(Listening { elapsed, stock });
        if elapsed >= wait {
            return Some(if stock {
                Listened::StockStayed { resolved: false }
            } else {
                Listened::Silent
            });
        }
        tokio::time::sleep(POLL.min(wait.saturating_sub(elapsed))).await;
    }
}

async fn answers(address: SocketAddr) -> bool {
    matches!(
        tokio::time::timeout(CONNECT_WAIT, tokio::net::TcpStream::connect(address)).await,
        Ok(Ok(_))
    )
}

/// Whether stock reported the version it installs from after the latest
/// request went out.
fn stock_stayed(status: &StockStatus) -> bool {
    let Some(latest) = status.ota_trials.first() else {
        return false;
    };
    let version = status
        .attributes
        .get("softwareVersion")
        .and_then(|value| value.as_str());
    version == Some(ORIGINAL_VERSION)
        && status
            .version_reported_at
            .is_some_and(|reported| reported > latest.created_at)
}

/// Ask the ledger to close the latest request from that report. It decides:
/// it wants 20 minutes since the request and 10 quiet ones with stock
/// connected, and sends nothing.
async fn resolve(handle: &Handle, status: &StockStatus) -> bool {
    let Some(latest) = status.ota_trials.first() else {
        return false;
    };
    let input = ResolveInput {
        request_id: latest.input.request_id,
        note: "wellfed install: stock reported its version after the request".to_string(),
    };
    handle.resolve(input).await.is_ok()
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use serde_json::json;
    use wellfed_formats::stock::{Enrollment, TrialInput, TrialPhase, TrialRecord};

    use super::*;

    fn trial(at: i64) -> TrialRecord {
        let at = DateTime::from_timestamp(at, 0).unwrap();
        let id = uuid::Uuid::from_u128(7);
        TrialRecord {
            sequence: 1,
            input: TrialInput {
                request_id: id,
                server_token: id,
            },
            created_at: at,
            updated_at: at,
            phase: TrialPhase::Uncertain,
            download_reported: false,
            resolution_note: None,
            image_sha256: String::new(),
            build: None,
            stock_reported: false,
        }
    }

    /// Only a version report after the request says stock stayed: the one
    /// it sent before the request says nothing about what came of it.
    #[test]
    fn stock_stayed_only_on_a_report_after_the_request() {
        let mut status = StockStatus {
            ota_trials: vec![trial(100)],
            ..StockStatus::default()
        };
        assert!(!stock_stayed(&status));

        status
            .attributes
            .insert("softwareVersion".into(), json!(ORIGINAL_VERSION));
        status.version_reported_at = DateTime::from_timestamp(50, 0);
        assert!(!stock_stayed(&status));
        status.version_reported_at = DateTime::from_timestamp(150, 0);
        assert!(stock_stayed(&status));

        status
            .attributes
            .insert("softwareVersion".into(), json!("1.1.14"));
        assert!(!stock_stayed(&status));
    }

    fn installer(dir: &Path, host: [u8; 4]) -> std::path::PathBuf {
        let path = dir.join("installer.json");
        // Below the ephemeral range, on a loopback address of its own.
        let listen = SocketAddr::from((host, 21_883));
        wellfed_stock::config::init_installer(&path, listen, chrono_tz::UTC).unwrap();
        path
    }

    /// With no feeder enrolled there is nothing to listen for.
    #[tokio::test(flavor = "current_thread")]
    async fn a_config_with_no_enrollment_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = installer(dir.path(), [127, 41, 7, 1]);
        let error = listen(&path, Duration::ZERO, |_| {}, std::future::pending())
            .await
            .unwrap_err();
        assert!(matches!(error, Error::StockServer(_)), "{error:?}");
    }

    fn enroll(config: &Path, device: &str, address: &str) {
        let record = Enrollment {
            client_id: device.into(),
            username: "stock".into(),
            password: "its-password".into(),
            address: address.parse().unwrap(),
            enrolled_at: DateTime::from_timestamp(1, 0).unwrap(),
        };
        wellfed_stock::enroll::create(&record, &Enrollment::path(config)).unwrap();
    }

    /// A request for `device` that went out 30 minutes ago, unsettled.
    fn open_trial(config: &Path, device: &str) {
        let id = uuid::Uuid::new_v4();
        let sent = chrono::Utc::now() - chrono::Duration::minutes(30);
        let ledger = json!({
            "device_id": device,
            "ota_trials": {
                id.to_string(): {
                    "sequence": 1,
                    "input": {"request_id": id, "server_token": uuid::Uuid::new_v4()},
                    "created_at": sent, "updated_at": sent,
                    "phase": "uncertain", "download_reported": false,
                    "resolution_note": null, "image_sha256": "ab".repeat(32),
                }
            },
        });
        let path = wellfed_stock::controller::ledger_path(config, device).unwrap();
        wellfed_stock::storage::write_json(&path, &ledger).unwrap();
    }

    /// A stock feeder on the broker: it reports its version over and over,
    /// and every command it hears goes to `heard`.
    fn stock_client(
        listen: SocketAddr,
        device: &str,
        heard: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) -> tokio::task::JoinHandle<()> {
        use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};

        let mut options = MqttOptions::new(device, listen.ip().to_string(), listen.port());
        options.set_credentials("stock", "its-password");
        options.set_clean_session(true);
        let (client, mut events) = AsyncClient::new(options, 32);
        let (subscribe, report) = (
            format!("dl/PLAF103/{device}/device/+/sub"),
            format!("dl/PLAF103/{device}/device/event/post"),
        );
        tokio::spawn(async move {
            let mut beat = tokio::time::interval(Duration::from_millis(200));
            loop {
                tokio::select! {
                    event = events.poll() => match event {
                        Ok(Event::Incoming(Packet::ConnAck(_))) => {
                            let _ = client.subscribe(&subscribe, QoS::AtLeastOnce).await;
                        }
                        Ok(Event::Incoming(Packet::Publish(publish))) => {
                            let message: serde_json::Value =
                                serde_json::from_slice(&publish.payload).unwrap_or_default();
                            let command = message["cmd"].as_str().unwrap_or_default().to_string();
                            heard.lock().unwrap().push(command);
                        }
                        Ok(_) => {}
                        Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
                    },
                    _ = beat.tick() => {
                        let body = json!({
                            "cmd": "DEVICE_START_EVENT",
                            "ts": chrono::Utc::now().timestamp_millis(),
                            "softwareVersion": ORIGINAL_VERSION,
                            "powerType": 1,
                        });
                        let _ = client.try_publish(&report, QoS::AtMostOnce, false, body.to_string());
                    }
                }
            }
        })
    }

    /// Stock coming back says it stayed. Its ledger will not close the request
    /// without 10 quiet minutes, so the end says that too, and a connected
    /// stock has heard only read-only queries and replies to its own reports.
    #[tokio::test(flavor = "current_thread")]
    async fn stock_coming_back_says_it_stayed_and_hears_no_install() {
        let dir = tempfile::tempdir().unwrap();
        let listen = SocketAddr::from(([127, 41, 7, 5], 21_883));
        let path = installer(dir.path(), [127, 41, 7, 5]);
        enroll(&path, "AF_TEST", "127.41.7.6");
        open_trial(&path, "AF_TEST");

        let heard = std::sync::Arc::default();
        let client = stock_client(listen, "AF_TEST", std::sync::Arc::clone(&heard));
        let mut saw_stock = false;
        let listened = listen_for(&path, Duration::from_secs(8), &mut saw_stock).await;
        client.abort();

        assert_eq!(listened.unwrap(), Listened::StockStayed { resolved: false });
        assert!(saw_stock);
        let heard = heard.lock().unwrap();
        assert!(!heard.is_empty(), "the server's session reached stock");
        for command in heard.iter() {
            assert!(
                matches!(command.as_str(), "ATTR_GET_SERVICE" | "DEVICE_START_EVENT"),
                "stock heard {command}"
            );
        }
    }

    async fn listen_for(
        path: &Path,
        wait: Duration,
        saw_stock: &mut bool,
    ) -> Result<Listened, Error> {
        listen(
            path,
            wait,
            |tick| *saw_stock |= tick.stock,
            std::future::pending(),
        )
        .await
    }

    /// The device API answering where stock connected from says it installed.
    #[tokio::test(flavor = "current_thread")]
    async fn the_device_api_answering_says_it_installed() {
        let dir = tempfile::tempdir().unwrap();
        let path = installer(dir.path(), [127, 41, 7, 7]);
        enroll(&path, "AF_TEST", "127.41.7.8");
        let api = tokio::net::TcpListener::bind(("127.41.7.8", PORT))
            .await
            .unwrap();

        let listened = listen(&path, UNCERTAIN_WAIT, |_| {}, std::future::pending())
            .await
            .unwrap();
        assert_eq!(
            listened,
            Listened::Replacement {
                address: api.local_addr().unwrap()
            }
        );
    }

    /// A server that cannot run is not a quiet feeder: its broker's port is
    /// taken, and the listener says so instead of counting to 20 minutes.
    #[tokio::test(flavor = "current_thread")]
    async fn a_dead_server_ends_the_wait() {
        let dir = tempfile::tempdir().unwrap();
        let path = installer(dir.path(), [127, 41, 7, 9]);
        enroll(&path, "AF_TEST", "127.41.7.10");
        let _taken = tokio::net::TcpListener::bind(("127.41.7.9", 21_883))
            .await
            .unwrap();

        let error = listen(&path, UNCERTAIN_WAIT, |_| {}, std::future::pending())
            .await
            .unwrap_err();
        assert!(matches!(error, Error::StockServerFailed(_)), "{error:?}");
    }

    /// A feeder that never comes back leaves it unknown, and a stop ends
    /// the wait at once. The server runs with no trial enabled, so nothing
    /// in it could send an install.
    #[tokio::test(flavor = "current_thread")]
    async fn a_silent_feeder_leaves_it_unknown_and_a_stop_ends_the_wait() {
        let dir = tempfile::tempdir().unwrap();
        let path = installer(dir.path(), [127, 41, 7, 3]);
        enroll(&path, "AF_TEST", "127.41.7.4");

        let mut ticks = Vec::new();
        let listened = listen(
            &path,
            Duration::from_millis(100),
            |tick| ticks.push(tick),
            std::future::pending(),
        )
        .await
        .unwrap();
        assert_eq!(listened, Listened::Silent);
        assert!(!ticks.is_empty());
        assert!(ticks.iter().all(|tick| !tick.stock));

        let error = listen(
            &path,
            UNCERTAIN_WAIT,
            |_| {},
            std::future::ready(Stop::Terminate),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, Error::Stopped(_)), "{error:?}");
    }
}
