//! Stock device state and durable, deliberately non-retrying install trials.
//!
//! The MQTT worker owns the one `Controller`, which holds an exclusive lock
//! on the ledger of stock-to-replacement install trials. Each trial is
//! persisted before its `OTA_UPGRADE` is handed to MQTT and is never
//! replayed: a repeated request ID returns the stored record without a second
//! command, and loading the ledger or losing the broker connection marks an
//! in-flight trial uncertain. Stock is only installed from and rolled back to
//! (docs/development/tools-roadmap.md), so this server never feeds.

use std::{collections::BTreeMap, fs, path::PathBuf};

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use wellfed_formats::local::LocalStatus;

use crate::{
    build_info::Build,
    config::Config,
    ota::{
        self,
        report::{self, REPORT_LIMIT},
        trial::{ORIGINAL_VERSION, Phase as TrialPhase, REPLACEMENT_VERSION},
    },
    protocol::{Channel, Message, Reply},
    storage,
};

/// Freshness required for version/power evidence before an OTA decision.
const EVIDENCE_SECONDS: i64 = 300;

/// A new request needs this much image-server accept window left. The stock
/// task waits up to 120 s for Wi-Fi plus 7 s before connecting.
const MIN_LEASE_SECONDS: i64 = 5 * 60;

/// Minimum wait covering the image server's 15-minute accept window and
/// five-minute connection limit; it is not a device-side idle acknowledgement.
const RESOLVE_AFTER_SECONDS: i64 = 20 * 60;

/// Resolution also needs an observed connection without OTA reports.
const QUIET_SECONDS: i64 = 10 * 60;

/// Missing feeder telemetry breaks the observation interval even when the
/// server remains connected to its own broker. Stock 1.1.15 heartbeats about
/// every 61 s (physical, fw40-stock-ota-03), so a 60 s bound restarted the
/// interval at every heartbeat and an idle feeder could never be resolved;
/// 90 s tolerates one late heartbeat but not a missed one.
const TELEMETRY_SECONDS: i64 = 90;

/// The trial ledger, persisted as [`LEDGER_FILE`] beside the config.
#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Ledger {
    device_id: String,
    ota_trials: BTreeMap<Uuid, ota::trial::Record>,
}

/// The ledger's file name beside the config.
pub const LEDGER_FILE: &str = "stock-ledger.json";

/// The retired ledger, which also held feeds and probe queries. It is
/// never read: an operator archives it after checking no trial in it is open.
pub const RETIRED_LEDGER_FILE: &str = "feed-ledger.json";

/// The status document `GET /api/status` serves.
pub type Status = LocalStatus;

fn reported_version(status: &Status) -> Option<&str> {
    status
        .attributes
        .get("softwareVersion")
        .and_then(Value::as_str)
}

fn power_type(status: &Status) -> Option<u64> {
    status.attributes.get("powerType").and_then(Value::as_u64)
}

/// Why an operation was refused; the API returns the message as its error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Rejection {
    #[error("device is offline or telemetry is stale")]
    Offline,
    #[error("request ID conflicts with a different operation")]
    Conflict,
    #[error("operation ledger is full; archive it explicitly before continuing")]
    Full,
    #[error("persistent storage failed; command was not authorized")]
    Storage,
    #[error("restart serve with --enable-ota-trial to allow the fixed experiment")]
    OtaDisabled,
    #[error(
        "trial requires valid IDs, recent original-version and external-power reports and no plans"
    )]
    InvalidOta,
    #[error("an install trial is active, uncertain or failed; resolve it first")]
    OtaActive,
    #[error(
        "image server is not listening with this token, or its accept window ends within five minutes"
    )]
    ImageServer,
    #[error(
        "resolution needs the latest uncertain/failed trial, 20 minutes since submission, 10 quiet connected minutes without OTA reports and a fresh source- or target-version report"
    )]
    ResolutionEvidence,
    #[error("only the latest submitted or uncertain trial closes on the replacement's answer")]
    NotOpen,
}

pub struct Controller {
    pub config: Config,
    pub status: Status,
    path: PathBuf,
    ledger: Ledger,
    storage_failed: bool,
    /// Start of the current interval with fresh feeder telemetry and no OTA report.
    ota_quiet_since: DateTime<Utc>,
    _lease: fs::File,
}

