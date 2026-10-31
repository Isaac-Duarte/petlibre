//! Stock's side of the run, in this process: `wellfed-stock`'s server (its
//! broker trusting the first feeder that connects, and its session with
//! stock) started once, before stock is pointed at it, and its image server
//! started for the one request. Both stop when the run ends, however it
//! ends, so the broker is never left on the MQTT port.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use wellfed_client::install::{FeederTrust, StockPower, StockView};
use wellfed_formats::stock::{Enrollment, ImageLease, OtaReport, StockStatus};
use wellfed_stock::clock::StockPlans;
use wellfed_stock::ota::image::{self, Images};
use wellfed_stock::ota::trial::{AnsweredInput, Input, Record};
use wellfed_stock::server::{self, Api, Handle, NoFeederEnrolled, OtaTrial, Refusal, Server};

use crate::Error;

/// How long the image server gets to let a transfer finish once the run
/// ends.
const IMAGES_DRAIN: Duration = Duration::from_secs(5);

/// How the run starts stock's server.
#[derive(Debug, Clone)]
pub struct StockOptions {
    /// `wellfed-stock`'s config: an installer's (`init-installer`), with
    /// no plan snapshot. The enrollment, the trial ledger and the image
    /// lease are kept beside it.
    pub config: PathBuf,
    /// How long the broker trusts the first stock feeder that connects.
    pub enroll: Duration,
    /// Whether the server also answers its loopback API, for the bench's
    /// page to read.
    pub api: Api,
    /// Which address the broker listens on.
    pub bind: server::Bind,
    /// What is known of stock's own plans: `Erased` only when this run's frame
    /// followed the Reset hold, or resumes one that did.
    pub stock_plans: StockPlans,
}

/// The installer's config at `path`, for a broker at `listen`: created once,
/// with fresh secrets and `zone` (an IANA name, UTC if it is none we know),
/// and kept, since the secrets and the records beside it outlive a run. The
/// platform's address may have changed since (a new DHCP lease), so the
/// broker's is written each time.
pub fn installer_config(path: &Path, listen: SocketAddr, zone: Option<&str>) -> Result<(), Error> {
    if !path.exists() {
        let zone = zone
            .and_then(|zone| zone.parse().ok())
            .unwrap_or(chrono_tz::UTC);
        return wellfed_stock::config::init_installer(path, listen, zone)
            .map_err(|error| Error::StockServer(error.into()));
    }

    let text = std::fs::read(path).map_err(|source| Error::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let mut config: wellfed_stock::config::InstallConfig =
        serde_json::from_slice(&text).map_err(|source| Error::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    if config.mqtt_listen == listen {
        return Ok(());
    }
    config.mqtt_listen = listen;
    wellfed_stock::storage::write_json(path, &config)
        .map_err(|error| Error::StockServer(error.into()))
}

/// Forget the feeder the broker beside `config` trusted, and any contest: the
/// next feeder that connects is asked about from the start.
pub fn forget_enrollment(config: &Path) -> Result<(), Error> {
    for path in [Enrollment::path(config), Enrollment::contested_path(config)] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(Error::Remove { path, source }),
        }
    }
    Ok(())
}

/// Where the ledger of `device`, by the installer's config, lives.
pub fn ledger_path(config: &Path, device: &str) -> Result<PathBuf, Error> {
    wellfed_stock::controller::ledger_path(config, device)
        .map_err(|error| Error::StockServer(error.into()))
}

/// Whether the feeder the broker beside `config` trusts has a request whose
/// outcome is not settled. An unreadable record counts as open: only the
/// server's own check may rule a request out.
pub fn open_request(config: &Path) -> bool {
    config.exists() && wellfed_stock::controller::open_request(config).unwrap_or(true)
}

/// The feeder with `identity` just answered with `build` in a proven session:
/// remember it, for the feeder's stock device, and close that device's trial if
/// this is the build it installed. A feeder this phone never paired from stock
/// is no evidence of any trial. Whether a trial closed; fails while a server
/// owns the ledger, with the answer kept for [`settle_answered`].
pub fn feeder_answered(
    config: &Path,
    backups: &Path,
    identity: &[u8; 32],
    build: &str,
) -> Result<bool, Error> {
    let paired = crate::storage::Paired::load(backups)?
        .filter(|record| record.identity.0 == *identity)
        .map(|record| record.device);
    let known = crate::storage::Answered::load(backups, identity)?.map(|answered| answered.device);
    let Some(device) = paired.or(known) else {
        return Ok(false);
    };

    let sequence = wellfed_stock::controller::latest_sequence(config, &device)
        .map_err(|error| Error::StockServer(error.into()))?;
    crate::storage::Answered {
        device,
        build: build.to_string(),
        sequence,
    }
    .keep(backups, identity)?;
    settle_answered(config, backups, identity)
}

