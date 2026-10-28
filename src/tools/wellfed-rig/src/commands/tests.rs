//! No device I/O: loader sequencing, refusals and update stop rules.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;

use serde_json::{Value, json};
use wellfed_formats::flash_plan::{GiveWayPlan, Kind, PlannedSector, Region};

use super::*;
use crate::commands::ota::{ArgError, OtaError, Source};
use crate::device::{Answer, DeviceError, DeviceStatus};
use crate::tools::{Capture, CaptureFailure, Captured, InstallJob, Installing, PushError};
use wellfed_formats::bench::VERIFICATION;
use wellfed_formats::bench::progress::{FlashTool, StepState};
use wellfed_relay::FakeBoard;

fn sector() -> PlannedSector {
    PlannedSector {
        offset: 0x186000,
        bytes: 0x1000,
        before_sha256: "b".into(),
        staged_sha256: "s".into(),
    }
}

/// A development plan staging `sectors` with `packets` staging packets.
fn development(sectors: usize, packets: u64) -> DevelopmentPlan {
    DevelopmentPlan {
        schema_version: 1,
        kind: Kind::Bank1Update,
        bank: 1,
        interrupted: false,
        source_ota_sha256: None,
        target_ota_sha256: "t".into(),
        source_payload_bytes: 0,
        target_payload_bytes: 0,
        region: Region {
            offset: 0x186000,
            bytes: 0xa4000,
        },
        current_region_sha256: "c".into(),
        staged_region_sha256: "s".into(),
        proposed_region_sha256: "p".into(),
        ordered_sectors: vec![sector(); sectors],
        erase_sectors: sectors as u64,
        staging_packets: packets,
        commit_packets: packets.min(1),
        additional_commit_erases: 0,
        retained_tail_sha256: "r".into(),
        scope: "test".into(),
        other_bank_sha256: None,
        restore_marker: false,
    }
}

/// What the fake flash client's `prepare` writes for `command`.
fn plan(command: &str) -> String {
    let operation = match command {
        "give-way" => json!(GiveWayPlan {
            schema_version: 1,
            kind: Kind::GiveWay,
            bank: 1,
            offset: 0x186000,
            fallback_ota_sha256: "f".into(),
            bank0_region_sha256: "z".into(),
            before_sha256: "b".into(),
            after_sha256: "a".into(),
            program_frame_sha256: "p".into(),
            scope: "test".into(),
        }),
        // Stock recovery's plan has its own shape (wellfed-flash `stock::Reviewed`).
        "stock-recovery" => {
            return json!({"kind": "original_stock_bank1_recovery", "recovery_plan": {
                "baseline_sha256": "b", "target_bank": 1, "changed_sector_count": 204}})
            .to_string();
        }
        _ => json!(development(1, 4)),
    };
    json!({"operation_plan": operation}).to_string()
}

fn loader_result() -> Observation {
    Observation {
        bytes: 120,
        nak_bytes: 99,
        longest_nak_run: 99,
        download_banner_seen: true,
        stock_startup_markers: 0,
        fault_marker_seen: false,
    }
}

/// An application boot: some output, no loader banner.
fn boot_result() -> Observation {
    Observation {
        bytes: 10,
        ..Observation::default()
    }
}

#[derive(Default)]
struct Calls {
    observed: Vec<PathBuf>,
    flash: Vec<Vec<String>>,
    flash_codes: VecDeque<i32>,
    results: VecDeque<Observation>,
    analyzer_code: i32,
    /// Analyzer launches that exit while arming, before the one that runs.
    analyzer_early_exits: u32,
    /// Analyzer launches so far.
    analyzer_launches: u32,
    /// Analyzers killed so far.
    analyzer_kills: u32,
    sigrok: bool,
    /// UART bytes each capture records, in order.
    rx: VecDeque<Vec<u8>>,
    /// Each capture's serial-events.jsonl, in order; none when empty.
    chunks: VecDeque<String>,
    /// The checkout, where a successful fake review writes its output.
    root: PathBuf,
    /// `wellfed-build` runs: builds and reviews.
    builds: Vec<Vec<String>>,
    /// Exit codes of the build and review children, in order.
    codes: VecDeque<i32>,
    /// What `wellfed-build source-id --image` prints; none fails the query.
    source_id: Option<String>,
    build_queries: Vec<Vec<String>>,
    /// Device API status checks (key file, server config) and their
    /// answers (an error's text); then the default.
    queries: Vec<(PathBuf, PathBuf)>,
    statuses: VecDeque<Result<DeviceStatus, DeviceError>>,
    status_default: Option<DeviceStatus>,
    /// Every child in order: `build <args>`, `local <args>`, `status <keys>`.
    ran: Vec<String>,
    /// Whether a full read's bank 1 starts with the loader marker.
    bank1_valid: bool,
    /// Updates pushed over the device API, and how each push ends (`Ok` by
    /// default); a refusal to start fails the push before it runs.
    installs: Vec<InstallJob>,
    install_results: VecDeque<Result<(), Arc<PushError>>>,
    install_refused: bool,
    /// What a cancel over the device API answers.
    cancel_answer: Option<Answer>,
}

/// A push that ends at once, as scripted.
struct FakeInstalling(Option<Result<(), Arc<PushError>>>);

impl Installing for FakeInstalling {
    fn finished(&mut self) -> Option<Result<(), Arc<PushError>>> {
        self.0.clone()
    }
    fn notes(&mut self) -> Vec<String> {
        Vec::new()
    }
    fn stop(&mut self) {}
}

#[derive(Clone, Default)]
struct FakeTools(Rc<RefCell<Calls>>);

struct FakeObserver(Observation, PathBuf, Option<Vec<u8>>, Option<String>);

impl Observer for FakeObserver {
    fn finish(&mut self) -> Result<Observation> {
        if let Some(rx) = self.2.take() {
            fs::create_dir_all(&self.1)?;
            fs::write(self.1.join("serial-rx.bin"), rx)?;
        }
        if let Some(chunks) = self.3.take() {
            fs::write(self.1.join("serial-events.jsonl"), chunks)?;
        }
        Ok(self.0.clone())
    }
    fn kill(&mut self) {}
}

/// The bench commands never chain captures; the suite's own tests use a rig.
struct NoCapture;

impl Capture for NoCapture {
    fn segments(&mut self) -> Vec<PathBuf> {
        Vec::new()
    }
    fn failure(&mut self) -> Option<Arc<CaptureFailure>> {
        None
    }
    fn stop(&mut self) {}
}

struct FakeAnalyzer {
    output: PathBuf,
    code: i32,
    running: bool,
    calls: Rc<RefCell<Calls>>,
}

impl Analyzer for FakeAnalyzer {
    fn running(&mut self) -> bool {
        self.running
    }
    fn wait(&mut self, _: Duration) -> Result<Option<i32>> {
        // Recorded while the feeder was being powered.
        fs::write(&self.output, [0u8; 10])?;
        Ok(Some(self.code))
    }
    fn kill(&mut self) {
        self.calls.borrow_mut().analyzer_kills += 1;
    }
    fn stderr(&mut self) -> String {
        String::new()
    }
}

impl Tools for FakeTools {
    fn observe(&mut self, _: &str, output: &Path, _: Duration) -> Result<Box<dyn Observer>> {
        let mut calls = self.0.borrow_mut();
        calls.observed.push(output.to_path_buf());
        let result = calls.results.pop_front().unwrap_or_else(loader_result);
        let rx = calls.rx.pop_front();
        let chunks = calls.chunks.pop_front();
        Ok(Box::new(FakeObserver(
            result,
            output.to_path_buf(),
            rx,
            chunks,
        )))
    }

    fn flash(&mut self, args: &[String], _: &mut Status) -> Result<i32> {
        let mut calls = self.0.borrow_mut();
        calls.flash.push(args.to_vec());
        let code = calls.flash_codes.pop_front().unwrap_or(0);
        // A successful prepare writes its plan to --output.
        if code == 0 && args[1] == "prepare" {
            let at = args.iter().position(|a| a == "--output").unwrap();
            fs::write(&args[at + 1], plan(&args[0]))?;
        }
        // A full read writes the image up to bank 1's marker.
        if code == 0 && args[..2] == ["read", "full"] {
            let output = PathBuf::from(flag(args, "--output"));
            fs::create_dir_all(&output)?;
            let mut image = vec![0xffu8; 0x186000];
            image.extend(if calls.bank1_valid {
                *b"81958711"
            } else {
                *b"\x001958711"
            });
            fs::write(output.join("flash-read.bin"), image)?;
        }
        Ok(code)
    }

    fn install(&mut self, job: &InstallJob) -> Result<Box<dyn Installing>> {
        let mut calls = self.0.borrow_mut();
        calls.ran.push(format!(
            "install {} {}",
            job.manifest.display(),
            job.image.display()
        ));
        calls.installs.push(job.clone());
        if calls.install_refused {
            return Err(DeviceError::Closed.into());
        }
        let result = calls.install_results.pop_front();
        Ok(Box::new(FakeInstalling(result)))
    }

