//! Listening for a feeder after an install request whose outcome is not
//! known: the request may have reached stock, so it is never sent again,
//! and only what the feeder does next can say what became of it.
//!
//! The listener runs stock's server with the trial disabled, so nothing in
//! it can send an install, and watches for up to [`UNCERTAIN_WAIT`]: stock
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
/// says what became of the request or `stop` resolves. Sends the feeder
/// nothing, whatever it sees.
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
    let server = Server::start(
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
    server.stop().await;

    ended.map_err(|by| {
        Error::Stopped(Stopped {
            by,
            phase: wellfed_client::install::Phase::AwaitInstall,
            unsettled: None,
        })
    })
}

async fn watch(server: &Server, wait: Duration, tick: &mut impl FnMut(Listening)) -> Listened {
    let started = Instant::now();
    let mut stock = false;
    loop {
        let handle = server.handle();
        let status = handle.as_ref().and_then(|handle| handle.status().ok());

        let enrolled = status
            .as_ref()
            .and_then(|status| status.enrollment.as_ref())
            .map(|enrollment| SocketAddr::new(enrollment.address, PORT));
        if let Some(address) = enrolled
            && answers(address).await
        {
            return Listened::Replacement { address };
        }

        if let (Some(handle), Some(status)) = (&handle, &status)
            && stock_stayed(status)
        {
            stock = true;
            if resolve(handle, status).await {
                return Listened::StockStayed { resolved: true };
            }
        }

        let elapsed = started.elapsed();
        tick(Listening { elapsed, stock });
        if elapsed >= wait {
            return if stock {
                Listened::StockStayed { resolved: false }
            } else {
                Listened::Silent
            };
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

    /// A feeder that never comes back leaves it unknown, and a stop ends
    /// the wait at once. The server runs with no trial enabled, so nothing
    /// in it could send an install.
    #[tokio::test(flavor = "current_thread")]
    async fn a_silent_feeder_leaves_it_unknown_and_a_stop_ends_the_wait() {
        let dir = tempfile::tempdir().unwrap();
        let path = installer(dir.path(), [127, 41, 7, 3]);
        let record = Enrollment {
            client_id: "AF_TEST".into(),
            username: "stock".into(),
            password: "its-password".into(),
            address: "127.41.7.4".parse().unwrap(),
            enrolled_at: DateTime::from_timestamp(1, 0).unwrap(),
        };
        wellfed_stock::enroll::create(&record, &Enrollment::path(&path)).unwrap();

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
