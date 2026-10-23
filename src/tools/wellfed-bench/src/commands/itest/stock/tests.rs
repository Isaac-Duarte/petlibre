//! No device I/O: the stock-ota stages against a scripted feeder that runs
//! stock until asked, then reports the way the stock updater and the
//! replacement firmware do, on a virtual clock.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::*;
use crate::commands::itest::Rig;
use crate::commands::ota::OtaArgs;
use crate::server::{ForgedCase, LocalStatus, Request};

const BUILD: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";
const OTHER: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const TOKEN: &str = "6f1c2b9e-7a43-4c55-9b0e-2d8a1f3c4e5d";

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
    /// The replacement's health: build and bank.
    health: Option<(String, u64)>,
    offered: bool,
    served: Option<PathBuf>,
    actions: Vec<String>,
    notes: Vec<String>,
}

enum Change {
    Line(String),
    Report(Value),
    StockOffline,
    Health(String, u64),
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
            health: None,
            offered: false,
            served: None,
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
                Change::Health(build, bank) => self.health = Some((build, bank)),
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
    fn stage(&mut self, _args: &OtaArgs) -> Result<Option<Staged>> {
        unreachable!("stock-ota stages before the rig")
    }

    fn review_build(&mut self, _review: &Path) -> Result<String> {
        unreachable!()
    }

    fn server(&mut self, server: Server) -> Result<()> {
        self.actions.push(format!("server {server:?}"));
        Ok(())
    }

    fn forge(&mut self) -> Result<Vec<ForgedCase>> {
        unreachable!()
    }

    fn offer(&mut self, staged: Option<&Staged>) -> Result<()> {
        self.offered = staged.is_some();
        self.actions.push(format!(
            "offer before the request: {}",
            self.trial.is_none()
        ));
        Ok(())
    }

    fn request(&mut self, _request: &Request) -> Result<String> {
        unreachable!()
    }

    fn status(&mut self) -> Result<LocalStatus> {
        self.release();
        let trials: Vec<Value> = self
            .trial
            .iter()
            .map(|(id, phase)| {
                json!({"input": {"request_id": id}, "phase": phase,
                                     "download_reported": !self.reports.is_empty()})
            })
            .collect();
        let replacement = match &self.health {
            Some((build, bank)) => json!({"health": {"build": build, "bank": bank, "seq": 1,
                                         "outputs": "stopped", "faults": []},
                                         "online": true, "stale": false, "age_ms": 500}),
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
        if self.script != Script::Unrouted && !self.stock_online && self.trial.is_none() {
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

    fn stock_trial(&mut self, trial: &TrialRequest) -> Result<()> {
        assert_eq!(trial.image, "replacement");
        assert_eq!(trial.server_token, TOKEN);
        require_none(&self.trial);
        self.trial = Some((trial.request_id.clone(), "submitted"));

        match self.script {
            Script::Refused => {
                self.report(2.0, "OTA_UPGRADE", None, Some(0x464));
                self.at(2.0, Change::Phase("reported_error"));
            }
            Script::Installs | Script::WrongBuild => {
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
                if self.offered {
                    self.line(90.0, "PLOTA1 manifest version=72 bytes=663552");
                    self.line(120.0, "PLOTA1 progress sectors=162/162 leases=2600");
                    self.line(130.0, "PLOTA1 activated version=72 sectors=162");
                    self.line(
                        135.0,
                        &format!("PLFW1 start build={build} bank=1 reset=0x0 mmu=1"),
                    );
                    self.line(136.0, "PLTRL1 confirmed=0 boot=1 limit=3");
                    self.line(170.0, "PLCNF1 confirmed=1 healthy_ms=30000");
                    self.at(172.0, Change::Health(build.into(), 1));
                    self.at(172.0, Change::Phase("replacement_healthy"));
                } else {
                    self.at(100.0, Change::Health(build.into(), 0));
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
    if promote {
        argv.push("--promote");
    }
    Cli::try_parse_from(argv).unwrap().args
}

fn staged() -> Staged {
    Staged {
        build: BUILD.into(),
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
            (REQUEST, "pass"),
            (DOWNLOAD, "pass"),
            (BANK0, "pass")
        ]
    );
    assert_eq!(
        feeder.served.as_deref(),
        Some(Path::new("private/verification/poc1-review"))
    );
    assert!(!feeder.offered, "no promotion offer without --promote");
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
            (REQUEST, "pass"),
            (DOWNLOAD, "pass"),
            (PROMOTE, "pass")
        ]
    );
    // Offered before stock was asked: the recovery role asks at its boot.
    assert_eq!(feeder.actions, ["offer before the request: true"]);
    assert!(feeder.offered);
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
            (REQUEST, "pass"),
            (DOWNLOAD, "fail"),
            (BANK0, "skip")
        ]
    );
    let Verdict::Fail(reason) = &reports[2].1 else {
        unreachable!()
    };
    assert!(reason.contains("error 0x464"), "{reason}");
    assert!(recovery(&session_reports(&reports)).is_some());
}

#[test]
fn another_build_starting_after_activation_fails_the_install() {
    let mut feeder = StockFeeder::new(Script::WrongBuild);
    let reports = session(&mut feeder, false);

    let Verdict::Fail(reason) = &reports[2].1 else {
        panic!("{:?}", reports[2])
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
fn the_steps_end_with_the_promotion_only_when_asked() {
    assert_eq!(args(false).steps(), [READY, REQUEST, DOWNLOAD, BANK0]);
    assert_eq!(args(true).steps(), [READY, REQUEST, DOWNLOAD, PROMOTE]);
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