impl Controller {
    pub fn load(config: Config, path: PathBuf, build: Build) -> Result<Self> {
        let retired = path.with_file_name(RETIRED_LEDGER_FILE);
        ensure!(
            !retired.exists(),
            "{} is the retired ledger, which held feeds and probe queries; check that no trial in it is submitted, uncertain or failed, then archive it (rename it) and start again",
            retired.display()
        );

        let lease = storage::lock(&path)?;
        let mut ledger = if path.exists() {
            serde_json::from_slice::<Ledger>(&fs::read(&path)?).context("invalid stock ledger")?
        } else {
            Ledger {
                device_id: config.device_id.clone(),
                ..Default::default()
            }
        };

        ensure!(
            ledger.device_id == config.device_id,
            "ledger belongs to another device"
        );

        // A trial in flight when the last process stopped may or may not have
        // reached the device: mark it uncertain rather than resend it.
        for record in ledger.ota_trials.values_mut() {
            if record.phase == TrialPhase::Submitted {
                record.phase = TrialPhase::Uncertain;
                record.updated_at = Utc::now();
            }
        }

        storage::write_json(&path, &ledger)?;
        let mut value = Self {
            config,
            path,
            ledger,
            status: Status {
                build_commit: build.commit.into(),
                build_clean: build.clean,
                ..Status::default()
            },
            storage_failed: false,
            ota_quiet_since: Utc::now(),
            _lease: lease,
        };

        value.refresh(Utc::now());
        Ok(value)
    }

    fn save(&self) -> Result<()> {
        storage::write_json(&self.path, &self.ledger)
    }

    fn telemetry_current(&self, now: DateTime<Utc>) -> bool {
        self.status.broker_connected
            && self.status.last_seen.is_some_and(|seen| {
                seen <= now && now - seen < chrono::Duration::seconds(TELEMETRY_SECONDS)
            })
    }

    fn check_observation_interval(&mut self, now: DateTime<Utc>) {
        if !self.telemetry_current(now) || self.ota_quiet_since > now {
            self.ota_quiet_since = now;
        }
    }

    pub fn refresh(&mut self, now: DateTime<Utc>) {
        self.status.ota_trials = self.ledger.ota_trials.values().cloned().collect();
        self.status
            .ota_trials
            .sort_by_key(|record| std::cmp::Reverse(record.sequence));

        self.check_observation_interval(now);
        self.status.device_online = self.telemetry_current(now);
    }

    /// Reports sent while disconnected are unseen, so quiet time restarts here.
    pub fn connected(&mut self, now: DateTime<Utc>) {
        self.status.broker_connected = true;
        self.ota_quiet_since = now;
    }

    pub fn disconnected(&mut self, now: DateTime<Utc>) -> Result<()> {
        self.status.broker_connected = false;
        self.status.last_seen = None;
        self.status.version_reported_at = None;
        self.status.power_reported_at = None;
        self.ota_quiet_since = now;

        let mut changed = false;
        for record in self.ledger.ota_trials.values_mut() {
            if record.phase == TrialPhase::Submitted {
                record.phase = TrialPhase::Uncertain;
                record.updated_at = now;
                changed = true;
            }
        }

        if changed {
            self.save()?;
        }
        self.refresh(now);
        Ok(())
    }

    pub fn expire(&mut self, now: DateTime<Utc>) -> Result<()> {
        let mut changed = false;
        for trial in self.ledger.ota_trials.values_mut() {
            if trial.phase == TrialPhase::Submitted && (now - trial.created_at).num_seconds() >= 600
            {
                trial.phase = TrialPhase::Uncertain;
                trial.updated_at = now;
                changed = true;
            }
        }

        if changed {
            self.save()?;
        }
        self.refresh(now);
        Ok(())
    }

    pub fn enable_ota_trial(&mut self) {
        self.status.ota_trial_enabled = true;
    }

    /// Whether stock reported its version recently enough for an OTA decision.
    pub fn version_fresh(&self, now: DateTime<Utc>) -> bool {
        Self::fresh(self.status.version_reported_at, now)
    }

    fn fresh(seen: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
        seen.is_some_and(|seen| (0..EVIDENCE_SECONDS).contains(&(now - seen).num_seconds()))
    }

