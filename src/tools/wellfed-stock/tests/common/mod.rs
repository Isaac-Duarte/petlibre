//! Shared fixtures: a synthetic stock configuration, the controller over
//! it, and helpers that run the real `wellfed-stock serve` binary on
//! loopback ports.

#![allow(dead_code)]
use std::{
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};

use serde_json::Value;

use wellfed_stock::{build_info::Build, config::Config};

pub use wellfed_stock::controller::Controller;

/// The identity a test build reports.
pub const BUILD: Build = Build {
    commit: "unknown",
    clean: false,
};

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
        plans: Some(vec![]),
        binding_response: None,
        hotspot_interface: Some("wlan1".to_owned()),
        target_mac: Some("02:00:00:00:00:42".to_owned()),
        device_address: None,
    }
}

/// The controller over `path`'s ledger, with [`config`].
pub fn load(path: PathBuf) -> anyhow::Result<Controller> {
    Controller::load(config(), path, BUILD)
}

pub struct Service(Child);

impl Service {
    /// Run `command` (a `wellfed-stock` invocation) until dropped.
    pub fn spawn(mut command: Command) -> Self {
        Self(
            command
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        )
    }
}

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

/// `N` distinct free loopback addresses, for the ports a spawned server
/// binds. Every listener is held until all are chosen: a port released at
/// once can be handed out again by the next bind, and a server given one
/// port for both its broker and its API fails to start.
pub fn addresses<const N: usize>() -> [std::net::SocketAddr; N] {
    let held: [TcpListener; N] = std::array::from_fn(|_| TcpListener::bind("127.0.0.1:0").unwrap());
    held.map(|listener| listener.local_addr().unwrap())
}

pub fn start(path: &std::path::Path) -> Service {
    start_mode(path, false)
}

pub fn start_mode(path: &std::path::Path, ota: bool) -> Service {
    start_in(path, ota, None)
}

/// Start the server, optionally in `cwd` (paths like `private/...` resolve there).
pub fn start_in(path: &std::path::Path, ota: bool, cwd: Option<&std::path::Path>) -> Service {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wellfed-stock"));
    command.args(["serve", "--config"]).arg(path);

    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    if ota {
        // Tests run from uncommitted trees; production refuses such builds.
        command
            .arg("--enable-ota-trial")
            .env(wellfed_stock::build_info::ALLOW_UNREVIEWED_ENV, "1");
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
    panic!("stock server did not become ready");
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
