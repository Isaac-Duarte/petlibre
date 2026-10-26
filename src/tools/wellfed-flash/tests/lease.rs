//! The `wellfed-flash` binary against the bench lease, with no port: a
//! device command refuses another tool's or a killed run's lease before any
//! serial access, joins its parent's, and an offline command needs none.
//! The port named does not exist, so nothing here can reach a device.

use std::path::Path;
use std::process::{Child, Command, Output, Stdio};

use wellfed_formats::bench::lease::{self, Holder, LEASE, Lease};
use wellfed_formats::private::write_json;
use serde_json::Value;

const BINARY: &str = env!("CARGO_BIN_EXE_wellfed-flash");
const NO_PORT: &str = "/nonexistent/wellfed-flash-test-port";

fn flash(root: &Path, args: &[&str]) -> Output {
    Command::new(BINARY)
        .arg("--root")
        .arg(root)
        .args(args)
        .current_dir(root)
        .env_remove("WELLFED_FLASH_EMULATOR")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn observe(root: &Path) -> Output {
    let output = root.join("private/observe-out").display().to_string();
    flash(
        root,
        &["read", "observe", "--port", NO_PORT, "--output", &output],
    )
}

fn summary(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.lines().last().unwrap()).unwrap()
}

/// Another process holding the bench under `root`.
fn hold(root: &Path) -> Child {
    let child = Command::new("sleep").arg("60").spawn().unwrap();
    let command = vec!["wellfed-bench".into(), "dump".into(), "d1".into()];
    let holder = Holder::of(child.id(), command).unwrap();
    write_json(&root.join(LEASE), &holder).unwrap();
    child
}

#[test]
fn a_device_command_refuses_a_held_bench_before_the_port() {
    let root = tempfile::tempdir().unwrap();
    let mut other = hold(root.path());

    let output = observe(root.path());

    assert_eq!(output.status.code(), Some(2));
    let summary = summary(&output);
    assert_eq!(summary["status"], "refused");
    assert_eq!(summary["error_type"], "refused");
    let error = summary["error"].as_str().unwrap();
    assert!(error.contains("The bench is held by pid"), "{error}");
    assert!(!root.path().join("private/observe-out").exists());

    other.kill().unwrap();
    other.wait().unwrap();
}

#[test]
fn a_killed_runs_lease_is_reported_not_taken() {
    let root = tempfile::tempdir().unwrap();
    let mut other = hold(root.path());
    other.kill().unwrap();
    other.wait().unwrap();

    let output = observe(root.path());

    assert_eq!(output.status.code(), Some(2));
    let error = summary(&output)["error"].as_str().unwrap().to_string();
    assert!(error.contains("outlived its process"), "{error}");
    assert!(lease::inspect(root.path()).unwrap().is_some());
}

#[test]
fn a_child_of_the_holder_runs_on_its_lease_and_leaves_it() {
    let root = tempfile::tempdir().unwrap();
    // This test process stands as the bench, wellfed-flash as its child.
    let held = Lease::acquire(root.path()).unwrap();

    let output = observe(root.path());

    // It went past the lease, opened its session and stopped on the
    // missing port.
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(summary(&output)["status"], "failed");
    let (holder, _) = lease::inspect(root.path()).unwrap().unwrap();
    assert_eq!(&holder, held.holder());
}

#[test]
fn an_offline_command_needs_no_lease() {
    let root = tempfile::tempdir().unwrap();
    let mut other = hold(root.path());

    let output = flash(
        root.path(),
        &["decode-status", "--sr1", "0", "--sr2", "2", "--sr3", "0"],
    );

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    other.kill().unwrap();
    other.wait().unwrap();
}