    fn cancel_update(&mut self, keys: &Path, _config: &Path) -> Result<Answer> {
        let mut calls = self.0.borrow_mut();
        calls.ran.push(format!("cancel {}", keys.display()));
        Ok(calls.cancel_answer.clone().unwrap_or(Answer::Unexpected))
    }

    fn device_status(&mut self, keys: &Path, config: &Path) -> Result<DeviceStatus> {
        let mut calls = self.0.borrow_mut();
        calls
            .queries
            .push((keys.to_path_buf(), config.to_path_buf()));
        calls.ran.push(format!("status {}", keys.display()));
        match calls.statuses.pop_front() {
            Some(answer) => Ok(answer?),
            None => calls
                .status_default
                .clone()
                .ok_or_else(|| DeviceError::Closed.into()),
        }
    }

    fn build(&mut self, args: &[String], _: &mut Status) -> Result<i32> {
        let mut calls = self.0.borrow_mut();
        calls.builds.push(args.to_vec());
        calls.ran.push(format!("build {}", args.join(" ")));
        let code = calls.codes.pop_front().unwrap_or(0);
        // A successful review freezes the build at --output.
        if code == 0 && args[0] == "review" {
            let manifest = json!({"startup_identity": BUILD, "startup_motion_policy": null,
                                  "firmware": "feeder", "variant": "stall"});
            freeze(
                &calls.root,
                &calls.root.join(flag(args, "--output")),
                &manifest,
            );
        }
        Ok(code)
    }

    fn build_query(&mut self, args: &[String]) -> Result<Captured> {
        let mut calls = self.0.borrow_mut();
        calls.build_queries.push(args.to_vec());
        Ok(match &calls.source_id {
            Some(id) => Captured {
                code: 0,
                stdout: format!("{id}\n"),
                stderr: String::new(),
            },
            None => Captured {
                code: 1,
                ..Captured::default()
            },
        })
    }

    fn capture(
        &mut self,
        _: &str,
        _: Duration,
        _: Box<dyn Fn(u32) -> PathBuf + Send>,
    ) -> Result<Box<dyn Capture>> {
        Ok(Box::new(NoCapture))
    }

    fn analyzer(&mut self, output: &Path, _: Duration, _: u64) -> Result<Box<dyn Analyzer>> {
        let mut calls = self.0.borrow_mut();
        calls.analyzer_launches += 1;
        let running = calls.analyzer_launches > calls.analyzer_early_exits;
        let code = calls.analyzer_code;
        drop(calls);
        Ok(Box::new(FakeAnalyzer {
            output: output.to_path_buf(),
            code,
            running,
            calls: self.0.clone(),
        }))
    }

    fn sigrok_available(&self) -> bool {
        self.0.borrow().sigrok
    }
    fn flash_tool(&self) -> Option<FlashTool> {
        None
    }
}

struct Rig {
    bench: Bench,
    board: FakeBoard,
    tools: FakeTools,
    _tmp: tempfile::TempDir,
}

fn rig() -> Rig {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir(tmp.path().join("private")).unwrap();
    wellfed_signing::keygen(&tmp.path().join(wellfed_signing::KEY_PATH)).unwrap();
    let board = FakeBoard::default();
    let tools = FakeTools::default();
    {
        let mut calls = tools.0.borrow_mut();
        calls.sigrok = true;
        calls.root = tmp.path().to_path_buf();
        calls.status_default = Some(device_status(BUILD, 1));
    }

    let bench = Bench {
        root: tmp.path().to_path_buf(),
        config: Config {
            relay_port: "relay".into(),
            uart_port: "uart".into(),
            baseline: "b".into(),
            rom_report: "r".into(),
            analyzer_channels: None,
        },
        relay: Relay::new(Box::new(board.clone())),
        status: Status::disabled(),
        tools: Box::new(tools.clone()),
        cancel: Cancel::new(),
        sleep: |_, _| Ok(()),
        off_time: Duration::from_secs(2),
        uart_when_off: UartWhenOff::Isolated,
    };

    Rig {
        bench,
        board,
        tools,
        _tmp: tmp,
    }
}

fn update(name: &str) -> UpdateArgs {
    UpdateArgs {
        name: name.into(),
        source: Some("s".into()),
        bank: 1,
        original_read: None,
        target: "t".into(),
        interrupted: false,
        restore_marker: false,
        attended_motion: false,
    }
}

fn execute(name: &str) -> ExecuteArgs {
    ExecuteArgs {
        update: update(name),
        expected_plan_sha256: "0".repeat(64),
        boot_seconds: 10.0,
    }
}

