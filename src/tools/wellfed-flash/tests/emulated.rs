//! The `wellfed-flash` binary against the pinned vendor helper, executed
//! under unicorn on synthetic NOR (`wellfed-emu`). No device opens.
//!
//! Each test serves one emulated helper on a Unix socket named by
//! `WELLFED_FLASH_EMULATOR` and runs the CLI against it, so plans, journals
//! and reports are the real ones. The helper's command parser and packet
//! checks run as ARM; ROM UART and flash calls are modeled, so this
//! establishes no physical write acceptance, timing or power-cut behavior.
//!
//! Needs the ignored baseline, frozen development reviews, session reports
//! and the pinned helper (`docs/development/fixtures.md`).
//! `WELLFED_FIXTURE_ROOT` names another checkout whose `private/` holds them;
//! outputs go to temporary directories under that `private/`.

use std::fs;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use wellfed_emu::helper::{Call, Helper, Scope};
use wellfed_emu::{HELPER, serve};
use wellfed_flash::journal;
use wellfed_flash::ota::container;
use wellfed_formats::canonical::sha256;
use serde_json::{Value, json};
use tempfile::TempDir;

const BINARY: &str = env!("CARGO_BIN_EXE_wellfed-flash");
const BANK1: usize = 0x18_6000;
const BANK1_END: usize = 0x2D_6000;
const SECTOR: usize = 0x1000;
const BASELINE: &str = "private/fixtures/baseline.bin";
const SOURCE: &str = "private/fixtures/development-source";
const TARGET: &str = "private/fixtures/development-target";
const SMALL: &str = "private/fixtures/development-bounded-read/report.json";
const ROM: &str = "private/fixtures/rom-inspection/report.json";
/// Healthy status registers: no protection, QE set.
const STATUS: [u8; 3] = [0, 2, 0];

fn root() -> PathBuf {
    std::env::var_os("WELLFED_FIXTURE_ROOT").map_or_else(
        || PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../..")),
        PathBuf::from,
    )
}

struct Fixture {
    root: PathBuf,
    baseline: Vec<u8>,
    helper: Vec<u8>,
    port: String,
    /// Temporary private outputs; dropped (removed) with the fixture.
    scratch: TempDir,
    rom: PathBuf,
    small: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = root().canonicalize().unwrap();
        let read = |name: &str| fs::read(root.join(name)).unwrap();
        let small: Value = serde_json::from_slice(&read(SMALL)).unwrap();
        let scratch = tempfile::tempdir_in(root.join("private")).unwrap();
        let rom = emulated(&root.join(ROM), scratch.path());
        let small_copy = emulated(&root.join(SMALL), scratch.path());
        Self {
            baseline: read(BASELINE),
            helper: read(HELPER),
            port: small["port"].as_str().unwrap().to_string(),
            scratch,
            rom,
            small: small_copy,
            root,
        }
    }

    fn payload(&self, review: &str) -> Vec<u8> {
        let image = fs::read(self.root.join(review).join("image.bin")).unwrap();
        container(&image).unwrap().to_vec()
    }

    fn out(&self, name: &str) -> PathBuf {
        self.scratch.path().join(name)
    }

    /// The session evidence every device command continues.
    fn evidence(&self, output: &Path) -> Vec<String> {
        [
            "--port",
            &self.port,
            "--baseline",
            BASELINE,
            "--rom-report",
            &self.rom.display().to_string(),
            "--small-report",
            &self.small.display().to_string(),
            "--output",
            &output.display().to_string(),
        ]
        .map(String::from)
        .to_vec()
    }

    /// Run the CLI against `helper`, serving it until the process exits.
    fn run(&self, helper: &mut Helper<'_>, args: &[String]) -> Run {
        let socket = self.scratch.path().join("helper.sock");
        let _ = fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        let (stdout, stderr) = (self.out("stdout"), self.out("stderr"));
        let mut child = Command::new(BINARY)
            .arg("--root")
            .arg(&self.root)
            .args(args)
            .current_dir(&self.root)
            .env("WELLFED_FLASH_EMULATOR", &socket)
            .stdout(fs::File::create(&stdout).unwrap())
            .stderr(fs::File::create(&stderr).unwrap())
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        let (status, error) = serve::until_exit(&listener, helper, &mut child).unwrap();
        let stdout = fs::read_to_string(stdout).unwrap();
        Run {
            code: status.code().unwrap(),
            summary: stdout
                .lines()
                .last()
                .map_or(Value::Null, |line| serde_json::from_str(line).unwrap()),
            stderr: fs::read_to_string(stderr).unwrap(),
            helper_error: error.map(|e| e.to_string()),
        }
    }
}

