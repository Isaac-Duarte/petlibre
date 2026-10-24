//! No device I/O: the stock-ota stages against a scripted feeder that runs
//! stock until asked, then reports the way the stock updater and the
//! replacement firmware do, on a virtual clock.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::*;
use crate::commands::itest::Rig;
use crate::commands::ota::OtaArgs;
use crate::device::{Answer, Ask, DeviceStatus, Intruder, Rebuffed};
use crate::server::LocalStatus;

const BUILD: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";
const OTHER: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const TOKEN: &str = "6f1c2b9e-7a43-4c55-9b0e-2d8a1f3c4e5d";
/// The stock payload hash FW41 signs as its build ID.
const STOCK: &str = "05147476fc70c91cc77ad7da6d6772376b2081d460ac7b1912e2d73868a09498";
const STOCK_REVIEW: &str = "private/verification/stock-review";

/// How the scripted install goes.
#[derive(Clone, Copy, PartialEq)]
enum Script {
    /// Stock installs; the build boots from bank 0 and reports health.
    Installs,
    /// Stock's precheck refuses (an OTA already active).
    Refused,
    /// Stock never reaches this server.
    Unrouted,
    /// The feeder runs some other stock release.
    OtherVersion,
    /// Something other than the build starts after activation.
    WrongBuild,
    /// A build from before recovery-role health: silent on bank 0.
    OldBuild,
    /// Nothing reports after the bank-0 start.
    Silent,
}