    /// `lease` is the image server's private lease, already checked to have a
    /// live listener. It is only consulted for a new request ID.
    pub fn ota_trial(
        &mut self,
        input: ota::trial::Input,
        lease: Option<&ota::image::Lease>,
        now: DateTime<Utc>,
    ) -> std::result::Result<(ota::trial::Record, Option<Reply>), Rejection> {
        if self.storage_failed {
            return Err(Rejection::Storage);
        }

        if let Some(record) = self.ledger.ota_trials.get(&input.request_id) {
            return if record.input == input {
                Ok((record.clone(), None))
            } else {
                Err(Rejection::Conflict)
            };
        }

        if !self.status.ota_trial_enabled {
            return Err(Rejection::OtaDisabled);
        }
        self.refresh(now);
        if !self.status.device_online {
            return Err(Rejection::Offline);
        }

        if input.request_id.is_nil()
            || input.server_token.is_nil()
            || !self.config.plans.is_empty()
            || !Self::fresh(self.status.version_reported_at, now)
            || !Self::fresh(self.status.power_reported_at, now)
            || reported_version(&self.status) != Some(ORIGINAL_VERSION)
            || power_type(&self.status) != Some(1)
        {
            return Err(Rejection::InvalidOta);
        }

        if self.ledger.ota_trials.values().any(|r| {
            matches!(
                r.phase,
                TrialPhase::Submitted | TrialPhase::Uncertain | TrialPhase::ReportedError
            )
        }) {
            return Err(Rejection::OtaActive);
        }
        if self.ledger.ota_trials.len() >= 32 {
            return Err(Rejection::Full);
        }
        // The digest is the one the image server bound to the build's review.
        let Some(lease) = lease.filter(|lease| {
            lease.token == input.server_token
                && (lease.expires_at - now).num_seconds() >= MIN_LEASE_SECONDS
        }) else {
            return Err(Rejection::ImageServer);
        };
        let image_sha256 = lease.sha256.clone();

        let record = ota::trial::Record {
            sequence: self.ledger.ota_trials.len() + 1,
            input,
            created_at: now,
            updated_at: now,
            phase: TrialPhase::Submitted,
            download_reported: false,
            resolution_note: None,
            image_sha256,
        };

        // Saved before the command is returned for publishing, so a crash
        // leaves an uncertain trial, never a resent one.
        self.ledger
            .ota_trials
            .insert(record.input.request_id, record.clone());
        if self.save().is_err() {
            self.storage_failed = true;
            return Err(Rejection::Storage);
        }

        self.refresh(now);
        Ok((
            record.clone(),
            Some(ota::trial::command(&record, &self.config)),
        ))
    }

    /// The operator's client saw the replacement build answer the device
    /// API after `request`'s trial was submitted: the trial's evidence, as
    /// stock never reports again after activating it. Only the latest trial
    /// closes this way, while it is submitted or uncertain; the note records
    /// what the client saw. Sends nothing, and a repeat of the same request
    /// returns the closed record.
    pub fn replacement_answered(
        &mut self,
        request: ota::trial::ResolveInput,
        now: DateTime<Utc>,
    ) -> std::result::Result<ota::trial::Record, Rejection> {
        if self.storage_failed {
            return Err(Rejection::Storage);
        }
        if request.note.trim().is_empty() || request.note.len() > 512 {
            return Err(Rejection::Conflict);
        }

        let latest = self
            .ledger
            .ota_trials
            .values()
            .max_by_key(|r| r.sequence)
            .map(|r| r.input.request_id);
        let record = self
            .ledger
            .ota_trials
            .get_mut(&request.request_id)
            .ok_or(Rejection::Conflict)?;
        if record.phase == TrialPhase::ReplacementHealthy
            && record.resolution_note.as_ref() == Some(&request.note)
        {
            return Ok(record.clone());
        }
        if latest != Some(request.request_id)
            || !matches!(record.phase, TrialPhase::Submitted | TrialPhase::Uncertain)
            || now <= record.created_at
        {
            return Err(Rejection::NotOpen);
        }

        record.phase = TrialPhase::ReplacementHealthy;
        record.resolution_note = Some(request.note);
        record.updated_at = now;
        let result = record.clone();

        if self.save().is_err() {
            self.storage_failed = true;
            return Err(Rejection::Storage);
        }
        self.refresh(now);
        Ok(result)
    }