/// Close the trial the feeder with `identity` last answered about, if it is
/// still open: at the end of a run, whose server held the ledger, and at a
/// restore's activation.
pub fn settle_answered(config: &Path, backups: &Path, identity: &[u8; 32]) -> Result<bool, Error> {
    let Some(answered) = crate::storage::Answered::load(backups, identity)? else {
        return Ok(false);
    };
    let Some(sequence) = answered.sequence else {
        return Ok(false);
    };
    let note = format!(
        "build {} answered from this feeder's key",
        answered.build.get(..12).unwrap_or(&answered.build)
    );
    wellfed_stock::controller::close_answered(
        config,
        &answered.device,
        &answered.build,
        sequence,
        note,
    )
    .map_err(|error| Error::StockServer(error.into()))
}

/// A restore activated stock on the feeder with `identity`: settle the trial it
/// answered about, and say stock is coming back, so its next report does not
/// reopen that trial.
pub fn feeder_restored(config: &Path, backups: &Path, identity: &[u8; 32]) -> Result<(), Error> {
    settle_answered(config, backups, identity)?;
    let Some(answered) = crate::storage::Answered::load(backups, identity)? else {
        return Ok(());
    };
    if let Some(sequence) = answered.sequence {
        wellfed_stock::controller::stock_restored(config, &answered.device, sequence)
            .map_err(|error| Error::StockServer(error.into()))?;
    }
    Ok(())
}

/// Why the server's status is not there to read.
pub(crate) enum Unready {
    /// No feeder has enrolled, so the session with stock has not started.
    NoFeeder,
    /// The enrollment window closed with no feeder.
    WindowClosed(String),
    /// The server failed, with its error.
    Failed(String),
    /// The enrollment record beside the config cannot be read.
    Unreadable(String),
}

/// How a server that ended on its own ended.
#[derive(Clone)]
enum Ended {
    WindowClosed(String),
    Failed(String),
}

impl From<Ended> for Unready {
    fn from(ended: Ended) -> Self {
        match ended {
            Ended::WindowClosed(error) => Self::WindowClosed(error),
            Ended::Failed(error) => Self::Failed(error),
        }
    }
}

pub(crate) struct Stock {
    options: StockOptions,
    server: Option<Server>,
    images: Option<Images>,
    /// Why it ended, once it ended on its own.
    ended: Option<Ended>,
}

impl Stock {
    pub(crate) fn new(options: StockOptions) -> Self {
        Self {
            options,
            server: None,
            images: None,
            ended: None,
        }
    }

    pub(crate) const fn serving(&self) -> bool {
        self.server.is_some()
    }

    pub(crate) fn config(&self) -> &Path {
        &self.options.config
    }

    pub(crate) fn enroll(&self) -> Duration {
        self.options.enroll
    }

    /// Start the server, with the trial enabled and the enrollment window
    /// open; once only. Whether it started now.
    pub(crate) fn start(&mut self) -> Result<bool, Error> {
        if self.server.is_some() || self.ended.is_some() {
            return Ok(false);
        }
        let path = self
            .options
            .config
            .canonicalize()
            .map_err(|source| Error::Read {
                path: self.options.config.clone(),
                source,
            })?;
        let server = Server::start(
            &path,
            server::Options {
                trial: OtaTrial::Enabled,
                enroll: Some(self.options.enroll),
                api: self.options.api,
                bind: self.options.bind,
                stock_plans: self.options.stock_plans,
            },
        )
        .map_err(|error| Error::StockServer(error.into()))?;
        self.server = Some(server);
        Ok(true)
    }

