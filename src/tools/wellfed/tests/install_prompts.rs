//! `wellfed install --prompts lines`, run as the bench server runs it: its
//! standard output carries prompt lines and nothing else. No feeder needed:
//! the stock server, which runs in the installer's own process, only listens.

use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};

use wellfed_server_testkit::release;

/// The broker's address: a loopback address of its own, so nothing else on
/// this host listens there.
const BROKER: &str = "127.0.83.1";

/// `install --prompts lines`, with the files it reads
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
        .arg("--keys")
        .arg(path("owner.json"))
        .args(["--prompts", "lines"])
        .env("HOME", dir)
        .env("XDG_RUNTIME_DIR", dir)
        // Tests run from uncommitted trees; a real install refuses them.
        .env("WELLFED_ALLOW_UNREVIEWED_OTA_BUILD", "1");
    command
}

/// The stock server runs in the installer's process, and what it and the run
/// print goes to standard error, so standard output still holds prompt lines
/// alone: none yet, since stock has not connected.
#[test]
fn the_stock_server_s_output_stays_off_the_prompt_lines() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = install(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut said = String::new();
    while !said.contains("Started the stock server (its broker trusts the first feeder") {
        assert_ne!(stderr.read_line(&mut said).unwrap(), 0, "{said}");
    }
    child.kill().unwrap();

    let mut stdout = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    child.wait().unwrap();
    assert_eq!(stdout, "", "stdout; stderr: {said}");
}
