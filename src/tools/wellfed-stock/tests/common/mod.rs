//! Shared fixtures: a synthetic stock configuration, the controller over
//! it, and helpers that run the real `wellfed-stock serve` binary on
//! loopback ports.

#![allow(dead_code)]
use std::{
    net::{Ipv4Addr, SocketAddr, TcpListener},
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

    /// Wait until the server accepts connections at `at`. A server that
    /// exits first fails the test with its exit status (its errors are on
    /// the test's standard error); one that never listens fails after a
    /// minute, far past a loaded start, rather than at nextest's timeout.
    pub async fn listening(&mut self, at: std::net::SocketAddr) {
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while std::net::TcpStream::connect(at).is_err() {
            if let Some(status) = self.0.try_wait().unwrap() {
                panic!("the server exited before it listened on {at}: {status}");
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the server never listened on {at}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
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
/// binds once it has started.
///
/// A port is free when chosen but taken only later, by the server's own
/// bind. One from the kernel's ephemeral range could meanwhile go to any
/// process's port-0 bind or outgoing connection (the server's broker then
/// fails `address in use`), so these are drawn at random from below it,
/// where nothing is handed out unasked. Every listener is held until all
/// are chosen, so the `N` differ.
pub fn addresses<const N: usize>() -> std::io::Result<[SocketAddr; N]> {
    addresses_on(Ipv4Addr::LOCALHOST)
}

/// As [`addresses`], on another loopback `host`.
pub fn addresses_on<const N: usize>(host: Ipv4Addr) -> std::io::Result<[SocketAddr; N]> {
    const LOW: u16 = 10_000;
    let ephemeral = std::fs::read_to_string("/proc/sys/net/ipv4/ip_local_port_range")
        .ok()
        .and_then(|range| range.split_whitespace().next()?.parse::<u16>().ok())
        .unwrap_or(32_768);
    let span = ephemeral
        .checked_sub(LOW)
        .filter(|span| *span > 0)
        .ok_or_else(|| {
            std::io::Error::other(format!(
                "the ephemeral range starts at {ephemeral}: no room below it above {LOW}"
            ))
        })?;

    // std's hasher keys come from the operating system's random source.
    use std::hash::BuildHasher as _;
    let random = std::hash::RandomState::new();
    let mut held = Vec::with_capacity(N);
    let mut draw = 0u64;
    while held.len() < N {
        if draw == 10_000 {
            return Err(std::io::Error::other(format!(
                "no {N} free ports below {ephemeral} on {host}"
            )));
        }
        let offset = u16::try_from(random.hash_one(draw) % u64::from(span)).unwrap();
        if let Ok(listener) = TcpListener::bind((host, LOW + offset)) {
            held.push(listener);
        }
        draw += 1;
    }
    let addresses = held
        .iter()
        .map(TcpListener::local_addr)
        .collect::<std::io::Result<Vec<_>>>()?;
    Ok(addresses
        .try_into()
        .expect("one address for each of the N listeners"))
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
