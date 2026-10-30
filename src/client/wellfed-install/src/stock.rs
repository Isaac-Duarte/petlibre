//! Stock's side of the run, in this process: `wellfed-stock`'s server (its
//! broker trusting the first feeder that connects, and its session with
//! stock) started once, before stock is pointed at it, and its image server
//! started for the one request. Both stop when the run ends, however it
//! ends, so the broker is never left on the MQTT port.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use wellfed_client::install::{FeederTrust, StockPower, StockView};
use wellfed_formats::stock::{Enrollment, ImageLease, OtaReport, StockStatus};
use wellfed_stock::clock;
use wellfed_stock::ota::image::{self, Images};
use wellfed_stock::ota::trial::{AnsweredInput, Input, Record};
use wellfed_stock::server::{self, Api, Handle, OtaTrial, Refusal, Server};

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
    /// Whether stock's clock is set even when its own reading is unknown
    /// or far off ([`clock::Policy`]).
    pub clock: clock::Policy,
    /// Whether the server also answers its loopback API, for the bench's
    /// page to read.
    pub api: Api,
}

/// Why the server's status is not there to read.
pub(crate) enum Unready {
    /// No feeder has enrolled, so the session with stock has not started.
    NoFeeder,
    /// The server ended on its own (its enrollment window closed, or it
    /// failed), with its error.
    Ended(String),
    /// The enrollment record beside the config cannot be read.
    Unreadable(String),
}

pub(crate) struct Stock {
    options: StockOptions,
    server: Option<Server>,
    images: Option<Images>,
    /// The server's error once it ended on its own.
    ended: Option<String>,
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
                clock: self.options.clock,
                api: self.options.api,
            },
        )
        .map_err(|error| Error::StockServer(error.into()))?;
        self.server = Some(server);
        Ok(true)
    }

    /// The server's status, or why it is not there.
    pub(crate) async fn status(&mut self) -> Result<StockStatus, Unready> {
        if let Some(error) = &self.ended {
            return Err(Unready::Ended(error.clone()));
        }
        let Some(server) = &mut self.server else {
            return Err(Unready::NoFeeder);
        };
        if server.is_finished() {
            let error = match server.finished().await {
                Ok(()) => "it stopped".to_string(),
                Err(error) => format!("{error:#}"),
            };
            self.server = None;
            self.ended = Some(error.clone());
            return Err(Unready::Ended(error));
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

    /// Serve `review` to the enrolled feeder, and the lease that names it.
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
        let Some(IpAddr::V4(peer)) = config.device_address else {
            return Err(Error::NoPeer);
        };
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
