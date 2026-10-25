//! Shared integration-test fixtures: a synthetic configuration and helpers
//! that run the real `wellfed-local serve` binary on loopback ports.

#![allow(dead_code)]
use ed25519_dalek::{Signer, SigningKey};
use wellfed_core::ota::{self, MANIFEST_LEN};
use wellfed_local::config::Config;
use wellfed_local::ota::firmware::{self, CONTAINER_HEADER};
use sha2::{Digest, Sha256};

pub fn config() -> Config {
    Config {
        version: 1,
        device_id: "AF_TEST_ONLY".to_owned(),
        device_client_id: "AF_TEST_ONLY".to_owned(),
        device_username: "test-device".to_owned(),
        device_password: "test-password".to_owned(),
        server_password: "test-server-password-32-characters".to_owned(),
        api_token: "test-api-token-at-least-32-characters".to_owned(),
        mqtt_listen: "127.0.0.1:18883".parse().unwrap(),
        http_listen: "127.0.0.1:18765".parse().unwrap(),
        timezone: chrono_tz::America::Denver,
        plans: vec![],
        binding_response: None,
        hotspot_interface: "wlan1".to_owned(),
        target_mac: "02:00:00:00:00:42".to_owned(),
        firmware: None,
    }
}

use serde_json::Value;
use std::{
    net::TcpListener,
    process::{Child, Command, Stdio},
    time::Duration,
};

pub struct Service(Child);
impl Drop for Service {
    fn drop(&mut self) {
        let _ = Command::new("kill")
            .args(["-INT", &self.0.id().to_string()])
            .status();

        for _ in 0..30 {
            if self.0.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn address() -> std::net::SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

pub fn start(path: &std::path::Path) -> Service {
    start_mode(path, false)
}

pub fn start_mode(path: &std::path::Path, ota: bool) -> Service {
    start_in(path, ota, None)
}

/// Start the server, optionally in `cwd` (paths like `private/...` resolve there).
pub fn start_in(path: &std::path::Path, ota: bool, cwd: Option<&std::path::Path>) -> Service {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wellfed-local"));
    command.args(["serve", "--config"]).arg(path);

    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    if ota {
        // Tests run from uncommitted trees; production refuses such builds.
        command
            .arg("--enable-ota-trial")
            .env(wellfed_local::build_info::ALLOW_UNREVIEWED_ENV, "1");
    }

    Service(
        command
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    )
}

/// Waits up to 10 s: a loaded host (a parallel test binary, a bench update)
/// can delay the spawned server's broker connection past 5 s.
pub async fn ready(config: &Config, client: &reqwest::Client) {
    for _ in 0..200 {
        if let Ok(response) = client
            .get(format!("http://{}/api/status", config.http_listen))
            .bearer_auth(&config.api_token)
            .send()
            .await
            && let Ok(value) = response.json::<Value>().await
            && value["broker_connected"] == true
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("local service did not become ready");
}

pub async fn status(config: &Config, client: &reqwest::Client) -> Value {
    client
        .get(format!("http://{}/api/status", config.http_listen))
        .bearer_auth(&config.api_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

/// An OTA container of `len` payload bytes, `fill` after the loader marker.
pub fn container(len: usize, fill: impl Fn(usize) -> u8) -> Vec<u8> {
    let mut out = vec![0u8; CONTAINER_HEADER];
    let mut payload: Vec<u8> = (0..len).map(fill).collect();
    payload[..8].copy_from_slice(&ota::MARKER);
    out.extend(payload);
    out
}

/// A manifest for `container` signed with `key`, as `wellfed-build sign`
/// writes one from a frozen review. A restatement of `wellfed-core::ota`'s
/// format rather than the real signer: that one signs only a verified
/// review, which these delivery fixtures are not, and this file goes with
/// the crate when its MQTT offers move to the device API.
pub fn signed(key: &SigningKey, container: &[u8], version: u32, build_id: [u8; 32]) -> Vec<u8> {
    let payload = firmware::payload(container).unwrap();
    let body = ota::manifest_body(&ota::Manifest {
        version,
        image_len: payload.len() as u32,
        target_bank: 1,
        image_sha256: Sha256::digest(payload).into(),
        build_id,
    });
    let mut out = body.to_vec();
    out.extend(key.sign(&body).to_bytes());
    assert_eq!(out.len(), MANIFEST_LEN);
    out
}