struct Run {
    code: i32,
    summary: Value,
    stderr: String,
    helper_error: Option<String>,
}

impl Run {
    fn expect(self, code: i32, status: &str) -> Value {
        assert_eq!(
            (self.code, self.summary["status"].as_str()),
            (code, Some(status)),
            "{}\n{:?}",
            self.stderr,
            self.helper_error
        );
        self.summary
    }
}

/// A copy of a real session report marked as emulator evidence, which is
/// all an emulated session may continue. Companion files are copied too.
fn emulated(report: &Path, scratch: &Path) -> PathBuf {
    let source = report.parent().unwrap();
    let directory = scratch.join(source.file_name().unwrap());
    fs::create_dir_all(&directory).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            fs::copy(&path, directory.join(path.file_name().unwrap())).unwrap();
        }
    }
    let mut value: Value = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
    value["transport"] = json!("emulator");
    let copy = directory.join(report.file_name().unwrap());
    fs::write(&copy, value.to_string()).unwrap();
    copy
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_string()).collect()
}

fn path(path: &Path) -> String {
    path.display().to_string()
}

fn bank1_sectors(length: usize) -> Vec<u32> {
    (BANK1..BANK1 + length)
        .step_by(SECTOR)
        .map(|s| u32::try_from(s).unwrap())
        .collect()
}

/// The research unit with the reviewed source in bank 1 and one byte
/// outside both banks changed, so preservation is checked against it.
fn unit(f: &Fixture) -> (Vec<u8>, Vec<u8>, Vec<u8>, usize) {
    let (source, target) = (f.payload(SOURCE), f.payload(TARGET));
    let mut flash = f.baseline.clone();
    flash[BANK1..BANK1 + source.len()].copy_from_slice(&source);
    flash[0x65_0000] ^= 1;
    let length = source.len().max(target.len());
    (flash, source, target, length)
}

fn helper<'a>(f: &Fixture, flash: &[u8], sectors: Vec<u32>) -> Helper<'a> {
    let scope = Scope {
        bank: 1,
        sectors,
        status: Some(STATUS),
    };
    Helper::new(&f.helper, flash, scope).unwrap()
}

fn erases(helper: &Helper<'_>) -> Vec<u32> {
    helper
        .calls()
        .iter()
        .filter_map(|call| match call {
            Call::Erase { offset } => Some(*offset),
            _ => None,
        })
        .collect()
}

fn receipt(directory: &Path) -> journal::Receipt {
    journal::inspect(&fs::read(directory.join("transaction/journal.jsonl")).unwrap()).unwrap()
}

#[test]
#[ignore = "needs the private baseline, frozen reviews, session reports and pinned helper"]
fn serial_evidence_never_continues_an_emulated_session() {
    let f = Fixture::new();
    let (flash, _, _, length) = unit(&f);
    let mut helper = helper(&f, &flash, bank1_sectors(length));

    let mut args = strings(&["update", "inspect"]);
    args.extend(strings(&[
        "--port",
        &f.port,
        "--baseline",
        BASELINE,
        "--rom-report",
        ROM,
        "--small-report",
        SMALL,
        "--source",
        SOURCE,
        "--target",
        TARGET,
    ]));
    args.extend(["--output".into(), path(&f.out("inspect"))]);
    let summary = f.run(&mut helper, &args).expect(2, "refused");

    assert!(
        summary["error"]
            .as_str()
            .unwrap()
            .contains("different transport")
    );
    assert!(helper.host_tx().is_empty());
}