struct StockFeeder {
    script: Script,
    now: f64,
    pending: Vec<(f64, Change)>,
    uart: Vec<u8>,
    stock_online: bool,
    version: &'static str,
    reports: Vec<Value>,
    trial: Option<(String, &'static str)>,
    /// A request from an earlier run: its ID, phase, and when the server's
    /// evidence lets it be resolved.
    earlier: Option<(String, &'static str, f64)>,
    /// The replacement's MQTT health (bank 0, or a build from before the
    /// device API): build, bank, and whether it is the recovery role.
    health: Option<(String, u64, bool)>,
    /// The normal role on bank 1 answers the device API instead: its build.
    api: Option<String>,
    offered: bool,
    served: Option<PathBuf>,
    /// Stock runs (and reports once routed); false while a replacement does.
    on_stock: bool,
    actions: Vec<String>,
    notes: Vec<String>,
}

enum Change {
    Line(String),
    Report(Value),
    StockOffline,
    /// Stock took over bank 1: the replacement is gone.
    OnStock,
    /// Build, bank and whether it is the recovery role.
    Health(String, u64, bool),
    Phase(&'static str),
}

impl StockFeeder {
    fn new(script: Script) -> Self {
        Self {
            script,
            now: 0.0,
            pending: Vec::new(),
            uart: Vec::new(),
            stock_online: false,
            version: if script == Script::OtherVersion {
                "1.1.14"
            } else {
                STOCK_VERSION
            },
            reports: Vec::new(),
            trial: None,
            earlier: None,
            health: None,
            api: None,
            offered: false,
            served: None,
            on_stock: true,
            actions: Vec::new(),
            notes: Vec::new(),
        }
    }

    fn at(&mut self, delay: f64, change: Change) {
        self.pending.push((self.now + delay, change));
    }

    fn line(&mut self, delay: f64, text: &str) {
        self.at(delay, Change::Line(text.into()));
    }

    fn report(&mut self, delay: f64, command: &str, progress: Option<u8>, error: Option<u32>) {
        let at = format!(
            "2026-10-23T12:{:02}:{:02}Z",
            (self.now + delay) as u64 / 60 % 60,
            (self.now + delay) as u64 % 60
        );
        self.at(
            delay,
            Change::Report(
                json!({"received_at": at, "command": command, "phase": "download",
                                  "progress": progress, "error_code": error}),
            ),
        );
    }

    fn release(&mut self) {
        let now = self.now;
        let (mut due, later): (Vec<_>, Vec<_>) =
            self.pending.drain(..).partition(|(at, _)| *at <= now);
        self.pending = later;
        due.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, change) in due {
            match change {
                Change::Line(text) => {
                    self.uart.extend_from_slice(text.as_bytes());
                    self.uart.extend_from_slice(b"\n\r");
                }
                Change::Report(report) => self.reports.push(report),
                Change::StockOffline => self.stock_online = false,
                Change::OnStock => {
                    self.on_stock = true;
                    self.health = None;
                    self.api = None;
                }
                // Since FW47 the normal role reports over the device API only.
                Change::Health(build, 1, false) => {
                    self.health = None;
                    self.api = Some(build);
                }
                Change::Health(build, bank, recovery) => {
                    self.health = Some((build, bank, recovery));
                    self.api = None;
                }
                Change::Phase(phase) => {
                    if let Some((_, current)) = &mut self.trial {
                        *current = phase;
                    }
                }
            }
        }
    }
}

impl Rig for StockFeeder {
    /// Only the round trip stages through the rig: the stock review, or the
    /// baseline's again.
    fn stage(&mut self, args: &OtaArgs) -> Result<Option<Staged>> {
        let review = args.review.clone().expect("the round trip offers reviews");
        let stock = review == Path::new(STOCK_REVIEW);
        Ok(Some(Staged {
            build: if stock { STOCK } else { BUILD }.into(),
            target: if stock {
                crate::commands::ota::Target::Stock
            } else {
                crate::commands::ota::Target::Replacement
            },
            version: 73,
            image: review.join("image.bin").display().to_string(),
            manifest: format!("private/verification/{}-v73.manifest", args.name),
            review,
        }))
    }

    fn review_build(&mut self, _review: &Path) -> Result<String> {
        unreachable!()
    }

    fn server(&mut self, server: Server) -> Result<()> {
        self.actions.push(format!("server {server:?}"));
        Ok(())
    }

    /// The bank-0 recovery role finds the offer at its next 10 s ask and
    /// installs it into bank 1, whose trial boot confirms.
    fn offer(&mut self, staged: Option<&Staged>) -> Result<()> {
        let Some(staged) = staged else {
            self.actions.push("clear".into());
            return Ok(());
        };
        // A stock offer: our running build installs it into bank 1, then
        // stock prints its banner and, once routed, reports.
        if staged.target == crate::commands::ota::Target::Stock {
            self.actions.push("offer stock".into());
            self.line(10.0, "PLOTA1 manifest version=73 bytes=1376256");
            self.line(40.0, "PLOTA1 activated version=73 sectors=336");
            self.line(45.0, "[stock] app_main:version:1.1.15");
            self.at(46.0, Change::OnStock);
            return Ok(());
        }

        self.offered = true;
        let on = match (&self.health, &self.api) {
            (Some((_, bank, recovery)), _) => {
                format!("bank {bank}{}", if *recovery { " recovery" } else { "" })
            }
            (None, Some(_)) => "bank 1".into(),
            (None, None) => "nothing".into(),
        };
        self.actions.push(format!("offer on {on}"));

        let build = BUILD;
        self.line(10.0, "PLOTA1 manifest version=72 bytes=663552");
        self.line(40.0, "PLOTA1 progress sectors=162/162 leases=2600");
        self.line(50.0, "PLOTA1 activated version=72 sectors=162");
        self.line(
            55.0,
            &format!("PLFW1 start build={build} bank=1 reset=0x0 mmu=1"),
        );
        self.line(56.0, "PLTRL1 confirmed=0 boot=1 limit=3");
        self.line(90.0, "PLCNF1 confirmed=1 healthy_ms=30000");
        self.at(92.0, Change::Health(build.into(), 1, false));
        Ok(())
    }

    fn device(&mut self, ask: &Ask) -> Result<Answer> {
        self.release();
        let Some(build) = self.api.clone() else {
            return Err(Error::Bench("the feeder did not answer".into()));
        };
        assert_eq!(*ask, Ask::Status, "the round trip only reads status");
        Ok(Answer::Status(DeviceStatus {
            build,
            bank: 1,
            uptime_ms: 40_000,
            boot_count: 3,
            wifi: petlibre_proto::api::WifiSource::Build,
            losses: 0,
            clock_ms: None,
            settings_generation: 1,
            wifi_generation: 1,
        }))
    }

    fn intrude(&mut self, _intruder: Intruder) -> Result<Rebuffed> {
        unreachable!()
    }

    fn unix_ms(&self) -> i64 {
        unreachable!()
    }

    fn status(&mut self) -> Result<LocalStatus> {
        self.release();
        // Newest first, as the server lists them.
        let earlier = self
            .earlier
            .iter()
            .map(|(id, phase, _)| (id.clone(), *phase));
        let trials: Vec<Value> = self
            .trial
            .clone()
            .into_iter()
            .chain(earlier)
            .map(|(id, phase)| {
                json!({"input": {"request_id": id}, "phase": phase,
                                     "download_reported": !self.reports.is_empty()})
            })
            .collect();
        let replacement = match &self.health {
            Some((build, bank, recovery)) => {
                let mut health = json!({"build": build, "bank": bank, "seq": 1,
                                        "outputs": "stopped", "faults": []});
                if *recovery {
                    health["role"] = json!("recovery");
                }
                json!({"health": health, "online": true, "stale": false, "age_ms": 500})
            }
            None => json!({"online": false, "stale": true}),
        };
        Ok(serde_json::from_value(json!({
            "device_online": self.stock_online,
            "attributes": {"softwareVersion": self.version, "powerType": 1},
            "version_reported_at": "2026-10-23T12:00:00Z",
            "ota_trial_enabled": true,
            "ota_trials": trials,
            "ota_reports": self.reports,
            "replacement": replacement,
        }))?)
    }

    fn hotspot(&mut self, _up: bool) -> Result<()> {
        unreachable!()
    }

    fn power_cycle(&mut self) -> Result<()> {
        unreachable!()
    }

    fn uart(&mut self) -> Result<Vec<u8>> {
        self.release();
        if self.on_stock
            && self.script != Script::Unrouted
            && !self.stock_online
            && self.trial.is_none()
        {
            // Stock joins and reports a little after the run starts.
            self.at(40.0, Change::Line("[stock] mqtt connected".into()));
            self.stock_online = true;
        }
        Ok(std::mem::take(&mut self.uart))
    }

    fn wait(&mut self, seconds: f64) -> Result<()> {
        self.now += seconds;
        Ok(())
    }

    fn clock(&self) -> f64 {
        self.now
    }

    fn note(&mut self, text: &str) {
        self.notes.push(text.into());
    }

    fn images(&mut self, review: &Path) -> Result<String> {
        self.served = Some(review.to_path_buf());
        Ok(TOKEN.into())
    }

    fn resolve_trial(&mut self, request_id: &str, _note: &str) -> Result<()> {
        let now = self.now;
        let Some((id, phase, at)) = &mut self.earlier else {
            panic!("nothing to resolve")
        };
        assert_eq!(id, request_id);
        if now < *at {
            return Err(Error::Bench(
                "POST /api/ota/trial/resolve: 409 resolution needs more evidence".into(),
            ));
        }
        *phase = "not_installed";
        self.actions.push("resolved".into());
        Ok(())
    }

    fn stock_trial(&mut self, trial: &TrialRequest) -> Result<()> {
        assert!(
            self.earlier
                .as_ref()
                .is_none_or(|(_, phase, _)| *phase == "not_installed"),
            "asked while an earlier request was still open"
        );
        assert_eq!(trial.server_token, TOKEN);
        require_none(&self.trial);
        self.trial = Some((trial.request_id.clone(), "submitted"));

        match self.script {
            Script::Refused => {
                self.report(2.0, "OTA_UPGRADE", None, Some(0x464));
                self.at(2.0, Change::Phase("reported_error"));
            }
            Script::Installs | Script::WrongBuild | Script::OldBuild | Script::Silent => {
                self.report(10.0, "OTA_PROGRESS", Some(50), None);
                self.report(60.0, "OTA_PROGRESS", Some(100), None);
                self.at(65.0, Change::StockOffline);
                let build = if self.script == Script::WrongBuild {
                    OTHER
                } else {
                    BUILD
                };
                self.line(
                    70.0,
                    &format!("PLFW1 start build={build} bank=0 reset=0x0 mmu=0"),
                );
                // The recovery role reports while it waits for an offer;
                // a build from before that reports nothing from bank 0.
                if self.script != Script::Silent {
                    let recovery = self.script != Script::OldBuild;
                    self.at(100.0, Change::Health(build.into(), 0, recovery));
                    self.at(100.0, Change::Phase("replacement_healthy"));
                }
            }
            Script::Unrouted | Script::OtherVersion => unreachable!("never asked"),
        }
        Ok(())
    }
}

/// The run sends stock one request, ever.
fn require_none(trial: &Option<(String, &'static str)>) {
    assert!(trial.is_none(), "a second stock request was sent");
}

fn args(promote: bool) -> StockOtaArgs {
    use clap::Parser;
    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        args: StockOtaArgs,
    }
    let mut argv = vec!["stock-ota", "poc1"];
    if !promote {
        argv.push("--no-promote");
    }
    Cli::try_parse_from(argv).unwrap().args
}

fn staged() -> Staged {
    Staged {
        build: BUILD.into(),
        target: crate::commands::ota::Target::Replacement,
        version: 72,
        review: PathBuf::from("private/verification/poc1-review"),
        manifest: "private/verification/poc1-v72.manifest".into(),
        image: "private/verification/poc1-review/image.bin".into(),
    }
}

fn session(feeder: &mut StockFeeder, promote: bool) -> Vec<(&'static str, Verdict)> {
    let args = args(promote);
    let rig_args = args.rig_args();
    let mut ctx = Ctx::new(feeder, &rig_args);
    let (reports, silent) = run(&mut ctx, &args, &staged());
    assert!(silent.is_none());
    reports
        .into_iter()
        .map(|r| (r.scenario, r.verdict))
        .collect()
}

fn verdicts(reports: &[(&'static str, Verdict)]) -> Vec<(&'static str, &'static str)> {
    reports
        .iter()
        .map(|(name, verdict)| (*name, verdict.kind().as_str()))
        .collect()
}

#[test]
fn stock_installs_the_build_which_reports_from_bank_0() {
    let mut feeder = StockFeeder::new(Script::Installs);
    let reports = session(&mut feeder, false);

    assert_eq!(
        verdicts(&reports),
        [
            (READY, "pass"),
            (SETTLE, "pass"),
            (REQUEST, "pass"),
            (DOWNLOAD, "pass"),
            (BANK0, "pass")
        ]
    );
    assert_eq!(
        feeder.served.as_deref(),
        Some(Path::new("private/verification/poc1-review"))
    );
    assert!(!feeder.offered, "no promotion offer with --no-promote");
    assert!(
        feeder
            .notes
            .iter()
            .any(|n| n.contains("stock OTA_PROGRESS phase download 100%"))
    );
    assert_eq!(
        feeder
            .notes
            .iter()
            .filter(|n| n.contains("OTA_PROGRESS phase download 50%"))
            .count(),
        1,
        "each stock report is noted once"
    );
}

#[test]
fn promote_lets_the_bank_0_build_install_itself_into_bank_1() {
    let mut feeder = StockFeeder::new(Script::Installs);
    let reports = session(&mut feeder, true);

    assert_eq!(
        verdicts(&reports),
        [
            (READY, "pass"),
            (SETTLE, "pass"),
            (REQUEST, "pass"),
            (DOWNLOAD, "pass"),
            (BANK0, "pass"),
            (PROMOTE, "pass")
        ]
    );
    // Offered once the recovery role reported from bank 0.
    assert_eq!(feeder.actions, ["offer on bank 0 recovery"]);
    assert!(feeder.offered);
}

#[test]
fn a_bank_0_build_without_recovery_health_fails_before_any_offer() {
    for (script, why) in [
        (Script::OldBuild, "not the recovery role"),
        (Script::Silent, "No recovery-role health"),
    ] {
        let mut feeder = StockFeeder::new(script);
        let reports = session(&mut feeder, true);

        assert_eq!(
            verdicts(&reports)[3..],
            [(DOWNLOAD, "pass"), (BANK0, "fail"), (PROMOTE, "skip")]
        );
        let Verdict::Fail(reason) = &reports[4].1 else {
            unreachable!()
        };
        assert!(reason.contains(why), "{reason}");
        assert!(!feeder.offered);
    }
}

#[test]
fn a_feeder_not_reporting_stock_is_never_asked() {
    for script in [Script::Unrouted, Script::OtherVersion] {
        let mut feeder = StockFeeder::new(script);
        let reports = session(&mut feeder, false);

        assert_eq!(
            verdicts(&reports),
            [
                (READY, "fail"),
                (SETTLE, "skip"),
                (REQUEST, "skip"),
                (DOWNLOAD, "skip"),
                (BANK0, "skip")
            ]
        );
        assert!(feeder.trial.is_none());
        assert!(feeder.served.is_none());
        assert!(recovery(&session_reports(&reports)).is_none());
    }
}

#[test]
fn a_refused_request_fails_with_stock_s_error_and_is_not_resent() {
    let mut feeder = StockFeeder::new(Script::Refused);
    let reports = session(&mut feeder, false);

    assert_eq!(
        verdicts(&reports),
        [
            (READY, "pass"),
            (SETTLE, "pass"),
            (REQUEST, "pass"),
            (DOWNLOAD, "fail"),
            (BANK0, "skip")
        ]
    );
    let Verdict::Fail(reason) = &reports[3].1 else {
        unreachable!()
    };
    assert!(reason.contains("error 0x464"), "{reason}");
    assert!(recovery(&session_reports(&reports)).is_some());
}

#[test]
fn another_build_starting_after_activation_fails_the_install() {
    let mut feeder = StockFeeder::new(Script::WrongBuild);
    let reports = session(&mut feeder, false);

    let Verdict::Fail(reason) = &reports[3].1 else {
        panic!("{:?}", reports[3])
    };
    assert!(reason.contains("Build 0123456789ab started"), "{reason}");
}

#[test]
fn a_start_record_from_bank_1_is_refused() {
    let line = format!("PLFW1 start build={BUILD} bank=1 reset=0x0 mmu=1");
    assert!(started(&line, BUILD).is_err());
    let line = format!(
        "PLFW1 start build={} bank=0 reset=0x0 mmu=0",
        BUILD.to_uppercase()
    );
    assert!(started(&line, BUILD).is_ok());
}

#[test]
fn trial_phases_read_from_the_server_s_ledger() {
    let status: LocalStatus = serde_json::from_value(json!({
        "ota_trials": [
            {"input": {"request_id": "a"}, "phase": "replacement_healthy"},
            {"input": {"request_id": "b"}, "phase": "something_new"},
        ],
    }))
    .unwrap();
    assert_eq!(
        status.trial("a").map(|t| t.phase),
        Some(TrialPhase::ReplacementHealthy)
    );
    assert_eq!(
        status.trial("b").map(|t| t.phase),
        Some(TrialPhase::Unknown)
    );
    assert!(status.trial("c").is_none());
}

#[test]
fn the_steps_end_with_the_promotion_unless_told_not_to() {
    assert_eq!(
        args(false).steps(),
        [READY, SETTLE, REQUEST, DOWNLOAD, BANK0]
    );
    assert_eq!(
        args(true).steps(),
        [READY, SETTLE, REQUEST, DOWNLOAD, BANK0, PROMOTE]
    );
}

fn session_reports(reports: &[(&'static str, Verdict)]) -> Vec<Report> {
    reports
        .iter()
        .map(|(scenario, verdict)| Report {
            scenario,
            verdict: verdict.clone(),
            seconds: 0.0,
        })
        .collect()
}

#[test]
fn an_earlier_failed_request_is_resolved_before_stock_is_asked_again() {
    let mut feeder = StockFeeder::new(Script::Installs);
    feeder.earlier = Some(("f5ef47b8".into(), "reported_error", 600.0));
    let reports = session(&mut feeder, false);

    assert_eq!(
        verdicts(&reports),
        [
            (READY, "pass"),
            (SETTLE, "pass"),
            (REQUEST, "pass"),
            (DOWNLOAD, "pass"),
            (BANK0, "pass")
        ]
    );
    assert_eq!(feeder.actions, ["resolved"]);
    // One "not yet" line, however many polls it took.
    assert_eq!(
        feeder
            .notes
            .iter()
            .filter(|n| n.contains("Not yet resolvable"))
            .count(),
        1
    );
}

#[test]
fn an_earlier_request_the_server_never_resolves_stops_the_run() {
    let mut feeder = StockFeeder::new(Script::Installs);
    feeder.earlier = Some(("f5ef47b8".into(), "uncertain", f64::INFINITY));
    let reports = session(&mut feeder, false);

    assert_eq!(
        verdicts(&reports)[..3],
        [(READY, "pass"), (SETTLE, "fail"), (REQUEST, "skip")]
    );
    assert!(feeder.trial.is_none());
}

/// A feeder on the confirmed baseline, set for `stock-roundtrip`.
fn on_baseline(script: Script) -> (StockFeeder, ItestArgs) {
    let mut feeder = StockFeeder::new(script);
    feeder.on_stock = false;
    feeder.api = Some(BUILD.into());
    let mut rig_args = args(true).rig_args();
    rig_args.power = true;
    rig_args.stock = Some(PathBuf::from(STOCK_REVIEW));
    (feeder, rig_args)
}

fn baseline() -> super::super::Baseline {
    super::super::Baseline {
        build: BUILD.into(),
        review: PathBuf::from("private/verification/poc1-review"),
    }
}

#[test]
fn the_round_trip_goes_to_stock_and_back_to_the_confirmed_baseline() {
    let (mut feeder, rig_args) = on_baseline(Script::Installs);
    let mut ctx = Ctx::new(&mut feeder, &rig_args);
    ctx.baseline = Some(baseline());
    roundtrip(&mut ctx).unwrap();
    drop(ctx);

    assert_eq!(
        feeder.actions,
        [
            "offer stock",
            "clear",
            "server StockTrial",
            "offer on bank 0 recovery",
            "clear",
            "server Serve"
        ]
    );
    // Stock was asked once, for the baseline's own image.
    assert_eq!(
        feeder.served.as_deref(),
        Some(Path::new("private/verification/poc1-review"))
    );
    assert_eq!(feeder.api.as_deref(), Some(BUILD));
    assert!(
        feeder
            .notes
            .iter()
            .any(|n| n.contains("Stock 1.1.15 started"))
    );
}

#[test]
fn stock_that_never_reaches_the_server_stops_the_round_trip_on_stock() {
    let (mut feeder, rig_args) = on_baseline(Script::Unrouted);
    let mut ctx = Ctx::new(&mut feeder, &rig_args);
    ctx.baseline = Some(baseline());
    let error = roundtrip(&mut ctx).unwrap_err();
    drop(ctx);

    assert!(error.to_string().contains("No stock 1.1.15"), "{error}");
    assert!(feeder.trial.is_none(), "stock was never asked");
    assert!(feeder.on_stock);
}