fn prepare_plan(rig: &Rig, name: &str) {
    let p = Paths::new(&rig.bench.root, name);
    fs::create_dir_all(p.plan.parent().unwrap()).unwrap();
    fs::write(&p.plan, r#"{"operation_plan": {"staging_packets": 4, "commit_packets": 1, "region": {"bytes": 4096}}}"#).unwrap();
}

#[test]
fn loader_holds_strap_across_power_on_and_leaves_feeder_powered() {
    let mut rig = rig();
    let out = rig.bench.root.join("out");

    rig.bench.loader(&out).unwrap();

    assert_eq!(rig.tools.0.borrow().observed, [out]);
    assert_eq!(
        rig.board.log(),
        [
            (2, false),
            (1, false),
            (3, false),
            (4, false),
            (2, true),
            (3, true),
            (4, true),
            (1, true),
            (2, false)
        ]
    );
    assert_eq!(rig.board.state(), [true, false, true, true]);
}

#[test]
fn the_uart_is_isolated_while_off_unless_asked_to_stay_connected() {
    let mut rig = rig();
    rig.bench.off().unwrap();
    assert_eq!(rig.board.state(), [false; 4]);
    rig.bench.on().unwrap();
    assert_eq!(rig.board.state(), [true, false, true, true]);

    rig.bench.uart_when_off = UartWhenOff::Connected;
    rig.bench.off().unwrap();
    assert_eq!(rig.board.state(), [false, false, true, true]);
}

#[test]
fn a_capture_connects_a_powered_feeders_uart_only() {
    let mut rig = rig();
    rig.bench
        .capture("unpowered", Duration::from_secs(1))
        .unwrap();
    assert_eq!(
        rig.board.state(),
        [false; 4],
        "an unpowered feeder stays isolated"
    );

    rig.board.inner.lock().unwrap().on[1] = true;
    rig.bench
        .capture("powered", Duration::from_secs(1))
        .unwrap();
    assert_eq!(rig.board.state(), [true, false, true, true]);
}

#[test]
fn unconfirmed_loader_powers_off() {
    let confirmed = loader_result();
    for (field, result) in [
        (
            "download_banner_seen",
            Observation {
                download_banner_seen: false,
                ..confirmed.clone()
            },
        ),
        (
            "longest_nak_run",
            Observation {
                longest_nak_run: 3,
                ..confirmed.clone()
            },
        ),
        (
            "fault_marker_seen",
            Observation {
                fault_marker_seen: true,
                ..confirmed.clone()
            },
        ),
        (
            "stock_startup_markers",
            Observation {
                stock_startup_markers: 1,
                ..confirmed.clone()
            },
        ),
    ] {
        let mut rig = rig();
        rig.tools.0.borrow_mut().results.push_back(result);
        let out = rig.bench.root.join("out");

        assert!(rig.bench.loader(&out).is_err(), "{field}");
        assert_eq!(rig.board.state(), [false; 4], "{field}");
    }
}

#[test]
fn reused_capture_names_are_refused_before_any_relay_change() {
    let mut rig = rig();
    let existing = rig.bench.root.join(CAPTURES).join("used");
    fs::create_dir_all(&existing).unwrap();

    let error = rig
        .bench
        .boot(&existing, Duration::from_secs(10))
        .unwrap_err();
    assert!(
        matches!(&error, Error::Command(CommandsError::NameTaken { path }) if path == "private/captures/used"),
        "{error:?}"
    );
    // The refusal the operator reads, exactly.
    assert_eq!(error.to_string(), "Refusing to reuse private/captures/used");
    let error = rig.bench.loader(&existing).unwrap_err();
    assert!(
        matches!(error, Error::Command(CommandsError::NameTaken { .. })),
        "{error:?}"
    );
    assert!(rig.bench.capture("used", Duration::from_secs(5)).is_err());

    assert!(rig.board.log().is_empty());
}

#[test]
fn boot_refuses_loader_output() {
    let mut rig = rig();
    let out = rig.bench.root.join("out");

    let error = rig.bench.boot(&out, Duration::from_secs(10)).unwrap_err();
    assert!(
        matches!(error, Error::Command(CommandsError::ReachedLoader)),
        "{error:?}"
    );
}

#[test]
fn execute_requires_uninterrupted_power() {
    let mut rig = rig();
    prepare_plan(&rig, "unit");

    assert!(rig.bench.flash_execute(&execute("unit")).is_err());
    assert!(rig.tools.0.borrow().flash.is_empty());
}

#[test]
fn lengths_a_wait_cannot_take_are_refused() {
    assert_eq!(
        seconds_arg("--seconds", 1.5).unwrap(),
        Duration::from_millis(1500)
    );
    assert_eq!(
        minutes_arg("--minutes", 0.5).unwrap(),
        Duration::from_secs(30)
    );
    for value in [-1.0, f64::NAN, f64::INFINITY, 1e19] {
        let error = seconds_arg("--seconds", value).unwrap_err();
        assert!(
            matches!(
                error,
                Error::Command(CommandsError::BadLength {
                    flag: "--seconds",
                    ..
                })
            ),
            "{error:?}"
        );
    }
    assert_eq!(
        seconds_arg("--off-seconds", -2.0).unwrap_err().to_string(),
        "--off-seconds -2 must be a finite, non-negative number, at most a week"
    );

    // Refused before the plan is looked at or a relay moves.
    let mut rig = rig();
    let mut args = execute("unit");
    args.boot_seconds = -1.0;
    let error = rig.bench.flash_execute(&args).unwrap_err();
    assert!(
        matches!(error, Error::Command(CommandsError::BadLength { .. })),
        "{error:?}"
    );
    assert!(rig.board.log().is_empty());
}

#[test]
fn execute_requires_a_prepared_single_use_plan() {
    let mut rig = rig();
    rig.bench.on().unwrap();

    let error = rig.bench.flash_execute(&execute("unit")).unwrap_err();
    assert!(
        matches!(error, Error::Command(CommandsError::PlanRequired)),
        "{error:?}"
    );

    // An existing execute directory means the plan already ran.
    prepare_plan(&rig, "unit");
    fs::create_dir_all(Paths::new(&rig.bench.root, "unit").execute).unwrap();
    assert!(rig.bench.flash_execute(&execute("unit")).is_err());
    assert!(rig.tools.0.borrow().flash.is_empty());
}

#[test]
fn stopped_execute_never_boots_or_cuts_power() {
    let mut rig = rig();
    rig.bench.on().unwrap();
    prepare_plan(&rig, "unit");
    rig.tools.0.borrow_mut().flash_codes.push_back(1);

    assert_eq!(rig.bench.flash_execute(&execute("unit")).unwrap(), 1);

    let calls = rig.tools.0.borrow();
    assert_eq!(calls.flash.len(), 1);
    assert_eq!(calls.flash[0][..2], ["update", "execute"]);
    assert!(calls.observed.is_empty(), "no boot capture after a stop");
    assert!(rig.board.state()[0]);
}

#[test]
fn completed_execute_passes_the_reviewed_hash_and_boots() {
    let mut rig = rig();
    rig.bench.on().unwrap();
    prepare_plan(&rig, "unit");
    rig.tools.0.borrow_mut().results.push_back(boot_result());

    assert_eq!(rig.bench.flash_execute(&execute("unit")).unwrap(), 0);

    let calls = rig.tools.0.borrow();
    let args = &calls.flash[0];
    let at = args
        .iter()
        .position(|a| a == "--expected-plan-sha256")
        .unwrap();
    assert_eq!(args[at + 1], "0".repeat(64));
    assert_eq!(calls.observed, [Paths::new(&rig.bench.root, "unit").boot]);
}

#[test]
fn prepare_stops_at_the_first_failed_step_with_power_on() {
    let mut rig = rig();
    rig.tools.0.borrow_mut().flash_codes.extend([0, 1]);

    assert_eq!(rig.bench.flash_prepare(&update("unit")).unwrap(), 1);

    let calls = rig.tools.0.borrow();
    let ran: Vec<_> = calls.flash.iter().map(|a| a[..2].join(" ")).collect();
    assert_eq!(ran, ["read small", "update inspect"]);
    assert!(rig.board.state()[0]);
}

#[test]
fn prepare_records_the_matching_execute_line() {
    let mut rig = rig();
    rig.bench.status = Status::new(&rig.bench.root, rig.bench.root.join("status"));
    rig.bench
        .status
        .begin(BenchCommand::FlashPrepare, Some("unit"), &[])
        .unwrap();
    fs::create_dir_all(rig.bench.root.join(VERIFICATION)).unwrap();

    assert_eq!(rig.bench.flash_prepare(&update("unit")).unwrap(), 0);

    let ran: Vec<_> = rig
        .tools
        .0
        .borrow()
        .flash
        .iter()
        .map(|a| a[..2].join(" "))
        .collect();
    assert_eq!(ran, ["read small", "update inspect", "update prepare"]);

    let sha = sha256(plan("update").as_bytes());
    assert_eq!(
        rig.bench.status.state().unwrap().next.as_deref(),
        Some(format!(
            "wellfed-bench flash-execute unit --bank 1 --source s --target t --expected-plan-sha256 {sha}"
        ).as_str())
    );

    let mut restore = update("unit");
    restore.restore_marker = true;
    restore.interrupted = true;
    assert!(execute_line(&restore, "ab").contains("--restore-marker --interrupted"));
}

#[test]
fn give_way_prepare_then_execute() {
    let mut rig = rig();
    fs::create_dir_all(rig.bench.root.join(VERIFICATION)).unwrap();
    let args = GiveWayArgs {
        name: "gw".into(),
        fallback: "f".into(),
    };
    assert_eq!(rig.bench.give_way_prepare(&args).unwrap(), 0);

    rig.tools.0.borrow_mut().results.push_back(boot_result());
    let execute = GiveWayExecuteArgs {
        give_way: args,
        expected_plan_sha256: sha256(plan("give-way").as_bytes()),
        boot_seconds: 10.0,
    };
    assert_eq!(rig.bench.give_way_execute(&execute).unwrap(), 0);

    let calls = rig.tools.0.borrow();
    let last = calls.flash.last().unwrap();
    assert_eq!(last[..2], ["give-way", "execute"]);
    assert!(last.contains(&"--fallback".to_string()));
}

fn stock(name: &str, fallback: Option<&str>) -> StockArgs {
    StockArgs {
        name: name.into(),
        fallback: fallback.map(PathBuf::from),
    }
}

fn stock_banner() -> Observation {
    Observation {
        bytes: 4000,
        stock_startup_markers: 1,
        ..Observation::default()
    }
}

#[test]
fn stock_prepare_then_execute_puts_the_original_back_into_bank_1() {
    let mut rig = rig();
    fs::create_dir_all(rig.bench.root.join(VERIFICATION)).unwrap();
    let args = stock("st", None);
    assert_eq!(rig.bench.stock_prepare(&args).unwrap(), 0);

    {
        let calls = rig.tools.0.borrow();
        let commands: Vec<_> = calls.flash.iter().map(|a| a[..2].join(" ")).collect();
        assert_eq!(
            commands,
            ["read small", "read full", "stock-recovery prepare"]
        );
        // The recovery continues the full read of this helper session.
        let prepare = calls.flash.last().unwrap();
        assert!(flag(prepare, "--current").ends_with("st-full/flash-read.bin"));
        assert!(flag(prepare, "--small-report").ends_with("st-bounded/report.json"));
    }

    rig.tools.0.borrow_mut().results.push_back(stock_banner());
    let execute = StockExecuteArgs {
        stock: args,
        expected_plan_sha256: sha256(plan("stock-recovery").as_bytes()),
        boot_seconds: 10.0,
    };
    assert_eq!(rig.bench.stock_execute(&execute).unwrap(), 0);

    let calls = rig.tools.0.borrow();
    let last = calls.flash.last().unwrap();
    assert_eq!(last[..2], ["stock-recovery", "execute"]);
    assert_eq!(
        flag(last, "--expected-plan-sha256"),
        execute.expected_plan_sha256
    );
    assert!(flag(last, "--plan").ends_with("st-plan.json"));
    assert!(flag(last, "--current").ends_with("st-full/flash-read.bin"));
}

#[test]
fn a_valid_bank_1_needs_the_give_way_first() {
    let mut rig = rig();
    rig.tools.0.borrow_mut().bank1_valid = true;

    let error = rig.bench.stock_prepare(&stock("st", None)).unwrap_err();
    assert!(
        matches!(&error, Error::Command(CommandsError::Bank1Valid { name }) if name == "st"),
        "{error:?}"
    );
    let calls = rig.tools.0.borrow();
    assert!(!calls.flash.iter().any(|a| a[0] == "stock-recovery"));
}

#[test]
fn with_a_fallback_the_stock_commands_give_way_first() {
    let mut rig = rig();
    fs::create_dir_all(rig.bench.root.join(VERIFICATION)).unwrap();
    let args = stock("st", Some("bank0-review"));
    assert_eq!(rig.bench.stock_prepare(&args).unwrap(), 0);
    {
        let calls = rig.tools.0.borrow();
        let last = calls.flash.last().unwrap();
        assert_eq!(last[..2], ["give-way", "prepare"]);
        assert!(flag(last, "--output").ends_with("st-giveway-plan.json"));
    }

    rig.tools.0.borrow_mut().results.push_back(boot_result());
    let execute = StockExecuteArgs {
        stock: args,
        expected_plan_sha256: sha256(plan("give-way").as_bytes()),
        boot_seconds: 10.0,
    };
    assert_eq!(rig.bench.stock_execute(&execute).unwrap(), 0);
    let calls = rig.tools.0.borrow();
    assert_eq!(calls.flash.last().unwrap()[..2], ["give-way", "execute"]);
    assert!(!calls.flash.iter().any(|a| a[0] == "stock-recovery"));
}

#[test]
fn a_stock_boot_without_its_banner_fails() {
    let mut rig = rig();
    fs::create_dir_all(rig.bench.root.join(VERIFICATION)).unwrap();
    let args = stock("st", None);
    assert_eq!(rig.bench.stock_prepare(&args).unwrap(), 0);

    rig.tools.0.borrow_mut().results.push_back(boot_result());
    let execute = StockExecuteArgs {
        stock: args,
        expected_plan_sha256: sha256(plan("stock-recovery").as_bytes()),
        boot_seconds: 10.0,
    };
    let error = rig.bench.stock_execute(&execute).unwrap_err();
    assert!(
        matches!(error, Error::Command(CommandsError::NoStockBanner)),
        "{error:?}"
    );
}

#[test]
fn stock_execute_needs_its_plan() {
    let mut rig = rig();
    let execute = StockExecuteArgs {
        stock: stock("none", None),
        expected_plan_sha256: "0".repeat(64),
        boot_seconds: 10.0,
    };
    let error = rig.bench.stock_execute(&execute).unwrap_err();
    assert!(
        matches!(error, Error::Command(CommandsError::PlanRequired)),
        "{error:?}"
    );
    assert!(rig.tools.0.borrow().flash.is_empty());
}

#[test]
fn bank_1_needs_a_source_before_any_relay_change() {
    let mut rig = rig();
    let mut args = update("unit");
    args.source = None;

    let error = rig.bench.flash_prepare(&args).unwrap_err();
    assert!(
        matches!(error, Error::Command(CommandsError::SourceRequired)),
        "{error:?}"
    );
    assert!(rig.board.log().is_empty());
}

#[test]
fn motion_targets_need_the_attended_flag() {
    let rig = rig();
    let target = rig.bench.root.join("target");
    fs::create_dir_all(&target).unwrap();
    let mut args = update("unit");
    args.target = target.clone();
    fs::write(
        target.join("build-manifest.json"),
        r#"{"startup_motion_policy": "fw16-pulse-v1"}"#,
    )
    .unwrap();

    let error = rig.bench.require_attended_motion(&args).unwrap_err();
    assert!(
        matches!(&error, Error::Command(CommandsError::MotionNotAttended { policy }) if policy == "fw16-pulse-v1"),
        "{error:?}"
    );
    args.attended_motion = true;
    rig.bench.require_attended_motion(&args).unwrap();

    args.attended_motion = false;
    fs::write(
        target.join("build-manifest.json"),
        r#"{"startup_motion_policy": null}"#,
    )
    .unwrap();
    rig.bench.require_attended_motion(&args).unwrap();

    // No manifest: a stock or diagnostic target with no policy.
    fs::remove_file(target.join("build-manifest.json")).unwrap();
    rig.bench.require_attended_motion(&args).unwrap();

    // A manifest that cannot be read is not taken as having no policy.
    for unreadable in ["{", r#"{"startup_motion_policy": 1}"#] {
        fs::write(target.join("build-manifest.json"), unreadable).unwrap();
        let error = rig.bench.require_attended_motion(&args).unwrap_err();
        assert!(
            matches!(
                error,
                Error::Command(CommandsError::MotionPolicyUnreadable { .. })
            ),
            "{unreadable}"
        );
    }
}

#[test]
fn session_names_are_single_components() {
    for name in ["ok", "fw10-review-01"] {
        check_name(name).unwrap();
    }

    for name in ["", "..", "a/b", "/abs", "."] {
        assert!(check_name(name).is_err(), "{name}");
    }
}

fn transients(mode: TransientMode) -> TransientArgs {
    TransientArgs {
        name: "unit".into(),
        cycles: 2,
        mode,
        probes: Probes::Driver,
        seconds: 8.0,
        samplerate: 1000,
    }
}

#[test]
fn boot_cycles_power_on_after_arming_and_end_off() {
    let mut rig = rig();
    assert_eq!(
        rig.bench
            .transients(&transients(TransientMode::Boot))
            .unwrap(),
        0
    );

    // Each cycle: strap released, power off and the UART isolated, then the
    // UART connected and power on, then off.
    assert_eq!(
        rig.board.log()[..8],
        [
            (2, false),
            (1, false),
            (3, false),
            (4, false),
            (3, true),
            (4, true),
            (1, true),
            (2, false)
        ]
    );
    assert!(!rig.board.state()[0]);

    let out = rig.bench.root.join(CAPTURES).join("unit");
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(out.join("manifest.json")).unwrap()).unwrap();
    let files: Vec<_> = manifest["cycles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["file"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(files, ["cycle-001.bin", "cycle-002.bin"]);
    assert_eq!(manifest["cycles"][0]["bytes"], 10);
    assert_eq!(
        manifest["channels"],
        json!({"ina": 0, "inb": 1, "outa": 2, "outb": 3})
    );
}

#[test]
fn output_probes_record_only_the_output_channels() {
    let mut rig = rig();
    let args = TransientArgs {
        probes: Probes::Outputs,
        ..transients(TransientMode::Boot)
    };
    rig.bench.transients(&args).unwrap();

    let out = rig.bench.root.join(CAPTURES).join("unit");
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(out.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["channels"], json!({"outa": 2, "outb": 3}));
}

#[test]
fn loader_cycles_hold_the_strap_across_power_on() {
    let mut rig = rig();
    rig.bench
        .transients(&transients(TransientMode::Loader))
        .unwrap();

    assert_eq!(
        rig.board.log()[..9],
        [
            (2, false),
            (1, false),
            (3, false),
            (4, false),
            (2, true),
            (3, true),
            (4, true),
            (1, true),
            (2, false)
        ]
    );
    assert_eq!(rig.board.state()[..2], [false, false]);
}

#[test]
fn missing_sigrok_refuses_before_any_relay_change() {
    let mut rig = rig();
    rig.tools.0.borrow_mut().sigrok = false;

    let error = rig
        .bench
        .transients(&transients(TransientMode::Boot))
        .unwrap_err();

    assert!(
        matches!(
            error,
            Error::Command(CommandsError::ToolMissing { tool: "sigrok-cli" })
        ),
        "{error:?}"
    );
    assert!(rig.board.log().is_empty());
}

#[test]
fn an_analyzer_that_exits_while_arming_is_launched_again() {
    let mut rig = rig();
    rig.tools.0.borrow_mut().analyzer_early_exits = 2;

    rig.bench
        .transients(&transients(TransientMode::Boot))
        .expect("the third launch runs");

    // Three launches for the first cycle, one for the second.
    assert_eq!(rig.tools.0.borrow().analyzer_launches, 4);
}

#[test]
fn an_analyzer_that_never_arms_stops_before_power_on() {
    let mut rig = rig();
    rig.tools.0.borrow_mut().analyzer_early_exits = 3;

    let error = rig
        .bench
        .transients(&transients(TransientMode::Boot))
        .expect_err("three launches exited");

    assert!(
        matches!(error, Error::Command(CommandsError::AnalyzerExited { .. })),
        "{error:?}"
    );
    assert_eq!(rig.tools.0.borrow().analyzer_launches, 3);
    // Only the cut and the isolation: no relay was closed.
    assert!(
        rig.board.log().iter().all(|&(_, on)| !on),
        "{:?}",
        rig.board.log()
    );
}

#[test]
fn an_interrupt_while_arming_kills_the_analyzer_before_power_on() {
    let mut rig = rig();
    // Only the analyzer's arming pause is interrupted.
    rig.bench.sleep = |_, span| {
        if span == Duration::from_secs(3) {
            Err(Error::Interrupted)
        } else {
            Ok(())
        }
    };

    let error = rig
        .bench
        .transients(&transients(TransientMode::Boot))
        .expect_err("interrupted");

    assert!(matches!(error, Error::Interrupted), "{error:?}");
    let calls = rig.tools.0.borrow();
    assert_eq!((calls.analyzer_launches, calls.analyzer_kills), (1, 1));
    assert!(
        rig.board.log().iter().all(|&(_, on)| !on),
        "{:?}",
        rig.board.log()
    );
}

#[test]
fn a_failed_capture_stops_with_power_off() {
    let mut rig = rig();
    rig.tools.0.borrow_mut().analyzer_code = 1;

    assert!(
        rig.bench
            .transients(&transients(TransientMode::Boot))
            .is_err()
    );
    assert_eq!(rig.board.state()[..2], [false, false]);
}

#[test]
fn update_traffic_matches_observed_installs() {
    // Totals observed on October 22, 2026 (bench): a 158-sector bank-0
    // install and a no-op, both over a 0xa4000-byte region.
    let close = |estimate: u64, actual: u64| estimate.abs_diff(actual) * 100 <= actual;

    let (tx, rx) = update_traffic(&development(158, 632));
    assert!(close(tx, 667_579) && close(rx, 4_081_733), "{tx} {rx}");

    let (tx, rx) = update_traffic(&development(0, 0));
    assert!(tx == 0 && close(rx, 703_173), "{rx}");
}

const BUILD: &str = "8a13e59efa15e56b5f5e32825909dc45e381a2027abc65d8de6658c40b302447";

/// Freeze a build review in `review` under `root` as `wellfed-build
/// review` would, verifiable as `wellfed-flash` verifies one: a synthetic
/// image carrying the build's identity, the build manifest with `manifest`'s
/// fields, and a review.json binding both.
fn freeze(root: &Path, review: &Path, manifest: &Value) {
    let image = wellfed_image::fixtures::image_carrying(
        format!("PLFW1 start build={BUILD} bank=").as_bytes(),
    );
    wellfed_image::fixtures::freeze_build(root, review, &image, manifest).unwrap();
}

/// A version 2 manifest the bench key signed, through the firmware's
/// check (with the bench's own downgrade statement, which a stock image
/// always needs).
fn signed_by_bench(rig: &Rig, file: &str) -> wellfed_core::ota::Manifest {
    use wellfed_core::ota::offer::{Context, KeyRing, admit_offer};
    let key = wellfed_signing::Key::load(&rig.bench.root.join(wellfed_signing::KEY_PATH)).unwrap();
    let bytes = fs::read(rig.bench.root.join(VERIFICATION).join(file)).unwrap();
    let built_in = [key.public()];
    let context = Context {
        ring: KeyRing {
            built_in: &built_in,
            added: None,
            enrolled: &[],
            revoked: &[],
        },
        floor: 0,
        device: [0; 32],
        running: None,
    };
    let downgrade = key.downgrade(&bytes, &context.device, 0).unwrap();
    admit_offer(&bytes, Some(&downgrade), &context)
        .unwrap()
        .signed
        .manifest
}

/// A frozen review under private/verification, and `ota` arguments for it.
fn ota_review(rig: &Rig, manifest: Value) -> OtaArgs {
    let review = rig.bench.root.join(VERIFICATION).join("review");
    // The product firmware, the one the status check applies to, unless named.
    let mut manifest = manifest;
    if let Some(fields) = manifest.as_object_mut() {
        fields.entry("firmware").or_insert_with(|| json!("feeder"));
    }
    freeze(&rig.bench.root, &review, &manifest);
    OtaArgs {
        name: "unit".into(),
        review: Some("private/verification/review".into()),
        build: false,
        rebuild: false,
        firmware: None,
        variant: None,
        config: "private/server.json".into(),
        keys: "private/wellfed-owner.json".into(),
        version: None,
        minutes: 20.0,
        attended_motion: false,
        downgrade: false,
        fault: None,
        cancel: false,
    }
}

fn plain_review(rig: &Rig) -> OtaArgs {
    ota_review(
        rig,
        json!({"startup_identity": BUILD, "startup_motion_policy": null, "variant": "trial-reset"}),
    )
}

fn signed_manifest(rig: &Rig, file: &str, version: u32) {
    let mut bytes = b"PLOTA1\0\0".to_vec();
    bytes.extend(version.to_le_bytes());
    bytes.resize(148, 0);
    fs::write(rig.bench.root.join(VERIFICATION).join(file), bytes).unwrap();
}

fn flag<'a>(args: &'a [String], name: &str) -> &'a str {
    let at = args.iter().position(|a| a == name).unwrap();
    &args[at + 1]
}

