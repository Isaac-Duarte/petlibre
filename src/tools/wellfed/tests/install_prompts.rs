//! `wellfed install --prompts lines`, run as the bench server runs it: its
//! standard output carries prompt lines and nothing else, and a standard
//! input that closes before an answer ends the run. No feeder needed: the
//! DNS route asks for the override before the stock server starts, and the
//! stock server here is a stand-in script.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use wellfed_formats::bench::prompt::Prompt;
use wellfed_server_testkit::release;

/// A name in the reserved `.invalid` domain: the run's advisory lookup of
/// it fails at once, and the run goes on regardless.
const BROKER_NAME: &str = "broker.invalid";

/// `install --prompts lines` on the DNS route, with the files it reads
/// before its first prompt under `dir`, and `stock` as `wellfed-stock`.
/// HOME and XDG point into `dir`, so no bench server running for this user
/// is found.
fn install(dir: &Path, stock: &Path) -> Command {
    let path = |name: &str| dir.join(name);
    std::fs::write(
        path("installer.json"),
        r#"{"api_token": "t", "http_listen": "127.0.0.1:8765", "mqtt_listen": "127.0.0.1:1883"}"#,
    )
    .unwrap();
    let review = path("review");
    std::fs::create_dir(&review).unwrap();
    std::fs::write(
        review.join("build-manifest.json"),
        format!(r#"{{"startup_identity": "{}"}}"#, "ab".repeat(32)),
    )
    .unwrap();
    std::fs::write(review.join("image.bin"), [0u8; 64]).unwrap();
    // A release file is the manifest followed by its image; the run reads
    // only the manifest's version before the first prompt.
    let image_len = 16;
    let file = release::signed(&release::project(), 7, [0xab; 32], image_len);
    std::fs::write(path("manifest.bin"), &file[..file.len() - image_len]).unwrap();

    let mut command = Command::new(env!("CARGO_BIN_EXE_wellfed"));
    command
        .arg("install")
        .arg("--stock-config")
        .arg(path("installer.json"))
        .arg("--review")
        .arg(&review)
        .arg("--manifest")
        .arg(path("manifest.bin"))
        .arg("--broker-name")
        .arg(BROKER_NAME)
        .arg("--keys")
        .arg(path("owner.json"))
        .arg("--stock-server")
        .arg(stock)
        .args(["--prompts", "lines"])
        .env("HOME", dir)
        .env("XDG_RUNTIME_DIR", dir);
    command
}

fn set_override() -> Prompt {
    Prompt::SetOverride {
        name: BROKER_NAME.into(),
        broker: "127.0.0.1".parse().unwrap(),
    }
}

#[test]
fn lines_mode_writes_only_its_prompt_and_stops_at_a_closed_input() {
    let dir = tempfile::tempdir().unwrap();
    let output = install(dir.path(), &dir.path().join("no-stock-server"))
        .stdin(Stdio::null())
        .output()
        .unwrap();

    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "stdout: {stdout}\nstderr: {stderr}");
    assert_eq!(
        serde_json::from_str::<Prompt>(lines[0]).unwrap(),
        set_override()
    );

    assert!(!output.status.success());
    assert!(
        stderr.contains("standard input closed with the set-override prompt unanswered"),
        "{stderr}"
    );
}

/// The stock server's own output goes to install's standard error: the
/// stand-in prints a line on its standard output, and install's standard
/// output still holds prompt lines alone.
#[test]
fn the_stock_server_s_output_stays_off_the_prompt_lines() {
    let dir = tempfile::tempdir().unwrap();
    let started = dir.path().join("stock-started");
    let stock = dir.path().join("wellfed-stock");
    // `exec` makes the sleep the process install stops, so nothing is left
    // holding the output pipes open after the run.
    std::fs::write(
        &stock,
        format!(
            "#!/bin/sh\necho 'stand-in stock server on stdout'\ntouch '{}'\nexec sleep 30\n",
            started.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&stock, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut child = install(dir.path(), &stock)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert_eq!(
        serde_json::from_str::<Prompt>(&line).unwrap(),
        set_override()
    );
    stdin.write_all(b"\"done\"\n").unwrap();

    // The stock server starts before the restart prompt; wait for the
    // stand-in to have printed, then close the input at that prompt.
    line.clear();
    stdout.read_line(&mut line).unwrap();
    assert_eq!(
        serde_json::from_str::<Prompt>(&line).unwrap(),
        Prompt::RestartFeeder
    );
    wait_for(&started);
    drop(stdin);

    let mut rest = String::new();
    stdout.read_to_string(&mut rest).unwrap();
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    let status = child.wait().unwrap();

    assert_eq!(rest, "", "stdout after the prompts; stderr: {stderr}");
    assert!(
        stderr.contains("stand-in stock server on stdout"),
        "{stderr}"
    );
    assert!(!status.success());
    assert!(
        stderr.contains("standard input closed with the restart-feeder prompt unanswered"),
        "{stderr}"
    );
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "{} never appeared",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
