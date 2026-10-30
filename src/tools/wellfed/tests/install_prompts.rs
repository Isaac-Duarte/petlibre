//! `wellfed install --prompts lines`, run as the bench server runs it: its
//! standard output carries prompt lines and nothing else, and a standard
//! input that closes before an answer ends the run. No feeder needed: the
//! DNS route asks for the override before the stock server starts, and the
//! stock server, which runs in the installer's own process, only listens.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use wellfed_formats::bench::prompt::Prompt;
use wellfed_server_testkit::release;

/// A name in the reserved `.invalid` domain: the run's advisory lookup of
/// it fails at once, and the run goes on regardless.
const BROKER_NAME: &str = "broker.invalid";
/// The broker's address: a loopback address of its own, on the port the
/// DNS route requires, so nothing else on this host listens there.
const BROKER: &str = "127.0.83.1";

/// `install --prompts lines` on the DNS route, with the files it reads
/// before its first prompt under `dir`. HOME and XDG point into `dir`, so
/// no bench server running for this user is found.
fn install(dir: &Path) -> Command {
    let path = |name: &str| dir.join(name);
    let secret = "s".repeat(32);
    std::fs::write(
        path("installer.json"),
        format!(
            r#"{{"version": 1, "server_password": "{secret}", "api_token": "{secret}",
                "mqtt_listen": "{BROKER}:1883", "http_listen": "127.0.0.1:8765",
                "timezone": "UTC"}}"#
        ),
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
        .args(["--prompts", "lines"])
        .env("HOME", dir)
        .env("XDG_RUNTIME_DIR", dir)
        // Tests run from uncommitted trees; a real install refuses them.
        .env("WELLFED_ALLOW_UNREVIEWED_OTA_BUILD", "1");
    command
}

fn set_override() -> Prompt {
    Prompt::SetOverride {
        name: BROKER_NAME.into(),
        broker: BROKER.parse().unwrap(),
    }
}

#[test]
fn lines_mode_writes_only_its_prompt_and_stops_at_a_closed_input() {
    let dir = tempfile::tempdir().unwrap();
    let output = install(dir.path()).stdin(Stdio::null()).output().unwrap();

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

/// The stock server starts in the installer's process before the restart
/// prompt; what it and the run print goes to standard error, so standard
/// output still holds prompt lines alone. Closing the input there stops
/// the run, the server with it.
#[test]
fn the_stock_server_s_output_stays_off_the_prompt_lines() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = install(dir.path())
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

    line.clear();
    stdout.read_line(&mut line).unwrap();
    assert_eq!(
        serde_json::from_str::<Prompt>(&line).unwrap(),
        Prompt::RestartFeeder
    );
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
        stderr.contains("Started the stock server (its broker trusts the first feeder"),
        "{stderr}"
    );
    assert!(!status.success());
    assert!(
        stderr.contains("standard input closed with the restart-feeder prompt unanswered"),
        "{stderr}"
    );
}