fn captured(rig: &Rig) -> Vec<String> {
    rig.tools
        .0
        .borrow()
        .observed
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn ota_signs_one_above_the_highest_signed_version_then_offers() {
    let mut rig = rig();
    let args = plain_review(&rig);
    signed_manifest(&rig, "old-v3.manifest", 3);
    signed_manifest(&rig, "old-v12.manifest", 12);
    // Neither a full-length header nor a manifest name: ignored.
    fs::write(
        rig.bench.root.join(VERIFICATION).join("junk.manifest"),
        b"PLOTA1\0\0\xff\xff\xff\xff",
    )
    .unwrap();
    rig.tools.0.borrow_mut().rx.push_back(
        format!("PLFW1 start build={BUILD} bank=1\r\nPLCNF1 confirmed=1 healthy_ms=30000\r\n")
            .into_bytes(),
    );

    assert_eq!(rig.bench.ota(&args).unwrap(), 0);

    let signed = signed_by_bench(&rig, "unit-v13.manifest");
    assert_eq!(signed.version, 13);
    assert_eq!(wellfed_formats::hex::encode(&signed.build_id), BUILD);

    let calls = rig.tools.0.borrow();
    let [job] = &calls.installs[..] else {
        panic!("{:?}", calls.installs)
    };
    assert_eq!(
        job.manifest,
        Path::new("private/verification/unit-v13.manifest")
    );
    assert_eq!(
        job.image,
        Path::new("private/verification/review/image.bin")
    );
    assert_eq!(job.downgrade, None);
    assert_eq!(job.keys, Path::new("private/wellfed-owner.json"));
    drop(calls);
    assert!(rig.board.log().is_empty(), "no relay change");
}

/// The pinned stock image installs by its payload hash, as stock, and
/// stock serves no device API to poll. Needs the private stock image.
#[test]
#[ignore = "needs the pinned stock image under private/"]
fn ota_installs_the_pinned_stock_review_by_its_payload_hash_without_health() {
    let repo = wellfed_build::Repo::here().unwrap();
    let scratch = tempfile::tempdir_in(repo.root.join("private")).unwrap();
    let frozen = scratch.path().join("stock");
    let reviewed = wellfed_build::review::freeze(
        &repo,
        wellfed_formats::review::ImageKind::Stock,
        Path::new("unused"),
        &frozen,
    )
    .unwrap();

    let mut rig = rig();
    let mut args = plain_review(&rig);
    let review = rig.bench.root.join(VERIFICATION).join("stock-review");
    fs::create_dir_all(&review).unwrap();
    for file in ["image.bin", "review.json"] {
        fs::copy(frozen.join(file), review.join(file)).unwrap();
    }
    args.review = Some("private/verification/stock-review".into());
    rig.tools.0.borrow_mut().rx.push_back(
        b"PLWCH1 give_way=1 version=1 restart=1\r\nPLOTA1 activated version=1\r\n\
          [0]app_main:version:1.1.15 DATE:Jul 16 2026\r\n"
            .to_vec(),
    );

    assert_eq!(rig.bench.ota(&args).unwrap(), 0);

    let signed = signed_by_bench(&rig, "unit-v1.manifest");
    assert_eq!(
        wellfed_formats::hex::encode(&signed.build_id),
        reviewed.payload_sha256
    );
    let calls = rig.tools.0.borrow();
    let [job] = &calls.installs[..] else {
        panic!("{:?}", calls.installs)
    };
    // Stock always needs the owner's downgrade statement.
    assert_eq!(
        job.downgrade.as_deref(),
        Some(Path::new(wellfed_signing::KEY_PATH))
    );
    assert!(calls.queries.is_empty(), "stock has no status to poll");
}

#[test]
fn ota_cancel_sends_one_cancel_and_nothing_else() {
    let mut rig = rig();
    let args = OtaArgs {
        review: None,
        cancel: true,
        ..ota_review(&rig, json!({"startup_identity": BUILD}))
    };
    let view = wellfed_model::UpdateView {
        phase: wellfed_model::UpdatePhase::Idle,
        bank: 0,
        min_version: 4,
        digest: None,
        version: 0,
        image_len: 0,
        written: 0,
        wanted: Vec::new(),
        stopped: None,
    };
    rig.tools.0.borrow_mut().cancel_answer = Some(Answer::Update(view));
    assert_eq!(rig.bench.ota(&args).unwrap(), 0);

    let calls = rig.tools.0.borrow();
    assert_eq!(calls.ran, [format!("cancel {}", args.keys.display())]);
    assert!(calls.installs.is_empty());
    drop(calls);

    rig.tools.0.borrow_mut().cancel_answer = Some(Answer::Unexpected);
    assert_eq!(
        rig.bench.ota(&args).unwrap(),
        1,
        "no update state: not cancelled"
    );
}

/// Only the pinned stock image may be reviewed as stock: relabelling a
/// build would skip the motion gate and the health check.
#[test]
fn ota_refuses_a_stock_review_of_another_image() {
    let mut rig = rig();
    let args = ota_review(
        &rig,
        json!({"startup_identity": BUILD, "startup_motion_policy": "fw16-pulse-v1"}),
    );
    let review = rig.bench.root.join(VERIFICATION).join("review");
    fs::remove_file(review.join("build-manifest.json")).unwrap();
    let mut frozen: Value =
        serde_json::from_slice(&fs::read(review.join("review.json")).unwrap()).unwrap();
    frozen["image_kind"] = json!("stock");
    frozen["manifest_sha256"] = Value::Null;
    frozen["review"] = json!({"stock_sha256": frozen["ota_sha256"]});
    fs::write(review.join("review.json"), frozen.to_string()).unwrap();

    let error = rig.bench.ota(&args).unwrap_err();
    assert!(
        matches!(
            signing(&error),
            wellfed_signing::Error::Review(wellfed_image::Error::Review(
                wellfed_image::review::ReviewError::NotOriginalStock
            ))
        ),
        "{error:?}"
    );
    assert!(rig.tools.0.borrow().installs.is_empty());
}

/// The signer's refusal of the offered review.
fn signing(error: &Error) -> &wellfed_signing::Error {
    match error {
        Error::Signing { error, .. } => error,
        other => panic!("not a signing refusal: {other:?}"),
    }
}

#[test]
fn ota_takes_a_given_version() {
    let mut rig = rig();
    let mut args = plain_review(&rig);
    args.version = Some(40);
    rig.tools.0.borrow_mut().install_refused = true;

    assert_eq!(rig.bench.ota(&args).unwrap(), 1);
    assert_eq!(signed_by_bench(&rig, "unit-v40.manifest").version, 40);
    assert!(captured(&rig).is_empty());
}

#[test]
fn ota_refuses_a_motion_review_without_the_attended_flag() {
    let mut rig = rig();
    let mut args = ota_review(
        &rig,
        json!({"startup_identity": BUILD, "startup_motion_policy": "fw16-pulse-v1"}),
    );

    let error = rig.bench.ota(&args).unwrap_err();
    assert!(
        matches!(
            error,
            Error::Command(CommandsError::MotionNotAttended { .. })
        ),
        "{error:?}"
    );
    assert!(rig.tools.0.borrow().installs.is_empty());

    args.attended_motion = true;
    rig.tools.0.borrow_mut().install_refused = true;
    assert_eq!(rig.bench.ota(&args).unwrap(), 1);
}

#[test]
fn ota_refuses_a_review_without_a_build_id() {
    let mut rig = rig();
    let args = ota_review(&rig, json!({"startup_identity": "abc"}));

    let error = rig.bench.ota(&args).unwrap_err();
    assert!(
        matches!(
            signing(&error),
            wellfed_signing::Error::Frozen(wellfed_signing::FrozenError::BuildId)
        ),
        "{error:?}"
    );
    assert!(rig.tools.0.borrow().installs.is_empty());
}

#[test]
fn ota_refuses_a_directory_that_is_not_a_frozen_review() {
    let mut rig = rig();
    let args = plain_review(&rig);
    let review = rig.bench.root.join(VERIFICATION).join("review");
    let described = fs::read(review.join("review.json")).unwrap();

    fs::remove_file(review.join("review.json")).unwrap();
    let error = rig.bench.ota(&args).unwrap_err();
    assert!(
        matches!(
            signing(&error),
            wellfed_signing::Error::Frozen(wellfed_signing::FrozenError::NoReview { .. })
        ),
        "{error:?}"
    );

    // An image swapped after the freeze.
    fs::write(review.join("review.json"), described).unwrap();
    let other = wellfed_image::fixtures::image_carrying(
        format!("PLFW1 start build={BUILD} bank=1").as_bytes(),
    );
    fs::write(review.join("image.bin"), other).unwrap();
    let error = rig.bench.ota(&args).unwrap_err();
    assert!(
        matches!(
            signing(&error),
            wellfed_signing::Error::Review(wellfed_image::Error::Review(
                wellfed_image::review::ReviewError::ImageMismatch
            ))
        ),
        "{error:?}"
    );

    assert!(rig.tools.0.borrow().installs.is_empty());
    assert!(
        !rig.bench
            .root
            .join(VERIFICATION)
            .join("unit-v1.manifest")
            .exists()
    );
}

#[test]
fn ota_refuses_used_capture_names_before_signing() {
    let mut rig = rig();
    let args = plain_review(&rig);
    fs::create_dir_all(rig.bench.root.join(CAPTURES).join("unit-03")).unwrap();

    let error = rig.bench.ota(&args).unwrap_err();
    assert!(
        matches!(&error, Error::Ota(OtaError::SegmentTaken { file }) if file == "unit-03"),
        "{error:?}"
    );
    assert!(rig.tools.0.borrow().installs.is_empty());
}

#[test]
fn a_failed_offer_stops_without_capturing() {
    let mut rig = rig();
    let args = plain_review(&rig);
    rig.tools.0.borrow_mut().install_refused = true;

    assert_eq!(rig.bench.ota(&args).unwrap(), 1);
    assert_eq!(rig.tools.0.borrow().installs.len(), 1);
    assert!(captured(&rig).is_empty());
}

#[test]
fn a_refused_offer_ends_the_run_before_any_activation() {
    let mut rig = rig();
    let args = plain_review(&rig);
    {
        let mut calls = rig.tools.0.borrow_mut();
        calls
            .install_results
            .push_back(Err(Arc::new(PushError::Device(DeviceError::Install(
                wellfed_client::update::InstallError::Refused(wellfed_model::Refusal::Update(
                    wellfed_model::UpdateRefusal::Downgrade,
                )),
            )))));
        calls.rx.push_back(
            b"PLOTA1 refused reason=downgrade
"
            .to_vec(),
        );
    }

    let error = rig.bench.ota(&args).unwrap_err();
    let Error::Ota(OtaError::PushEnded { push, name, last }) = &error else {
        panic!("{error:?}");
    };
    assert!(
        matches!(**push, PushError::Device(DeviceError::Install(_))),
        "{push:?}"
    );
    assert_eq!((name.as_str(), *last), ("unit", 1));
    assert_eq!(captured(&rig), ["unit-01"]);
}

#[test]
fn ota_passes_across_segments_and_records_steps() {
    let mut rig = rig();
    rig.bench.status = Status::new(&rig.bench.root, rig.bench.root.join("status"));
    rig.bench
        .status
        .begin(BenchCommand::Ota, Some("unit"), steps(BenchCommand::Ota))
        .unwrap();
    let args = plain_review(&rig);
    {
        let mut calls = rig.tools.0.borrow_mut();
        calls.rx.push_back(b"PLWCH1 give_way=1 version=1 restart=1\r\nPLOTA1 manifest version=1 bytes=671744\r\nPLOTA1 progress sec".to_vec());
        calls.rx.push_back(format!("tors=16/164 leases=255\r\nPLOTA1 activated version=1\r\nPLTRL1 confirmed=0 boot=1 limit=3\r\nPLFW1 start build={BUILD} bank=1 reset=0\r\n").into_bytes());
        calls
            .rx
            .push_back(b"[SDK] noise\r\nPLCNF1 confirmed=1 healthy_ms=30000\r\n".to_vec());
    }

    assert_eq!(rig.bench.ota(&args).unwrap(), 0);

    assert_eq!(captured(&rig), ["unit-01", "unit-02", "unit-03"]);
    let state = rig.bench.status.state().unwrap();
    let steps: Vec<_> = state
        .steps
        .iter()
        .map(|s| (s.label.as_str(), s.state))
        .collect();
    assert_eq!(
        steps,
        [
            ("sign", StepState::Done),
            ("offer", StepState::Done),
            ("give way", StepState::Done),
            ("download", StepState::Done),
            ("new build boot", StepState::Done),
            ("confirm", StepState::Done),
            ("health", StepState::Running),
        ]
    );
    assert!(state.lines.iter().any(|l| l == "Written sectors: 16/164"));
}

#[test]
fn ota_reports_a_fallback_with_its_boot_count() {
    let mut rig = rig();
    let args = plain_review(&rig);
    rig.tools.0.borrow_mut().rx.push_back(
        format!("PLOTA1 activated version=1\nPLTRL1 confirmed=0 boot=3 limit=3\nPLFW1 start build={BUILD} bank=1\nPLTRL1 give_way=1 marker=00 reset=1\n").into_bytes(),
    );

    let error = rig.bench.ota(&args).unwrap_err();
    assert!(
        matches!(
            error,
            Error::Ota(OtaError::FellBack {
                boot: Some(3),
                last: 1,
                ..
            })
        ),
        "{error:?}"
    );
    // The stop the operator reads, and the run record keeps.
    assert_eq!(
        error.to_string(),
        "Fell back: KM0 rejected the trial after boot 3; bank 0's recovery role runs. Captures unit-01…01"
    );
    assert_eq!(captured(&rig), ["unit-01"]);
}

#[test]
fn ota_reports_an_update_error_stage() {
    let mut rig = rig();
    let args = plain_review(&rig);
    rig.tools
        .0
        .borrow_mut()
        .rx
        .push_back(b"PLOTA1 error stage=hash\r\n".to_vec());

    let error = rig.bench.ota(&args).unwrap_err();
    assert!(
        matches!(
            error,
            Error::Ota(OtaError::UpdateFailed {
                stage: wellfed_core::ota::install::StopStage::Hash,
                ..
            })
        ),
        "{error:?}"
    );
}

#[test]
fn ota_stops_when_time_runs_out() {
    let mut rig = rig();
    let mut args = plain_review(&rig);
    args.minutes = 2.5;

    let error = rig.bench.ota(&args).unwrap_err();
    assert!(
        matches!(
            error,
            Error::Ota(OtaError::NoOutcome {
                minutes: 2.5,
                last: 3,
                ..
            })
        ),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "No outcome within 2.5 min; the feeder is untouched. Captures unit-01…03"
    );
    assert_eq!(captured(&rig), ["unit-01", "unit-02", "unit-03"]);
}

const OTHER: &str = "5b8433a4f795cb2178c7a38a45087a6727e7a2621fc557d146cff607556f991e";

/// The device API's status of `build` running on `bank`.
fn device_status(build: &str, bank: u8) -> DeviceStatus {
    DeviceStatus {
        build: build.into(),
        bank,
        uptime_ms: 61_000,
        boot_count: 4,
        wifi: wellfed_model::WifiSource::Build,
        losses: 0,
        clock_ms: Some(1_792_674_000_000),
        settings_generation: 2,
        wifi_generation: 1,
        locked: Some(false),
    }
}

/// `ota --build` arguments.
fn build_args() -> OtaArgs {
    OtaArgs {
        name: "unit".into(),
        review: None,
        build: true,
        rebuild: false,
        firmware: Some(Firmware::Trial),
        variant: Some(Variant::Stall),
        config: "private/server.json".into(),
        keys: "private/wellfed-owner.json".into(),
        version: None,
        minutes: 20.0,
        attended_motion: false,
        downgrade: false,
        fault: None,
        cancel: false,
    }
}

fn recorded(rig: &Rig) -> Vec<String> {
    rig.bench.status.state().unwrap().lines.clone()
}

fn with_status(rig: &mut Rig, source: Source) {
    rig.bench.status = Status::new(&rig.bench.root, rig.bench.root.join("status"));
    rig.bench
        .status
        .begin(BenchCommand::Ota, Some("unit"), &ota::steps(source))
        .unwrap();
}

/// A passing run in one segment, with chunk times: the give-way at 12:00:00,
/// activation 200 s later, confirmation 45 s after that.
fn timed_pass(rig: &Rig) {
    let lines = [
        "PLWCH1 give_way=1 version=1 restart=1\r\n".to_string(),
        "PLOTA1 activated version=1\r\n".into(),
        format!("PLFW1 start build={BUILD} bank=1\r\nPLCNF1 confirmed=1 healthy_ms=30000\r\n"),
    ];
    let times = ["12:00:00", "12:03:20", "12:04:05.5"];
    let mut offset = 0;
    let mut chunks = String::new();
    for (line, time) in lines.iter().zip(times) {
        chunks += &format!(
            "{}\n",
            json!({"utc": format!("2026-10-22T{time}+00:00"), "direction": "rx",
                   "offset": offset, "bytes": line.len()})
        );
        offset += line.len();
    }
    let mut calls = rig.tools.0.borrow_mut();
    calls.rx.push_back(lines.concat().into_bytes());
    calls.chunks.push_back(chunks);
}

#[test]
fn ota_build_builds_reviews_offers_and_checks_health_in_order() {
    let mut rig = rig();
    with_status(&mut rig, Source::Build);
    let args = build_args();
    timed_pass(&rig);

    assert_eq!(rig.bench.ota(&args).unwrap(), 0);

    let calls = rig.tools.0.borrow();
    assert_eq!(
        calls.ran,
        [
            "build build --profile firstboot --firmware bringup --variant stall",
            "build review build --build private/firmware-build/plaf103/firstboot --output private/verification/unit-review",
            "install private/verification/unit-v1.manifest private/verification/unit-review/image.bin",
            "status private/wellfed-owner.json",
        ]
    );
    drop(calls);

    let lines = recorded(&rig);
    let has = |text: &str| lines.iter().any(|l| l.contains(text));
    assert!(has("Build took ") && has("Review took "), "{lines:?}");
    assert!(
        has("Status: build 8a13e59efa15 on bank 1, up 61 s, boot 4, Wi-Fi Build, clock valid"),
        "{lines:?}"
    );
    let at = lines.iter().position(|l| l == "Timing:").unwrap();
    let summary: Vec<_> = lines[at + 1..]
        .iter()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    for (row, expected) in summary.iter().zip([
        "Build ",
        "Review ",
        "Sign + offer ",
        "Give-way to activation (download) 3 min 20 s",
        "Activation to confirmation 45.5 s",
        "Health check ",
        "Total ",
    ]) {
        assert!(row.starts_with(expected), "{row:?} vs {expected:?}");
    }
    assert_eq!(summary.len(), 7);
    assert!(rig.board.log().is_empty(), "no relay change");
}

/// A frozen review in private/verification; `complete` adds its review.json.
fn old_review(rig: &Rig, name: &str, identity: &str, complete: bool) {
    let review = rig.bench.root.join(VERIFICATION).join(name);
    let manifest = json!({"startup_identity": identity, "startup_motion_policy": null,
                          "firmware": "feeder"});
    freeze(&rig.bench.root, &review, &manifest);
    if !complete {
        fs::remove_file(review.join("review.json")).unwrap();
    }
}

#[test]
fn ota_build_reuses_a_review_of_the_same_sources() {
    let mut rig = rig();
    with_status(&mut rig, Source::Build);
    let args = build_args();
    old_review(&rig, "earlier-review", BUILD, true);
    rig.tools.0.borrow_mut().source_id = Some(BUILD.into());
    timed_pass(&rig);

    assert_eq!(rig.bench.ota(&args).unwrap(), 0);

    let calls = rig.tools.0.borrow();
    assert_eq!(
        calls.build_queries,
        [[
            "source-id",
            "--image",
            "--firmware",
            "bringup",
            "--variant",
            "stall"
        ]]
    );
    assert!(calls.builds.is_empty(), "nothing built or reviewed");
    assert!(
        calls.ran[0].starts_with(
            "install private/verification/unit-v1.manifest private/verification/earlier-review/image.bin"
        ),
        "{:?}",
        calls.ran
    );
    drop(calls);

    let lines = recorded(&rig);
    let row = |label: &str| {
        lines
            .iter()
            .find(|l| l.trim_start().starts_with(label))
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
    };
    assert_eq!(row("Build").as_deref(), Some("Build reused (same sources)"));
    assert_eq!(
        row("Review").as_deref(),
        Some("Review reused (same sources)")
    );
}

#[test]
fn ota_build_builds_when_no_complete_review_matches_or_asked_to() {
    for (identity, complete, rebuild) in [
        ("ab".repeat(32), true, false),
        (BUILD.to_string(), false, false),
        (BUILD.to_string(), true, true),
    ] {
        let mut rig = rig();
        with_status(&mut rig, Source::Build);
        let mut args = build_args();
        args.rebuild = rebuild;
        old_review(&rig, "earlier-review", &identity, complete);
        rig.tools.0.borrow_mut().source_id = Some(BUILD.into());
        timed_pass(&rig);

        assert_eq!(rig.bench.ota(&args).unwrap(), 0);

        let calls = rig.tools.0.borrow();
        assert!(
            calls.ran[0].starts_with("build build --profile firstboot"),
            "{identity} {complete} {rebuild}: {:?}",
            calls.ran
        );
        assert_eq!(calls.build_queries.is_empty(), rebuild);
    }
}

#[test]
fn a_failed_build_or_review_stops_before_offering() {
    for codes in [&[1][..], &[0, 2]] {
        let mut rig = rig();
        with_status(&mut rig, Source::Build);
        let args = build_args();
        rig.tools.0.borrow_mut().codes.extend(codes);

        assert_eq!(rig.bench.ota(&args).unwrap(), 1, "{codes:?}");

        let calls = rig.tools.0.borrow();
        assert_eq!(calls.builds.len(), codes.len());
        assert!(calls.queries.is_empty());
        drop(calls);
        assert!(captured(&rig).is_empty());
        let lines = recorded(&rig);
        assert!(lines.iter().any(|l| l.starts_with("Build took ")));
        assert!(lines.iter().any(|l| l == "Timing:"), "{lines:?}");
    }
}

#[test]
fn ota_build_refuses_an_existing_review_up_front() {
    let mut rig = rig();
    let args = build_args();
    fs::create_dir_all(rig.bench.root.join(VERIFICATION).join("unit-review")).unwrap();

    let error = rig.bench.ota(&args).unwrap_err();
    assert!(
        matches!(&error, Error::Command(CommandsError::NameTaken { path }) if path == "private/verification/unit-review"),
        "{error:?}"
    );
    assert!(rig.tools.0.borrow().ran.is_empty());
}

#[test]
fn build_options_without_build_are_refused() {
    let mut rig = rig();
    let mut args = plain_review(&rig);
    args.variant = Some(Variant::Fail);

    let error = rig.bench.ota(&args).unwrap_err();
    assert!(
        matches!(error, Error::Ota(OtaError::BuildOnly)),
        "{error:?}"
    );
    assert!(rig.tools.0.borrow().ran.is_empty());
}

#[test]
fn the_motor_firmware_and_its_variant_are_refused() {
    assert_eq!(ota::parse_variant("pulse"), Err(ArgError::PulseVariant));
    assert_eq!(ota::parse_firmware("motor"), Err(ArgError::MotorFirmware));

    assert_eq!(ota::parse_variant("fail"), Ok(Variant::Fail));
    assert_eq!(ota::parse_firmware("adc"), Ok(Firmware::Adc));
    assert_eq!(Firmware::Speaker.name(), "speaker");
    assert_eq!(
        ota::parse_variant("fw14-adc"),
        Err(ArgError::UnknownVariant {
            text: "fw14-adc".into()
        })
    );
    // clap shows the refusal with every name it takes.
    assert_eq!(
        ota::parse_firmware("reference").unwrap_err().to_string(),
        "unknown firmware \"reference\"; use qa, optics, buttons, adc, speaker, blink, trial, feeder"
    );
}

/// The bench's firmware names predate the bring-up firmware's variants, so
/// each still names the image it always did.
#[test]
fn bench_names_resolve_to_the_builders_firmware_and_variant() {
    for (image, expected) in [
        (Image::of(Firmware::Qa), &["--firmware", "qa"][..]),
        (
            Image::of(Firmware::Optics),
            &["--firmware", "bringup", "--variant", "optics"],
        ),
        (
            Image::of(Firmware::Buttons),
            &["--firmware", "bringup", "--variant", "buttons"],
        ),
        (
            Image::of(Firmware::Adc),
            &["--firmware", "bringup", "--variant", "adc"],
        ),
        (
            Image::of(Firmware::Speaker),
            &["--firmware", "bringup", "--variant", "speaker"],
        ),
        (Image::of(Firmware::Blink), &["--firmware", "blink"]),
        (Image::of(Firmware::Trial), &["--firmware", "bringup"]),
        (
            Image::trial(Variant::Stall),
            &["--firmware", "bringup", "--variant", "stall"],
        ),
        (
            Image::trial(Variant::Fail),
            &["--firmware", "bringup", "--variant", "fail"],
        ),
        (Image::of(Firmware::Feeder), &["--firmware", "feeder"]),
        (
            Image {
                firmware: Firmware::Feeder,
                variant: Some(Variant::Twin),
            },
            &["--firmware", "feeder", "--variant", "twin"],
        ),
    ] {
        assert_eq!(image.args(), expected, "{image}");
    }
}

/// The bring-up variants the bench names are ones the bring-up firmware
/// declares.
#[test]
fn bench_bringup_variants_are_declared() {
    let manifest = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../firmware/wellfed/bringup/Cargo.toml"),
    )
    .unwrap();
    let images = [
        Image::of(Firmware::Optics),
        Image::of(Firmware::Buttons),
        Image::of(Firmware::Adc),
        Image::of(Firmware::Speaker),
        Image::of(Firmware::Trial),
        Image::trial(Variant::Stall),
        Image::trial(Variant::Fail),
    ];
    for image in images {
        let args = image.args();
        assert_eq!(args[..2], ["--firmware", "bringup"], "{image}");
        if let Some(variant) = args.get(3) {
            let table = format!("[package.metadata.wellfed.variants.{variant}]");
            assert!(manifest.contains(&table), "{image}: {table}");
        }
    }
}