    /// The server's status, or why it is not there.
    pub(crate) async fn status(&mut self) -> Result<StockStatus, Unready> {
        if let Some(ended) = &self.ended {
            return Err(ended.clone().into());
        }
        let Some(server) = &mut self.server else {
            return Err(Unready::NoFeeder);
        };
        if server.is_finished() {
            let ended = match server.finished().await {
                Ok(()) => Ended::Failed("it stopped".to_string()),
                Err(error) if error.downcast_ref::<NoFeederEnrolled>().is_some() => {
                    Ended::WindowClosed(format!("{error:#}"))
                }
                Err(error) => Ended::Failed(format!("{error:#}")),
            };
            self.server = None;
            self.ended = Some(ended.clone());
            return Err(ended.into());
        }
        let handle = server.handle().ok_or(Unready::NoFeeder)?;
        handle
            .status()
            .map_err(|error| Unready::Unreadable(error.to_string()))
    }

    fn handle(&self) -> Result<Handle, Error> {
        self.server
            .as_ref()
            .and_then(Server::handle)
            .ok_or(Error::StockServerGone)
    }

    /// Serve `review` to the enrolled feeder, at the address its enrollment
    /// recorded (with the `bench` feature, a captured device is found on
    /// the hotspot instead), and the lease that names it.
    pub(crate) async fn images(&mut self, review: &Path) -> Result<ImageLease, Error> {
        let path = self
            .options
            .config
            .canonicalize()
            .map_err(|source| Error::Read {
                path: self.options.config.clone(),
                source,
            })?;
        let config =
            wellfed_stock::config::load(&path).map_err(|error| Error::StockServer(error.into()))?;
        let peer = peer(&config)?;
        let images = image::start(&config, &path, review, peer)
            .await
            .map_err(|error| Error::Images(error.into()))?;
        let lease = images.lease.clone();
        self.images = Some(images);
        Ok(lease)
    }

    /// Ask stock once to install the served build. Not retried here.
    pub(crate) async fn trial(&self, input: Input) -> Result<Record, Refusal> {
        match self.handle() {
            Ok(handle) => handle.trial(input).await,
            Err(_) => Err(Refusal::Unanswered),
        }
    }

    /// Tell the server the served build answered from bank 0.
    pub(crate) async fn answered(&self, input: AnsweredInput) -> Result<Record, Error> {
        Ok(self.handle()?.answered(input).await?)
    }

    /// Where the feeder connected from, as the image lease beside the
    /// config names it (a resumed run may find none: the image server
    /// removes it when it stops).
    pub(crate) fn leased_peer(&self) -> Option<IpAddr> {
        let lease: ImageLease =
            wellfed_formats::load(&ImageLease::path(&self.options.config)).ok()?;
        Some(IpAddr::V4(lease.peer))
    }

    /// Where the enrolled feeder first connected from, for a run that has
    /// no image lease left to read it from.
    pub(crate) fn enrolled_peer(&self) -> Option<IpAddr> {
        let path = Enrollment::path(&self.options.config);
        let record = Enrollment::load(&path).ok()??;
        Some(record.address)
    }

    /// The stock device this install's broker enrolled (trust on first
    /// use), from its record beside the config; `None` for a captured
    /// device or before stock enrolled.
    pub(crate) fn device(&self) -> Result<Option<String>, Error> {
        let path = Enrollment::path(&self.options.config);
        let record = Enrollment::load(&path)
            .map_err(|source| Error::EnrollmentUnreadable { path, source })?;
        Ok(record.map(|record| record.client_id))
    }

    /// Stop the image server, then the server: a transfer under way gets a
    /// bounded drain, and the broker its own bounded shutdown.
    pub(crate) async fn stop(&mut self) {
        if let Some(images) = self.images.take() {
            images.stop(IMAGES_DRAIN).await;
        }
        if let Some(server) = self.server.take() {
            server.stop().await;
        }
    }
}

/// Where the image server admits the feeder from: the address the
/// enrollment recorded. A captured device has none; on the bench host it is
/// found by its MAC in the hotspot's neighbour table, as `ota-images` finds
/// it. Without the `bench` feature (an app) the enrollment is the only
/// source, and a captured device is refused.
fn peer(config: &wellfed_stock::config::Config) -> Result<Ipv4Addr, Error> {
    if let Some(IpAddr::V4(enrolled)) = config.device_address {
        return Ok(enrolled);
    }
    #[cfg(feature = "bench")]
    {
        match wellfed_stock::neighbour::discover(config) {
            Ok(IpAddr::V4(found)) => Ok(found),
            Ok(IpAddr::V6(_)) => Err(Error::NoPeer),
            Err(error) => Err(Error::Images(error.into())),
        }
    }
    #[cfg(not(feature = "bench"))]
    Err(Error::NoPeer)
}