    /// Close the latest uncertain or failed trial from fresh version evidence.
    /// Sends nothing. Missing feeder telemetry or an OTA report breaks the
    /// observation interval; elapsed time alone cannot establish an idle device.
    pub fn resolve_trial(
        &mut self,
        request: ota::trial::ResolveInput,
        now: DateTime<Utc>,
    ) -> std::result::Result<ota::trial::Record, Rejection> {
        if self.storage_failed {
            return Err(Rejection::Storage);
        }
        if request.note.trim().is_empty() || request.note.len() > 512 {
            return Err(Rejection::Conflict);
        }

        self.refresh(now);
        let latest = self
            .ledger
            .ota_trials
            .values()
            .max_by_key(|r| r.sequence)
            .map(|r| r.input.request_id);
        let quiet = (now - self.ota_quiet_since).num_seconds() >= QUIET_SECONDS;
        let status = &self.status;
        let record = self
            .ledger
            .ota_trials
            .get_mut(&request.request_id)
            .ok_or(Rejection::Conflict)?;
        if !matches!(
            record.phase,
            TrialPhase::Uncertain | TrialPhase::ReportedError
        ) {
            return Err(Rejection::Conflict);
        }

        let evidence = latest == Some(request.request_id)
            && status.device_online
            && Self::fresh(status.version_reported_at, now)
            && status
                .version_reported_at
                .is_some_and(|seen| seen > record.created_at)
            && (now - record.created_at).num_seconds() >= RESOLVE_AFTER_SECONDS
            && quiet;

        let phase = match reported_version(status) {
            _ if !evidence => return Err(Rejection::ResolutionEvidence),
            Some(REPLACEMENT_VERSION) => TrialPhase::VersionReported,
            Some(ORIGINAL_VERSION) => TrialPhase::NotInstalled,
            _ => return Err(Rejection::ResolutionEvidence),
        };

        record.phase = phase;
        record.resolution_note = Some(request.note);
        record.updated_at = now;
        let result = record.clone();

        if self.save().is_err() {
            self.storage_failed = true;
            return Err(Rejection::Storage);
        }
        self.refresh(now);
        Ok(result)
    }

    pub fn observe(
        &mut self,
        channel: Channel,
        message: &Message,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.observe_telemetry(message, now);

        if self.observe_trial(channel, message, now) {
            self.save()?;
        }
        self.refresh(now);
        Ok(())
    }

    fn observe_telemetry(&mut self, message: &Message, now: DateTime<Utc>) {
        // Check the gap before overwriting last_seen, including when the MQTT
        // actor did not get a chance to run its expiry tick during the gap.
        self.check_observation_interval(now);
        self.status.last_seen = Some(now);
        if message.report.software_version.is_some() {
            self.status.version_reported_at = Some(now);
        }
        if message.report.power_type.is_some() {
            self.status.power_reported_at = Some(now);
        }

        if let Some(report) = report::from_message(message, now) {
            self.ota_quiet_since = now;
            if self.status.ota_reports.len() == REPORT_LIMIT {
                self.status.ota_reports.remove(0);
            }
            self.status.ota_reports.push(report);
        }

        if let Some(ts) = message.ts {
            self.status.clock_offset_seconds = Some((now.timestamp_millis() - ts) / 1000);
        }

        // Only expose documented telemetry. Never reflect credentials, SSIDs,
        // factory identifiers or arbitrary device log strings through the API.
        for key in [
            "hardwareVersion",
            "softwareVersion",
            "powerMode",
            "powerType",
            "electricQuantity",
            "surplusGrain",
            "motorState",
            "grainOutletState",
            "enableAudio",
            "volume",
            "bowlMode",
            "enableLight",
            "lightSwitch",
            "enableSound",
            "soundSwitch",
            "disableHardwareButton",
            "rssi",
        ] {
            if let Some(value) = message.fields.get(key) {
                self.status.attributes.insert(key.to_owned(), value.clone());
            }
        }
    }

    /// The latest trial's download, precheck-error and startup reports.
    fn observe_trial(&mut self, channel: Channel, message: &Message, now: DateTime<Utc>) -> bool {
        let Some(trial) = self
            .ledger
            .ota_trials
            .values_mut()
            .max_by_key(|r| r.sequence)
        else {
            return false;
        };
        if matches!(
            trial.phase,
            TrialPhase::VersionReported | TrialPhase::ReplacementHealthy
        ) {
            return false;
        }

        let matched = message.request_id() == Some(trial.input.request_id);
        let mut changed = false;

        if matched && matches!(message.cmd.as_str(), "OTA_PROGRESS" | "OTA_INFORM") {
            trial.download_reported = true;
            trial.updated_at = now;
            changed = true;
        }

        if matched
            && matches!(message.cmd.as_str(), "OTA_UPGRADE" | "OTA_INFORM")
            && let Some(error) = message.report.err_code.filter(|&code| code != 0)
        {
            // Any error closes the trial as failed; the operator resolves it
            // from fresh version evidence before another can be submitted.
            tracing::info!(error, "install trial reported an error");
            trial.phase = TrialPhase::ReportedError;
            trial.updated_at = now;
            changed = true;
        }

        if channel == Channel::Event
            && message.cmd == "DEVICE_START_EVENT"
            && message.report.software_version.as_deref() == Some(REPLACEMENT_VERSION)
            && message.ts.is_some_and(|ts| {
                ts >= trial.created_at.timestamp_millis()
                    && ts <= now.timestamp_millis().saturating_add(10_000)
            })
        {
            trial.phase = TrialPhase::VersionReported;
            trial.updated_at = now;
            changed = true;
        }

        changed
    }
}
