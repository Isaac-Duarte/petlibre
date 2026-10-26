//! Shared fixtures: a synthetic stock configuration and the controller
//! over it.

#![allow(dead_code)]
use std::path::PathBuf;

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
        plans: vec![],
        binding_response: None,
        hotspot_interface: "wlan1".to_owned(),
        target_mac: "02:00:00:00:00:42".to_owned(),
    }
}

/// The controller over `path`'s ledger, with [`config`].
pub fn load(path: PathBuf) -> anyhow::Result<Controller> {
    Controller::load(config(), path, BUILD)
}
