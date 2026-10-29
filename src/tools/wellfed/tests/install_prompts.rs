//! `wellfed install --prompts lines`, run as the bench server runs it: its
//! standard output carries prompt lines and nothing else, and a standard
//! input that closes before an answer ends the run. No feeder needed: the
//! DNS route's first prompt comes before the stock server starts.

use std::process::{Command, Stdio};

use wellfed_formats::bench::prompt::Prompt;
use wellfed_server_testkit::release;

#[test]
fn lines_mode_writes_only_its_prompt_and_stops_at_a_closed_input() {
    let dir = tempfile::tempdir().unwrap();
    let path = |name: &str| dir.path().join(name);

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

    // HOME and XDG point at the temporary directory, so no bench server
    // running for this user is found.
    let output = Command::new(env!("CARGO_BIN_EXE_wellfed"))
        .arg("install")
        .arg("--stock-config")
        .arg(path("installer.json"))
        .arg("--review")
        .arg(&review)
        .arg("--manifest")
        .arg(path("manifest.bin"))
        .arg("--broker-name")
        .arg("broker.example.net")
        .arg("--keys")
        .arg(path("owner.json"))
        .arg("--stock-server")
        .arg(path("no-stock-server"))
        .args(["--prompts", "lines"])
        .env("HOME", dir.path())
        .env("XDG_RUNTIME_DIR", dir.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();

    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "stdout: {stdout}\nstderr: {stderr}");
    assert_eq!(
        serde_json::from_str::<Prompt>(lines[0]).unwrap(),
        Prompt::SetOverride {
            name: "broker.example.net".into(),
            broker: "127.0.0.1".parse().unwrap(),
        }
    );

    assert!(!output.status.success());
    assert!(
        stderr.contains("standard input closed with the set-override prompt unanswered"),
        "{stderr}"
    );
}