#[test]
fn a_variant_needs_the_firmware_that_declares_it() {
    for (firmware, variant) in [
        (Firmware::Speaker, None),
        (Firmware::Trial, Some(Variant::Entropy)),
        (Firmware::Feeder, Some(Variant::Fail)),
    ] {
        let mut rig = rig();
        let mut args = build_args();
        args.firmware = Some(firmware);
        if let Some(variant) = variant {
            args.variant = Some(variant);
        }

        let error = rig.bench.ota(&args).unwrap_err();
        assert!(
            matches!(error, Error::Ota(OtaError::VariantFirmware)),
            "{error:?}"
        );
        assert!(rig.tools.0.borrow().ran.is_empty());
    }
    assert!(
        ota::parse_variant("setup").is_err(),
        "the setup variant was retired"
    );
    assert_eq!(ota::parse_variant("entropy"), Ok(Variant::Entropy));
    assert_eq!(Variant::Entropy.firmware(), Firmware::Feeder);
    assert_eq!(ota::parse_variant("owner-stall"), Ok(Variant::OwnerStall));
    assert_eq!(ota::parse_variant("held-stall"), Ok(Variant::HeldStall));
    assert_eq!(Variant::HeldStall.firmware(), Firmware::Feeder);
    assert_eq!(Variant::OwnerStall.firmware(), Firmware::Feeder);
    assert_eq!(ota::parse_variant("gap-probe"), Ok(Variant::GapProbe));
    assert_eq!(Variant::GapProbe.firmware(), Firmware::Feeder);
    for (name, variant) in [
        ("feed-stall", Variant::FeedStall),
        ("stop-fail", Variant::StopFail),
        ("owner-fail", Variant::OwnerFail),
        ("feed-jam", Variant::FeedJam),
        ("sensor-fault", Variant::SensorFault),
    ] {
        assert_eq!(ota::parse_variant(name), Ok(variant));
        assert_eq!(
            (variant.name(), variant.firmware()),
            (name, Firmware::Feeder)
        );
    }
}

