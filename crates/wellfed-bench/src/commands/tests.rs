//! No device I/O: loader sequencing, refusals and update stop rules.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use super::*;
use crate::relay::FakeBoard;
use crate::status::FlashTool;

const PLAN: &str = r#"{"operation_plan": {"bank": 1, "ordered_sectors": [{}], "erase_sectors": 1,
    "target_ota_sha256": "t", "fallback_ota_sha256": "f"}}"#;

fn loader_result() -> Value {
    json!({"download_banner_seen": true, "longest_nak_run": 99, "nak_bytes": 99,
           "fault_marker_seen": false, "stock_startup_markers": 0, "bytes": 120})
}

#[derive(Default)]
struct Calls {
    observed: Vec<PathBuf>,
    flash: Vec<Vec<String>>,
    flash_codes: VecDeque<i32>,
    results: VecDeque<Value>,
    analyzer_code: i32,
    sigrok: bool,
}

#[derive(Clone, Default)]
struct FakeTools(Rc<RefCell<Calls>>);

struct FakeObserver(Value);

impl Observer for FakeObserver {
    fn finish(&mut self) -> Result<Value> {
        Ok(self.0.clone())
    }
    fn kill(&mut self) {}
}

struct FakeAnalyzer {
    output: PathBuf,
    code: i32,
}

impl Analyzer for FakeAnalyzer {
    fn running(&mut self) -> bool {
        true
    }
    fn wait(&mut self, _: Duration) -> Result<Option<i32>> {
        // Recorded while the feeder was being powered.
        fs::write(&self.output, [0u8; 10])?;
        Ok(Some(self.code))
    }
    fn kill(&mut self) {}
    fn stderr(&mut self) -> String {
        String::new()
    }
}

impl Tools for FakeTools {
    fn observe(&mut self, _: &str, output: &Path, _: f64) -> Result<Box<dyn Observer>> {
        let mut calls = self.0.borrow_mut();
        calls.observed.push(output.to_path_buf());
        let result = calls.results.pop_front().unwrap_or_else(loader_result);
        Ok(Box::new(FakeObserver(result)))
    }

    fn flash(&mut self, args: &[String], _: &mut Status) -> Result<i32> {
        let mut calls = self.0.borrow_mut();
        calls.flash.push(args.to_vec());
        let code = calls.flash_codes.pop_front().unwrap_or(0);
        // A successful prepare writes its plan to --output.
        if code == 0 && args[1] == "prepare" {
            let at = args.iter().position(|a| a == "--output").unwrap();
            fs::write(&args[at + 1], PLAN)?;
        }
        Ok(code)
    }

