#![allow(dead_code)]
use petlibre_local::config::Config;

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
        max_portions: 1,
        plans: vec![],
        binding_response: None,
        hotspot_interface: "wlan1".to_owned(),
        target_mac: "02:00:00:00:00:42".to_owned(),
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
    let mut command = Command::new(env!("CARGO_BIN_EXE_petlibre-local"));
    command.args(["serve", "--config"]).arg(path);
    if ota {
        // Tests run from uncommitted trees; production refuses such builds.
        command
            .arg("--enable-ota-trial")
            .env(petlibre_local::build_info::ALLOW_UNREVIEWED_ENV, "1");
    }
    Service(
        command
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    )
}

pub async fn ready(config: &Config, client: &reqwest::Client) {
    for _ in 0..100 {
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