#[test]
fn a_status_that_never_shows_the_build_fails_after_confirmation() {
    let mut rig = rig();
    with_status(&mut rig, Source::Review);
    let args = plain_review(&rig);
    timed_pass(&rig);
    rig.tools.0.borrow_mut().status_default = Some(device_status(OTHER, 0));

    let error = rig.bench.ota(&args).unwrap_err();
    let Error::Ota(OtaError::Unhealthy { last }) = &error else {
        panic!("{error:?}");
    };
    assert!(
        matches!(&**last, Error::NotReady(crate::server::NotReady::Running(build)) if build == "5b8433a4f795"),
        "{last:?}"
    );
    assert_eq!(
        error.to_string(),
        "Confirmed on UART but the device API did not report the build within 90 s \
         (the feeder reports build 5b8433a4f795)"
    );

    // Every 5 s from 0 to 90 s, and the unchanged reason is shown once.
    assert_eq!(rig.tools.0.borrow().queries.len(), 19);
    let lines = recorded(&rig);
    let waits = lines
        .iter()
        .filter(|l| l.starts_with("Waiting for status"))
        .count();
    assert_eq!(waits, 1);
    let health = lines
        .iter()
        .find(|l| l.trim_start().starts_with("Health check"))
        .unwrap();
    assert!(!health.ends_with('-'), "{health}");
}