/// Stock's latest OTA report on request `id`: one naming it, or naming
/// none, received since the request was recorded. The server keeps reports
/// from earlier requests too.
pub(crate) fn latest_report<'a>(status: &'a StockStatus, id: &str) -> Option<&'a OtaReport> {
    let since = status.trial(id)?.created_at;
    status.ota_reports.iter().rev().find(|report| {
        report.received_at >= since && report.message_id.as_deref().is_none_or(|m| m == id)
    })
}

/// The steps' view of stock, from the server's status document.
pub(crate) fn view(status: &StockStatus) -> StockView {
    let attribute = |name: &str| status.attributes.get(name);
    StockView {
        online: status.device_online,
        version: status
            .version_reported_at
            .and(attribute("softwareVersion"))
            .and_then(|value| value.as_str())
            .map(str::to_string),
        power: attribute("powerType")
            .and_then(serde_json::Value::as_u64)
            .map(StockPower::from_type),
        trust: match &status.enrollment {
            None => FeederTrust::Captured,
            Some(enrollment) if enrollment.contested => FeederTrust::Contested,
            Some(enrollment) => FeederTrust::Enrolled {
                device: enrollment.device.clone(),
                address: enrollment.address.to_string(),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wellfed_formats::stock::{
        OtaCommand, StockEnrollment, TrialInput, TrialPhase, TrialRecord,
    };

    use super::*;

    /// The config's secrets outlive a run, and its broker follows the
    /// platform's address.
    #[test]
    fn the_installers_config_is_kept_and_follows_the_broker() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("installer.json");
        let read = || -> wellfed_stock::config::InstallConfig {
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap()
        };

        let one: SocketAddr = "192.168.1.9:1883".parse().unwrap();
        installer_config(&path, one, Some("Not/AZone")).unwrap();
        let first = read();
        assert_eq!(first.mqtt_listen, one);
        assert_eq!(first.timezone, chrono_tz::UTC);

        let other: SocketAddr = "192.168.1.10:1883".parse().unwrap();
        installer_config(&path, other, None).unwrap();
        let second = read();
        assert_eq!(second.mqtt_listen, other);
        assert_eq!(second.server_password, first.server_password);
    }

    /// Forgetting a feeder does not lock the next one out, and does not let
    /// the forgotten one's open request go: each device has its own ledger.
    #[tokio::test(flavor = "current_thread")]
    async fn each_device_keeps_its_own_ledger_across_forgetting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("installer.json");
        let listen: SocketAddr = "127.41.6.1:21883".parse().unwrap();
        installer_config(&path, listen, None).unwrap();
        let path = path.canonicalize().unwrap();
        assert!(!open_request(&path));

        let enroll = |device: &str| {
            let record = Enrollment {
                client_id: device.into(),
                username: "stock".into(),
                password: "its-password".into(),
                address: "127.41.6.2".parse().unwrap(),
                enrolled_at: chrono::DateTime::UNIX_EPOCH,
            };
            wellfed_stock::enroll::create(&record, &Enrollment::path(&path)).unwrap();
        };
        let serve = async |device: &str| {
            let server = Server::start(
                &path,
                server::Options {
                    trial: OtaTrial::Disabled,
                    enroll: None,
                    api: Api::Off,
                    bind: server::Bind::Configured,
                    stock_plans: StockPlans::Unknown,
                },
            )
            .unwrap();
            for _ in 0..100 {
                if server.handle().is_some() || server.is_finished() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let status = server.handle().expect(device).status().unwrap();
            server.stop().await;
            status.ota_trials.first().map(|trial| trial.phase)
        };

        // The first feeder's request went unanswered.
        enroll("AF_FIRST");
        let id = uuid::Uuid::new_v4();
        let at = chrono::DateTime::UNIX_EPOCH;
        let ledger = json!({
            "device_id": "AF_FIRST",
            "ota_trials": {
                id.to_string(): {
                    "sequence": 1,
                    "input": {"request_id": id, "server_token": id},
                    "created_at": at, "updated_at": at,
                    "phase": "submitted", "download_reported": false,
                    "resolution_note": null, "image_sha256": "ab".repeat(32),
                }
            },
        });
        let first = wellfed_stock::controller::ledger_path(&path, "AF_FIRST").unwrap();
        wellfed_stock::storage::write_json(&first, &ledger).unwrap();
        assert!(open_request(&path));

        // The owner said it was not theirs: another feeder may come.
        forget_enrollment(&path).unwrap();
        assert!(!open_request(&path));
        enroll("AF_SECOND");
        assert_eq!(serve("AF_SECOND").await, None);
        assert!(!open_request(&path));

        // The first one back: its request is still open, and uncertain now
        // that the server that sent it is gone.
        forget_enrollment(&path).unwrap();
        enroll("AF_FIRST");
        assert_eq!(serve("AF_FIRST").await, Some(TrialPhase::Uncertain));
        assert!(open_request(&path));
    }

    /// Forgetting the broker's trust takes the contest with it, and is safe
    /// to ask for with nothing to forget.
    #[test]
    fn forgetting_the_enrollment_takes_the_contest_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("installer.json");
        std::fs::write(Enrollment::path(&config), "{}").unwrap();
        std::fs::write(Enrollment::contested_path(&config), "{}").unwrap();
        forget_enrollment(&config).unwrap();
        assert!(!Enrollment::path(&config).exists());
        assert!(!Enrollment::contested_path(&config).exists());
        forget_enrollment(&config).unwrap();
    }

    #[test]
    fn the_server_s_document_becomes_the_steps_view() {
        let mut status = StockStatus {
            device_online: true,
            ..StockStatus::default()
        };
        status
            .attributes
            .insert("softwareVersion".into(), json!("1.1.15"));
        status.attributes.insert("powerType".into(), json!(1));

        // A version the server has not seen reported is no version.
        let stale = view(&status);
        assert_eq!(stale.version, None);
        assert_eq!(stale.power, Some(StockPower::Adapter));

        status.version_reported_at = Some(chrono::DateTime::UNIX_EPOCH);
        let fresh = view(&status);
        assert_eq!(fresh.version.as_deref(), Some("1.1.15"));
        assert!(fresh.online);
        assert_eq!(fresh.trust, FeederTrust::Captured);

        let mut enrollment = StockEnrollment {
            device: "AF_NEW".into(),
            address: "192.168.4.20".parse().unwrap(),
            contested: false,
        };
        status.enrollment = Some(enrollment.clone());
        assert_eq!(
            view(&status).trust,
            FeederTrust::Enrolled {
                device: "AF_NEW".into(),
                address: "192.168.4.20".into()
            }
        );
        enrollment.contested = true;
        status.enrollment = Some(enrollment);
        assert_eq!(view(&status).trust, FeederTrust::Contested);
    }

    #[test]
    fn progress_is_read_only_from_this_request_s_reports() {
        let id = uuid::Uuid::from_u128(7);
        let at = |seconds| chrono::DateTime::from_timestamp(seconds, 0).unwrap();
        let report = |seconds, message: Option<&str>, progress| OtaReport {
            received_at: at(seconds),
            command: OtaCommand::Progress,
            message_id: message.map(str::to_string),
            phase: None,
            state: None,
            progress: Some(progress),
            code: None,
            error_code: None,
        };
        let status = StockStatus {
            ota_trials: vec![TrialRecord {
                sequence: 1,
                input: TrialInput {
                    request_id: id,
                    server_token: id,
                },
                created_at: at(100),
                updated_at: at(100),
                phase: TrialPhase::Submitted,
                download_reported: false,
                resolution_note: None,
                image_sha256: String::new(),
                build: None,
                stock_reported: false,
                answered_at: None,
                restored_at: None,
            }],
            ota_reports: vec![
                report(150, Some(&id.to_string()), 40),
                // An earlier request's, and one from before this request.
                report(160, Some("other"), 90),
                report(50, None, 80),
            ],
            ..StockStatus::default()
        };
        let latest = latest_report(&status, &id.to_string()).unwrap();
        assert_eq!(latest.progress, Some(40));
        assert!(latest_report(&status, "unknown").is_none());
    }
}
