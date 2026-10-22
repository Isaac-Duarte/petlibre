//! Device state and durable, deliberately non-retrying feed operations.
use std::{collections::BTreeMap, fs, path::PathBuf};

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::{
    build_info::BUILD,
    config::Config,
    ota::{OtaReport, REPORT_LIMIT},
    ota_image,
    ota_trial::{self, Phase as TrialPhase},
    probe,
    protocol::{self, Message, Reply},
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
/// server remains connected to its own broker.
const TELEMETRY_SECONDS: i64 = 60;
/// Stock precheck code for "an OTA is already active" (`0x0e018e30`).
const OTA_ALREADY_ACTIVE: u64 = 0x464;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedPhase {
    Dispatching,
    Accepted,
    Started,
    Completed,
    Rejected,
    Uncertain,
    Resolved,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FeedRecord {
    pub request_id: Uuid,
    pub portions: u8,
    pub phase: FeedPhase,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub start_event_id: Option<String>,
    pub completion_event_id: Option<String>,
    pub resolution_note: Option<String>,
}

impl FeedRecord {
    fn blocks_feeding(&self) -> bool {
        matches!(
            self.phase,
            FeedPhase::Dispatching
                | FeedPhase::Accepted
                | FeedPhase::Started
                | FeedPhase::Uncertain
        )
    }
}

#[derive(Default, Deserialize, Serialize)]
struct Ledger {
    device_id: String,
    feeds: BTreeMap<Uuid, FeedRecord>,
    #[serde(default)]
    ota_trials: BTreeMap<Uuid, ota_trial::Record>,
    #[serde(default)]
    ota_hold: bool,
    #[serde(default)]
    probe_queries: BTreeMap<Uuid, probe::Record>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Status {
    pub broker_connected: bool,
    pub device_online: bool,
    pub last_seen: Option<DateTime<Utc>>,
    pub clock_offset_seconds: Option<i64>,
    pub attributes: Map<String, Value>,
    pub version_reported_at: Option<DateTime<Utc>>,
    pub power_reported_at: Option<DateTime<Utc>>,
    pub build_commit: &'static str,
    pub build_clean: bool,
    pub ota_reports: Vec<OtaReport>,
    pub ota_trial_enabled: bool,
    pub ota_hold: bool,
    pub ota_trials: Vec<ota_trial::Record>,
    pub probe_queries: Vec<probe::Record>,
    pub feeds: Vec<FeedRecord>,
    /// Replacement-firmware health (FW08); null without a firmware role.
    pub replacement: Option<crate::replacement::Snapshot>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FeedInput {
    pub request_id: Uuid,
    pub portions: u8,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveInput {
    pub request_id: Uuid,
    pub note: String,
}

#[derive(Clone, Copy, Debug)]
pub enum Rejection {
    Offline,
    InvalidPortions,
    Conflict,
    Busy,
    Cooldown,
    Full,
    Storage,
    OtaDisabled,
    InvalidOta,
    OtaActive,
    ImageServer,
    ResolutionEvidence,
    InvalidProbe,
}

impl Rejection {
    pub fn message(self) -> &'static str {
        match self {
            Self::Offline => "device is offline or telemetry is stale",
            Self::InvalidPortions => "portions are outside the configured limit",
            Self::Conflict => "request ID conflicts with a different operation",
            Self::Busy => "a feed is active or uncertain; inspect its outcome first",
            Self::Cooldown => "wait at least 60 seconds between feed requests",
            Self::Full => "operation ledger is full; archive it explicitly before continuing",
            Self::Storage => "persistent storage failed; command was not authorized",
            Self::OtaDisabled => {
                "restart serve with --enable-ota-trial to allow the fixed experiment"
            }
            Self::InvalidOta => {
                "trial requires valid IDs, recent expected-version and external-power reports and no plans"
            }
            Self::OtaActive => {
                "OTA trial is active or uncertain; feeding remains held until a fresh original-version startup report"
            }
            Self::ImageServer => {
                "image server is not listening with this token, or its accept window ends within five minutes"
            }
            Self::ResolutionEvidence => {
                "resolution needs the latest uncertain/failed trial, 20 minutes since submission, 10 quiet connected minutes without OTA reports and a fresh source- or target-version report"
            }
            Self::InvalidProbe => {
                "query needs a freshly reported, confirmed C-probe installation and no pending or uncertain query for that installation"
            }
        }
    }
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
    pub fn load(config: Config, path: PathBuf) -> Result<Self> {
        let lease = storage::lock(&path)?;
        let mut ledger = if path.exists() {
            serde_json::from_slice::<Ledger>(&fs::read(&path)?).context("invalid feed ledger")?
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
        for record in ledger.feeds.values_mut() {
            if matches!(
                record.phase,
                FeedPhase::Dispatching | FeedPhase::Accepted | FeedPhase::Started
            ) {
                record.phase = FeedPhase::Uncertain;
                record.updated_at = Utc::now();
            }
        }
        for record in ledger.ota_trials.values_mut() {
            if record.phase == TrialPhase::Submitted {
                record.phase = TrialPhase::Uncertain;
                record.updated_at = Utc::now();
                ledger.ota_hold = true;
            }
        }
        for query in ledger.probe_queries.values_mut() {
            if query.phase == probe::Phase::Submitted {
                query.phase = probe::Phase::Uncertain;
            }
        }
        storage::write_json(&path, &ledger)?;
        let mut value = Self {
            config,
            path,
            ledger,
            status: Status {
                build_commit: BUILD.commit,
                build_clean: BUILD.clean,
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
        self.status.ota_hold = self.ledger.ota_hold;
        self.status.probe_queries = self.ledger.probe_queries.values().cloned().collect();
        self.status
            .probe_queries
            .sort_by_key(|record| std::cmp::Reverse(record.created_at));
        self.status.ota_trials = self.ledger.ota_trials.values().cloned().collect();
        self.status
            .ota_trials
            .sort_by_key(|record| std::cmp::Reverse(record.sequence));
        self.check_observation_interval(now);
        self.status.device_online = self.telemetry_current(now);
        let mut feeds: Vec<_> = self.ledger.feeds.values().cloned().collect();
        feeds.sort_by_key(|f| std::cmp::Reverse(f.created_at));
        feeds.truncate(100);
        self.status.feeds = feeds;
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
        for query in self.ledger.probe_queries.values_mut() {
            if query.phase == probe::Phase::Submitted {
                query.phase = probe::Phase::Uncertain;
                changed = true;
            }
        }
        for record in self.ledger.ota_trials.values_mut() {
            if record.phase == TrialPhase::Submitted {
                record.phase = TrialPhase::Uncertain;
                record.updated_at = now;
                changed = true;
            }
        }
        for record in self.ledger.feeds.values_mut() {
            if matches!(
                record.phase,
                FeedPhase::Dispatching | FeedPhase::Accepted | FeedPhase::Started
            ) {
                record.phase = FeedPhase::Uncertain;
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

    pub fn feed(
        &mut self,
        request: FeedInput,
        now: DateTime<Utc>,
    ) -> std::result::Result<(FeedRecord, Option<Reply>), Rejection> {
        if self.storage_failed {
            return Err(Rejection::Storage);
        }
        if let Some(record) = self.ledger.feeds.get(&request.request_id) {
            if record.portions != request.portions {
                return Err(Rejection::Conflict);
            }
            return Ok((record.clone(), None));
        }
        if self.ledger.ota_hold {
            return Err(Rejection::OtaActive);
        }
        self.refresh(now);
        if !self.status.device_online {
            return Err(Rejection::Offline);
        }
        if request.portions == 0 || request.portions > self.config.max_portions {
            return Err(Rejection::InvalidPortions);
        }
        if self.ledger.feeds.values().any(FeedRecord::blocks_feeding) {
            return Err(Rejection::Busy);
        }
        if self
            .ledger
            .feeds
            .values()
            .any(|r| (now - r.created_at).num_seconds() < 60)
        {
            return Err(Rejection::Cooldown);
        }
        if self.ledger.feeds.len() >= 10_000 {
            return Err(Rejection::Full);
        }
        let record = FeedRecord {
            request_id: request.request_id,
            portions: request.portions,
            phase: FeedPhase::Dispatching,
            created_at: now,
            updated_at: now,
            start_event_id: None,
            completion_event_id: None,
            resolution_note: None,
        };
        self.ledger.feeds.insert(request.request_id, record.clone());
        // Persist before handing anything to MQTT. A crash can leave an uncertain
        // operation, but cannot cause an automatic replay on the next start.
        if self.save().is_err() {
            self.storage_failed = true;
            return Err(Rejection::Storage);
        }
        let mut command = protocol::service(
            &self.config,
            "MANUAL_FEEDING_SERVICE",
            request.request_id,
            now,
        );
        command.payload["grainNum"] = Value::from(request.portions);
        self.refresh(now);
        Ok((record, Some(command)))
    }

    pub fn resolve(
        &mut self,
        request: ResolveInput,
        now: DateTime<Utc>,
    ) -> std::result::Result<FeedRecord, Rejection> {
        if request.note.trim().is_empty() || request.note.len() > 512 {
            return Err(Rejection::Conflict);
        }
        let record = self
            .ledger
            .feeds
            .get_mut(&request.request_id)
            .ok_or(Rejection::Conflict)?;
        if record.phase != FeedPhase::Uncertain {
            return Err(Rejection::Conflict);
        }
        record.phase = FeedPhase::Resolved;
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

    pub fn expire(&mut self, now: DateTime<Utc>) -> Result<()> {
        let mut changed = false;
        for query in self.ledger.probe_queries.values_mut() {
            if query.phase == probe::Phase::Submitted
                && (now - query.created_at).num_seconds() >= probe::QUERY_SECONDS
            {
                query.phase = probe::Phase::Uncertain;
                changed = true;
            }
        }
        for trial in self.ledger.ota_trials.values_mut() {
            if trial.phase == TrialPhase::Submitted && (now - trial.created_at).num_seconds() >= 600
            {
                trial.phase = TrialPhase::Uncertain;
                trial.updated_at = now;
                changed = true;
            }
        }
        for record in self.ledger.feeds.values_mut() {
            if matches!(
                record.phase,
                FeedPhase::Dispatching | FeedPhase::Accepted | FeedPhase::Started
            ) && (now - record.created_at).num_seconds() >= 60
            {
                record.phase = FeedPhase::Uncertain;
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

    pub fn enable_ota_trial(&mut self) {
        self.status.ota_trial_enabled = true;
    }

    /// Persist one explicit diagnostic intent before publication. Never retries;
    /// an uncertain query permits restoration, but blocks further hook execution
    /// during this installation. No configuration query is sent on reconnect.
    pub fn probe_query(
        &mut self,
        input: probe::Input,
        now: DateTime<Utc>,
    ) -> std::result::Result<(probe::Record, Option<Reply>), Rejection> {
        if self.storage_failed {
            return Err(Rejection::Storage);
        }
        if let Some(record) = self.ledger.probe_queries.get(&input.request_id) {
            return Ok((record.clone(), None));
        }
        if !self.status.ota_trial_enabled {
            return Err(Rejection::OtaDisabled);
        }
        self.refresh(now);
        if !self.status.device_online {
            return Err(Rejection::Offline);
        }
        let trial = self
            .ledger
            .ota_trials
            .values()
            .max_by_key(|record| record.sequence)
            .filter(|record| {
                record.input.image == ota_trial::Image::CProbe
                    && record.phase == TrialPhase::VersionReported
            })
            .ok_or(Rejection::InvalidProbe)?;
        if input.request_id.is_nil()
            || !Self::fresh(self.status.version_reported_at, now)
            || self.reported_version() != Some("1.1.17")
            || self.ledger.probe_queries.values().any(|query| {
                query.trial_request_id == trial.input.request_id
                    && query.phase != probe::Phase::ProofReported
            })
        {
            return Err(Rejection::InvalidProbe);
        }
        if self.ledger.probe_queries.len() >= 32 {
            return Err(Rejection::Full);
        }
        let record = probe::Record {
            request_id: input.request_id,
            trial_request_id: trial.input.request_id,
            created_at: now,
            phase: probe::Phase::Submitted,
            proof: None,
        };
        self.ledger
            .probe_queries
            .insert(input.request_id, record.clone());
        if self.save().is_err() {
            self.storage_failed = true;
            return Err(Rejection::Storage);
        }
        self.refresh(now);
        Ok((
            record,
            Some(probe::command(&self.config, input.request_id, now)),
        ))
    }

    fn fresh(seen: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
        seen.is_some_and(|seen| (0..EVIDENCE_SECONDS).contains(&(now - seen).num_seconds()))
    }

    fn reported_version(&self) -> Option<&str> {
        self.status
            .attributes
            .get("softwareVersion")
            .and_then(Value::as_str)
    }

    /// `lease` is the image server's private lease, already checked to have a
    /// live listener. It is only consulted for a new request ID.
    pub fn ota_trial(
        &mut self,
        input: ota_trial::Input,
        lease: Option<&ota_image::Lease>,
        now: DateTime<Utc>,
    ) -> std::result::Result<(ota_trial::Record, Option<Reply>), Rejection> {
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
            || input
                .image
                .required_installation()
                .is_some_and(|installation| {
                    !self.ledger.ota_trials.values().any(|r| {
                        r.input.image == installation && r.phase == TrialPhase::VersionReported
                    })
                })
            || self
                .status
                .attributes
                .get("softwareVersion")
                .and_then(Value::as_str)
                != Some(input.image.source_version())
            || self
                .status
                .attributes
                .get("powerType")
                .and_then(Value::as_u64)
                != Some(1)
        {
            return Err(Rejection::InvalidOta);
        }
        if self.ledger.feeds.values().any(FeedRecord::blocks_feeding) {
            return Err(Rejection::Busy);
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
        if !lease.is_some_and(|lease| {
            lease.token == input.server_token
                && lease.profile == input.image.profile()
                && (lease.expires_at - now).num_seconds() >= MIN_LEASE_SECONDS
        }) {
            return Err(Rejection::ImageServer);
        }
        let record = ota_trial::Record {
            sequence: self.ledger.ota_trials.len() + 1,
            input,
            created_at: now,
            updated_at: now,
            phase: TrialPhase::Submitted,
            download_reported: false,
            resolution_note: None,
        };
        self.ledger.ota_hold = true;
        self.ledger
            .ota_trials
            .insert(record.input.request_id, record.clone());
        if self.save().is_err() {
            self.storage_failed = true;
            return Err(Rejection::Storage);
        }
        self.refresh(now);
        Ok((record.clone(), Some(record.command(&self.config))))
    }

    /// Close the latest uncertain or failed trial from fresh version evidence.
    /// Sends nothing. Missing feeder telemetry or an OTA report breaks the
    /// observation interval; elapsed time alone cannot establish an idle device.
    pub fn resolve_trial(
        &mut self,
        request: ota_trial::ResolveInput,
        now: DateTime<Utc>,
    ) -> std::result::Result<ota_trial::Record, Rejection> {
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
        let record = self
            .ledger
            .ota_trials
            .get(&request.request_id)
            .ok_or(Rejection::Conflict)?;
        if !matches!(
            record.phase,
            TrialPhase::Uncertain | TrialPhase::ReportedError
        ) {
            return Err(Rejection::Conflict);
        }
        let reported = self.reported_version();
        let image = record.input.image;
        let evidence = latest == Some(request.request_id)
            && self.status.device_online
            && Self::fresh(self.status.version_reported_at, now)
            && self
                .status
                .version_reported_at
                .is_some_and(|seen| seen > record.created_at)
            && (now - record.created_at).num_seconds() >= RESOLVE_AFTER_SECONDS
            && (now - self.ota_quiet_since).num_seconds() >= QUIET_SECONDS;
        let phase = match reported {
            _ if !evidence => return Err(Rejection::ResolutionEvidence),
            Some(version) if version == image.target_version() => TrialPhase::VersionReported,
            Some(version) if version == image.source_version() => TrialPhase::NotInstalled,
            _ => return Err(Rejection::ResolutionEvidence),
        };
        // Feeding resumes only when the original application is reported.
        let original_running = match phase {
            TrialPhase::VersionReported => image.target_version() == "1.1.15",
            _ => image.source_version() == "1.1.15",
        };
        let record = self
            .ledger
            .ota_trials
            .get_mut(&request.request_id)
            .expect("record checked above");
        record.phase = phase;
        record.resolution_note = Some(request.note);
        record.updated_at = now;
        let result = record.clone();
        if original_running {
            self.ledger.ota_hold = false;
        }
        if self.save().is_err() {
            self.storage_failed = true;
            return Err(Rejection::Storage);
        }
        self.refresh(now);
        Ok(result)
    }

    pub fn observe(&mut self, channel: &str, message: &Message, now: DateTime<Utc>) -> Result<()> {
        // Check the gap before overwriting last_seen, including when the MQTT
        // actor did not get a chance to run its expiry tick during the gap.
        self.check_observation_interval(now);
        self.status.last_seen = Some(now);
        if message
            .fields
            .get("softwareVersion")
            .is_some_and(Value::is_string)
        {
            self.status.version_reported_at = Some(now);
        }
        if message.fields.get("powerType").is_some_and(Value::is_u64) {
            self.status.power_reported_at = Some(now);
        }
        if let Some(report) = OtaReport::from_message(message, now) {
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
        let mut changed = false;
        // Accept only fresh, computed replies to an explicit outstanding query.
        // A repeated device-generated ID cannot establish a second execution.
        let proof = self
            .ledger
            .probe_queries
            .values()
            .filter_map(|query| {
                query
                    .report(channel, message, now)
                    .map(|proof| (query.request_id, query.trial_request_id, proof))
            })
            .next();
        if let Some((id, trial_id, proof)) = proof {
            let installed = self
                .ledger
                .ota_trials
                .values()
                .max_by_key(|r| r.sequence)
                .is_some_and(|r| {
                    r.input.request_id == trial_id
                        && r.input.image == ota_trial::Image::CProbe
                        && r.phase == TrialPhase::VersionReported
                });
            let duplicate = self.ledger.probe_queries.values().any(|q| {
                q.proof
                    .as_ref()
                    .is_some_and(|p| p.message_id == proof.message_id)
            });
            if installed && !duplicate {
                let query = self
                    .ledger
                    .probe_queries
                    .get_mut(&id)
                    .expect("query found above");
                query.phase = probe::Phase::ProofReported;
                query.proof = Some(proof);
                changed = true;
            }
        }
        if let Some(trial) = self
            .ledger
            .ota_trials
            .values_mut()
            .max_by_key(|r| r.sequence)
        {
            let pending = trial.phase != TrialPhase::VersionReported;
            let matched = message
                .msg_id
                .as_deref()
                .and_then(|id| Uuid::parse_str(id).ok())
                == Some(trial.input.request_id);
            if pending && matched && matches!(message.cmd.as_str(), "OTA_PROGRESS" | "OTA_INFORM") {
                trial.download_reported = true;
                trial.updated_at = now;
                changed = true;
            }
            if pending
                && matches!(message.cmd.as_str(), "OTA_UPGRADE" | "OTA_INFORM")
                && matched
                && message
                    .fields
                    .get("errCode")
                    .and_then(Value::as_u64)
                    .is_some_and(|n| n != 0)
            {
                // Stock OTA_UPGRADE reports are only triggered by the request
                // handler (0x0e016ba0); a nonzero errCode comes only from its
                // rejected-precheck branch. Their phase label is hard-coded to
                // "precheck", so it is not evidence. "Already active" (0x464) is
                // excluded: that rejection means another download is running.
                // Other errors cannot establish the original application is intact.
                let error = message.fields.get("errCode").and_then(Value::as_u64);
                let precheck_only = trial.phase == TrialPhase::Submitted
                    && trial.input.image.source_version() == "1.1.15"
                    && !trial.download_reported
                    && message.cmd == "OTA_UPGRADE"
                    && error != Some(OTA_ALREADY_ACTIVE)
                    && Self::fresh(self.status.version_reported_at, now)
                    && self
                        .status
                        .attributes
                        .get("softwareVersion")
                        .and_then(Value::as_str)
                        == Some("1.1.15");
                trial.phase = TrialPhase::ReportedError;
                trial.updated_at = now;
                if precheck_only {
                    self.ledger.ota_hold = false;
                }
                changed = true;
            }
            if pending
                && channel == "event"
                && message.cmd == "DEVICE_START_EVENT"
                && message
                    .fields
                    .get("softwareVersion")
                    .and_then(Value::as_str)
                    == Some(trial.input.image.target_version())
                && message.ts.is_some_and(|ts| {
                    ts >= trial.created_at.timestamp_millis()
                        && ts <= now.timestamp_millis().saturating_add(10_000)
                })
            {
                trial.phase = TrialPhase::VersionReported;
                trial.updated_at = now;
                // Also re-holds if a resolved "not installed" test later boots.
                self.ledger.ota_hold = trial.input.image.target_version() != "1.1.15";
                changed = true;
            }
        }
        if channel == "service"
            && message.cmd == "MANUAL_FEEDING_SERVICE"
            && let Some(id) = message
                .msg_id
                .as_deref()
                .and_then(|v| Uuid::parse_str(v).ok())
            && let Some(record) = self.ledger.feeds.get_mut(&id)
            && record.phase == FeedPhase::Dispatching
            && message.fields.get("code").and_then(Value::as_i64).is_some()
        {
            record.phase = if message.fields.get("code").and_then(Value::as_i64) == Some(0) {
                FeedPhase::Accepted
            } else {
                FeedPhase::Uncertain
            };
            record.updated_at = now;
            changed = true;
        }
        if channel == "event"
            && message.cmd == "GRAIN_OUTPUT_EVENT"
            && message.fields.get("type").and_then(Value::as_u64) == Some(2)
            && let Some(request_id) = message
                .msg_id
                .as_deref()
                .and_then(|id| Uuid::parse_str(id).ok())
        {
            let event_id = message.msg_id.as_deref();
            let step = message.fields.get("execStep").and_then(Value::as_str);
            // Stock manual reports carry the submitted request ID for both
            // steps. Correlate it before counts; unrelated or overwritten IDs
            // must leave the pending request unresolved. Deduplicate each step
            // independently so its start cannot hide its completion.
            let already_seen = event_id.is_some_and(|id| {
                self.ledger.feeds.values().any(|r| match step {
                    Some("GRAIN_START") => r.start_event_id.as_deref() == Some(id),
                    Some("GRAIN_END") => r.completion_event_id.as_deref() == Some(id),
                    _ => false,
                })
            });
            if !already_seen
                && let Some(record) = self.ledger.feeds.get_mut(&request_id)
                && matches!(record.phase, FeedPhase::Accepted | FeedPhase::Started)
            {
                let expected = message.fields.get("expectGrainNum").and_then(Value::as_u64);
                if expected == Some(u64::from(record.portions)) {
                    match step {
                        Some("GRAIN_START") if record.phase == FeedPhase::Accepted => {
                            record.phase = FeedPhase::Started;
                            record.start_event_id = message.msg_id.clone();
                            changed = true;
                        }
                        Some("GRAIN_END")
                            if record.phase == FeedPhase::Started
                                && message.fields.get("finished") == Some(&Value::Bool(true)) =>
                        {
                            record.phase =
                                if message.fields.get("actualGrainNum").and_then(Value::as_u64)
                                    == expected
                                {
                                    FeedPhase::Completed
                                } else {
                                    FeedPhase::Uncertain
                                };
                            record.completion_event_id = message.msg_id.clone();
                            changed = true;
                        }
                        _ => {}
                    }
                    if changed {
                        record.updated_at = now;
                    }
                }
            }
        }
        if changed {
            self.save()?;
        }
        self.refresh(now);
        Ok(())
    }
}