/// A bring-up firmware serves no device API: its UART confirmation
/// is the whole check, and no status is asked for.
#[test]
fn a_firmware_without_the_device_api_passes_on_its_confirmation() {
    let mut rig = rig();
    with_status(&mut rig, Source::Review);
    let args = ota_review(
        &rig,
        json!({"startup_identity": BUILD, "startup_motion_policy": null, "firmware": "qa"}),
    );
    timed_pass(&rig);
    rig.tools.0.borrow_mut().status_default = Some(device_status(OTHER, 0));

    assert_eq!(rig.bench.ota(&args).unwrap(), 0);
    assert!(rig.tools.0.borrow().queries.is_empty());
    assert!(
        recorded(&rig)
            .iter()
            .any(|l| l.contains("qa serves no device API"))
    );
}

#[test]
fn a_missing_or_wrong_status_is_waited_out_until_it_matches() {
    let mut rig = rig();
    with_status(&mut rig, Source::Review);
    let args = plain_review(&rig);
    timed_pass(&rig);
    rig.tools.0.borrow_mut().statuses.extend([
        Err(DeviceError::Timeout(Duration::from_secs(10))),
        Ok(device_status(BUILD, 0)),
        Ok(device_status(BUILD, 1)),
    ]);

    assert_eq!(rig.bench.ota(&args).unwrap(), 0);

    assert_eq!(rig.tools.0.borrow().queries.len(), 3);
    let lines = recorded(&rig);
    let has = |text: &str| lines.iter().any(|l| l.contains(text));
    assert!(has("the feeder did not answer within 10 s"), "{lines:?}");
    assert!(has("the feeder runs from bank 0"), "{lines:?}");
    // A review run has nothing to report for the build.
    assert!(has("skipped (--review)"), "{lines:?}");
}