#[test]
#[ignore = "needs the private baseline, frozen reviews, session reports and pinned helper"]
fn update_plans_writes_and_journals_through_the_cli() {
    let f = Fixture::new();
    let (before, source, target, length) = unit(&f);
    let mut helper = helper(&f, &before, bank1_sectors(length));
    let images = strings(&["--source", SOURCE, "--target", TARGET]);
    let inspect = f.out("inspect");
    let plan = f.out("plan.json");

    let mut args = strings(&["update", "inspect"]);
    args.extend(f.evidence(&inspect));
    args.extend(images.clone());
    f.run(&mut helper, &args).expect(0, "complete");
    let report: Value =
        serde_json::from_slice(&fs::read(inspect.join("report.json")).unwrap()).unwrap();
    assert_eq!(report["transport"], "emulator");
    assert_eq!(report["status_checks"][0]["event"], "flash_status_checked");

    let mut args = strings(&["update", "prepare"]);
    args.extend(f.evidence(&plan));
    args.extend(images.clone());
    args.extend(["--inspection".into(), path(&inspect.join("report.json"))]);
    let prepared = f.run(&mut helper, &args).expect(0, "prepared");
    let reviewed: Value = serde_json::from_slice(&fs::read(&plan).unwrap()).unwrap();
    let ordered: Vec<u64> = reviewed["operation_plan"]["ordered_sectors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["offset"].as_u64().unwrap())
        .collect();
    assert!(ordered.len() > 1);
    assert_eq!(sha256(&fs::read(&plan).unwrap()), prepared["plan_sha256"]);

    helper.clear_host_tx();
    let execute = f.out("execute");
    let mut args = strings(&["update", "execute"]);
    args.extend(f.evidence(&execute));
    args.extend(images.clone());
    args.extend([
        "--inspection".into(),
        path(&inspect.join("report.json")),
        "--plan".into(),
        path(&plan),
        "--expected-plan-sha256".into(),
        prepared["plan_sha256"].as_str().unwrap().into(),
    ]);
    let run = f.run(&mut helper, &args);
    assert!(run.helper_error.is_none(), "{:?}", run.helper_error);
    run.expect(0, "verified");

    assert_eq!(
        helper.host_tx(),
        fs::read(execute.join("serial-tx.bin")).unwrap()
    );
    let erased: Vec<u64> = erases(&helper).into_iter().map(u64::from).collect();
    assert_eq!(erased, ordered);
    assert_ne!(source, target);
    let after = helper.snapshot().unwrap();
    assert!(after[..BANK1] == before[..BANK1]);
    assert!(after[BANK1 + length..] == before[BANK1 + length..]);
    assert!(after[BANK1..BANK1 + target.len()] == target[..]);
    let receipt = receipt(&execute);
    assert!(receipt.verified_terminal);
    assert_eq!(
        receipt.erase_order,
        ordered.iter().map(|o| json!(o)).collect::<Vec<_>>()
    );

    // A second execution with a wrong hash is refused before serial access.
    let again = f.out("again");
    let mut args = strings(&["update", "execute"]);
    args.extend(f.evidence(&again));
    args.extend(images);
    args.extend([
        "--inspection".into(),
        path(&inspect.join("report.json")),
        "--plan".into(),
        path(&plan),
        "--expected-plan-sha256".into(),
        "0".repeat(64),
    ]);
    f.run(&mut helper, &args).expect(2, "refused");
    assert!(!again.exists());
}

#[test]
#[ignore = "needs the private baseline, frozen reviews, session reports and pinned helper"]
fn give_way_then_restore_marker_through_the_cli() {
    let f = Fixture::new();
    let (mut flash, _, _, length) = unit(&f);
    // Bank 0 holds exactly the reviewed fallback; bank 1 the reviewed source.
    let fallback = f.payload(TARGET);
    flash[0x6000..0x6000 + fallback.len()].copy_from_slice(&fallback);
    let mut helper = helper(&f, &flash, bank1_sectors(length));
    let give = strings(&["--fallback", TARGET]);
    let inspect = f.out("inspect");
    let plan = f.out("plan.json");

    let mut args = strings(&["give-way", "inspect"]);
    args.extend(f.evidence(&inspect));
    args.extend(give.clone());
    f.run(&mut helper, &args).expect(0, "complete");
    let mut args = strings(&["give-way", "prepare"]);
    args.extend(f.evidence(&plan));
    args.extend(give.clone());
    args.extend(["--inspection".into(), path(&inspect.join("report.json"))]);
    let prepared = f.run(&mut helper, &args).expect(0, "prepared");
    let mut args = strings(&["give-way", "execute"]);
    args.extend(f.evidence(&f.out("execute")));
    args.extend(give);
    args.extend([
        "--inspection".into(),
        path(&inspect.join("report.json")),
        "--plan".into(),
        path(&plan),
        "--expected-plan-sha256".into(),
        prepared["plan_sha256"].as_str().unwrap().into(),
    ]);
    f.run(&mut helper, &args).expect(0, "verified");

    let after = helper.snapshot().unwrap();
    assert_eq!(after[BANK1], 0);
    assert!(after[BANK1 + 1..] == flash[BANK1 + 1..]);
    assert!(after[..BANK1] == flash[..BANK1]);
    assert!(erases(&helper).is_empty(), "a give-way never erases");

    // Restoring bank 1 is a development update of the source onto itself.
    let images = strings(&["--source", SOURCE, "--target", SOURCE, "--restore-marker"]);
    let inspect = f.out("restore-inspect");
    let plan = f.out("restore.json");
    let mut args = strings(&["update", "inspect"]);
    args.extend(f.evidence(&inspect));
    args.extend(images.clone());
    f.run(&mut helper, &args).expect(0, "complete");
    let mut args = strings(&["update", "prepare"]);
    args.extend(f.evidence(&plan));
    args.extend(images.clone());
    args.extend(["--inspection".into(), path(&inspect.join("report.json"))]);
    let prepared = f.run(&mut helper, &args).expect(0, "prepared");
    let mut args = strings(&["update", "execute"]);
    args.extend(f.evidence(&f.out("restore")));
    args.extend(images);
    args.extend([
        "--inspection".into(),
        path(&inspect.join("report.json")),
        "--plan".into(),
        path(&plan),
        "--expected-plan-sha256".into(),
        prepared["plan_sha256"].as_str().unwrap().into(),
    ]);
    f.run(&mut helper, &args).expect(0, "verified");
    assert!(helper.snapshot().unwrap() == flash);
}

#[test]
#[ignore = "needs the private baseline, frozen reviews, session reports and pinned helper"]
fn status_read_continues_the_session_with_whitelisted_reads_only() {
    let f = Fixture::new();
    let (flash, _, _, length) = unit(&f);
    let mut helper = helper(&f, &flash, bank1_sectors(length));
    let output = f.out("status");

    let args = strings(&[
        "read",
        "status",
        "--port",
        &f.port,
        "--rom-report",
        &path(&f.rom),
        "--small-report",
        &path(&f.small),
        "--helper",
        HELPER,
        "--output",
        &path(&output),
    ]);
    f.run(&mut helper, &args).expect(0, "complete");

    let report: Value =
        serde_json::from_slice(&fs::read(output.join("report.json")).unwrap()).unwrap();
    assert_eq!(report["status_observations"], json!([STATUS, STATUS]));
    assert_eq!(
        report["result"]["bank1_assessment"]["software_preconditions_met"],
        true
    );
    assert_eq!(report["helper_upload_attempted"], false);
    let expected: Vec<u8> = (0..2)
        .flat_map(|_| [5, 0x35, 0x15])
        .flat_map(|op| [0x21, op, 1])
        .collect();
    assert_eq!(fs::read(output.join("serial-tx.bin")).unwrap(), expected);
}

/// A full read of the helper's flash through the CLI, and a reviewed stock
/// recovery plan for it: `(read, plan, plan SHA-256)`.
fn recovery(f: &Fixture, helper: &mut Helper<'_>) -> (PathBuf, PathBuf, String) {
    let read = f.out("full");
    let mut args = strings(&["read", "full", "--port", &f.port]);
    args.extend(strings(&[
        "--rom-report",
        &path(&f.rom),
        "--small-report",
        &path(&f.small),
    ]));
    args.extend(["--output".into(), path(&read)]);
    f.run(helper, &args).expect(0, "complete");
    let current = read.join("flash-read.bin");

    let plan = f.out("recovery.json");
    let mut args = strings(&["stock-recovery", "prepare"]);
    args.extend(f.evidence(&plan));
    args.extend(["--current".into(), path(&current)]);
    let prepared = f.run(helper, &args).expect(0, "prepared");
    assert!(prepared["changed_sectors"].as_u64().unwrap() > 1);
    let sha = prepared["plan_sha256"].as_str().unwrap().to_string();
    (current, plan, sha)
}

/// The research unit with bank 1 unbootable: ordinary recovery refuses a
/// still-bootable bank.
fn unbootable(f: &Fixture) -> Vec<u8> {
    let (mut flash, _, _, _) = unit(f);
    flash[BANK1] = 0;
    flash
}

fn execute_recovery(
    current: &Path,
    plan: &Path,
    sha: &str,
    output: &Path,
    f: &Fixture,
) -> Vec<String> {
    let mut args = strings(&["stock-recovery", "execute"]);
    args.extend(f.evidence(output));
    args.extend([
        "--current".into(),
        path(current),
        "--plan".into(),
        path(plan),
        "--expected-plan-sha256".into(),
        sha.into(),
    ]);
    args
}

/// The original bank 1 restored over a changed, unbootable bank: a full
/// read, then a reviewed plan, then its execution, all through the CLI.
#[test]
#[ignore = "needs the private baseline, frozen reviews, session reports and pinned helper"]
fn stock_recovery_restores_bank1_and_preserves_everything_else() {
    let f = Fixture::new();
    let flash = unbootable(&f);
    let mut helper = helper(&f, &flash, bank1_sectors(BANK1_END - BANK1));
    let (current, plan, sha) = recovery(&f, &mut helper);
    assert!(fs::read(&current).unwrap() == flash);

    helper.clear_host_tx();
    let execute = f.out("recovery");
    let run = f.run(
        &mut helper,
        &execute_recovery(&current, &plan, &sha, &execute, &f),
    );
    assert!(run.helper_error.is_none(), "{:?}", run.helper_error);
    run.expect(0, "verified");

    let mut want = flash.clone();
    want[BANK1..BANK1_END].copy_from_slice(&f.baseline[BANK1..BANK1_END]);
    assert!(
        helper.snapshot().unwrap() == want,
        "bank 1 is stock and nothing else changed"
    );
    let erased = erases(&helper);
    assert_eq!(
        erased.last(),
        Some(&u32::try_from(BANK1).unwrap()),
        "marker sector last"
    );
    assert_eq!(
        helper.host_tx(),
        fs::read(execute.join("serial-tx.bin")).unwrap()
    );
    let receipt = receipt(&execute);
    assert!(receipt.verified_terminal);
    assert_eq!(receipt.terminal_snapshot_sha256, Some(json!(sha256(&want))));

    // A plan named by another hash is refused before serial access.
    let again = f.out("again");
    let args = execute_recovery(&current, &plan, &"0".repeat(64), &again, &f);
    f.run(&mut helper, &args).expect(2, "refused");
    assert!(!again.exists());
}

/// A helper fault mid-recovery (here the emulated part refuses the second
/// planned erase) ends the session terminally: the erase is never resent and
/// the journal never claims verification.
#[test]
#[ignore = "needs the private baseline, frozen reviews, session reports and pinned helper"]
fn a_fault_mid_recovery_stops_without_retry() {
    let f = Fixture::new();
    let flash = unbootable(&f);
    let mut reader = helper(&f, &flash, bank1_sectors(BANK1_END - BANK1));
    let (current, plan, sha) = recovery(&f, &mut reader);

    let reviewed: Value = serde_json::from_slice(&fs::read(&plan).unwrap()).unwrap();
    let order: Vec<u64> = reviewed["recovery_plan"]["ordered_sector_payloads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["offset"].as_u64().unwrap())
        .collect();
    let first = u32::try_from(order[0]).unwrap();
    let mut faulty = helper(&f, &flash, vec![first]);
    let execute = f.out("recovery");
    let run = f.run(
        &mut faulty,
        &execute_recovery(&current, &plan, &sha, &execute, &f),
    );
    assert!(run.helper_error.is_some());
    assert_eq!(run.code, 1, "{}", run.stderr);

    assert_eq!(erases(&faulty), [first]);
    let receipt = receipt(&execute);
    assert!(!receipt.verified_terminal);
    assert_eq!(receipt.erase_order, [json!(order[0]), json!(order[1])]);
    let after = faulty.snapshot().unwrap();
    assert!(after[..BANK1] == flash[..BANK1] && after[BANK1_END..] == flash[BANK1_END..]);
}