    fn analyzer(&mut self, output: &Path, _: f64, _: u64) -> Result<Box<dyn Analyzer>> {
        Ok(Box::new(FakeAnalyzer {
            output: output.to_path_buf(),
            code: self.0.borrow().analyzer_code,
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
    let board = FakeBoard::default();
    let tools = FakeTools::default();
    tools.0.borrow_mut().sigrok = true;

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
        sleep: |_| Ok(()),
        off_seconds: 2.0,
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
        [(2, false), (1, false), (2, true), (1, true), (2, false)]
    );
    assert_eq!(rig.board.state(), [true, false, false, false]);
}

#[test]
fn unconfirmed_loader_powers_off() {
    for (field, value) in [
        ("download_banner_seen", json!(false)),
        ("longest_nak_run", json!(3)),
        ("fault_marker_seen", json!(true)),
        ("stock_startup_markers", json!(1)),
    ] {
        let mut rig = rig();
        let mut result = loader_result();
        result[field] = value;
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

    let error = rig.bench.boot(&existing, 10.0).unwrap_err();
    assert!(error.to_string().contains("Refusing to reuse"));
    let error = rig.bench.loader(&existing).unwrap_err();
    assert!(error.to_string().contains("Refusing to reuse"));
    assert!(rig.bench.capture("used", 5.0).is_err());

    assert!(rig.board.log().is_empty());
}

#[test]
fn boot_refuses_loader_output() {
    let mut rig = rig();
    let out = rig.bench.root.join("out");

    let error = rig.bench.boot(&out, 10.0).unwrap_err();
    assert!(error.to_string().contains("ROM loader"));
}

#[test]
fn execute_requires_uninterrupted_power() {
    let mut rig = rig();
    prepare_plan(&rig, "unit");

    assert!(rig.bench.flash_execute(&execute("unit")).is_err());
    assert!(rig.tools.0.borrow().flash.is_empty());
}

#[test]
fn execute_requires_a_prepared_single_use_plan() {
    let mut rig = rig();
    rig.bench.on().unwrap();

    let error = rig.bench.flash_execute(&execute("unit")).unwrap_err();
    assert!(error.to_string().contains("Prepared plan required"));

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
    rig.tools
        .0
        .borrow_mut()
        .results
        .push_back(json!({"download_banner_seen": false, "bytes": 10, "fault_marker_seen": false}));

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
        .begin("flash-prepare", Some("unit"), &[])
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

    let sha = sha256_hex(PLAN.as_bytes());
    assert_eq!(
        rig.bench.status.state.as_ref().unwrap().next.as_deref(),
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

    rig.tools
        .0
        .borrow_mut()
        .results
        .push_back(json!({"download_banner_seen": false, "bytes": 10}));
    let execute = GiveWayExecuteArgs {
        give_way: args,
        expected_plan_sha256: sha256_hex(PLAN.as_bytes()),
        boot_seconds: 10.0,
    };
    assert_eq!(rig.bench.give_way_execute(&execute).unwrap(), 0);

    let calls = rig.tools.0.borrow();
    let last = calls.flash.last().unwrap();
    assert_eq!(last[..2], ["give-way", "execute"]);
    assert!(last.contains(&"--fallback".to_string()));
}

#[test]
fn bank_1_needs_a_source_before_any_relay_change() {
    let mut rig = rig();
    let mut args = update("unit");
    args.source = None;

    let error = rig.bench.flash_prepare(&args).unwrap_err();
    assert!(error.to_string().contains("--source is required"));
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
    assert!(error.to_string().contains("attended-motion"));
    args.attended_motion = true;
    rig.bench.require_attended_motion(&args).unwrap();

    args.attended_motion = false;
    fs::write(
        target.join("build-manifest.json"),
        r#"{"startup_motion_policy": null}"#,
    )
    .unwrap();
    rig.bench.require_attended_motion(&args).unwrap();
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

    // Each cycle: strap released and power off, then power on, then off.
    assert_eq!(
        rig.board.log()[..4],
        [(2, false), (1, false), (1, true), (2, false)]
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
fn loader_cycles_hold_the_strap_across_power_on() {
    let mut rig = rig();
    rig.bench
        .transients(&transients(TransientMode::Loader))
        .unwrap();

    assert_eq!(
        rig.board.log()[..5],
        [(2, false), (1, false), (2, true), (1, true), (2, false)]
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

    assert!(error.to_string().contains("sigrok-cli"));
    assert!(rig.board.log().is_empty());
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
fn demo_switches_only_spares_and_ends_them_off() {
    let mut rig = rig();
    rig.bench.on().unwrap();
    let before = rig.board.log().len();
    let args = DemoArgs {
        patterns: vec![],
        speed: 1.0,
        text: "SOS".into(),
    };

    assert_eq!(rig.bench.demo(&args).unwrap(), 0);

    let log = rig.board.log();
    assert!(
        log[before..]
            .iter()
            .all(|(channel, _)| SPARES.contains(channel))
    );
    assert_eq!(rig.board.state(), [true, false, false, false]);
}

#[test]
fn demo_stops_at_the_toggle_cap_and_still_ends_off() {
    let mut rig = rig();
    let args = DemoArgs {
        patterns: vec![Pattern::Morse],
        speed: 1.0,
        // Each E is two toggles: more than the cap allows.
        text: "E".repeat(70),
    };

    let error = rig.bench.demo(&args).unwrap_err();
    assert!(error.to_string().contains("Toggle cap"));
    assert_eq!(rig.board.state()[2..], [false, false]);
}

#[test]
fn update_traffic_matches_observed_installs() {
    // Totals observed on October 22, 2026 (FW32 bench): a 158-sector bank-0
    // install and a no-op, both over a 0xa4000-byte region.
    let plan = |sectors: usize, packets: u64| {
        json!({"staging_packets": packets, "commit_packets": packets.min(1),
               "region": {"bytes": 0xa4000}, "interrupted": false,
               "ordered_sectors": vec![json!({}); sectors]})
    };
    let close = |estimate: u64, actual: u64| estimate.abs_diff(actual) * 100 <= actual;

    let (tx, rx) = update_traffic(&plan(158, 632)).unwrap();
    assert!(close(tx, 667_579) && close(rx, 4_081_733), "{tx} {rx}");

    let (tx, rx) = update_traffic(&plan(0, 0)).unwrap();
    assert!(tx == 0 && close(rx, 703_173), "{rx}");
}
