//! `/api/feeder`: stock as `wellfed-stock serve` last heard it.
//!
//! The page asks the server, never the feeder: one `GET /api/status` on the
//! server's loopback API, with the token from `private/server.json`. That
//! read changes nothing, so it is safe beside a running bench command or
//! integration run. A server that is down or not configured is a state the
//! page shows, not an error. The server hears only stock: the replacement
//! firmware speaks no MQTT, and answers its device API instead.

use std::path::Path;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use wellfed_formats::stock::StockStatus;
use wellfed_formats::stock::{SERVER_CONFIG, ServerConfig};
use wellfed_stock_api::{Auth, StockApi};

/// The page polls; a slow server must not hold a worker for long.
const TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(tag = "server", rename_all = "lowercase")]
pub(crate) enum FeederView {
    /// No readable `private/server.json`.
    Unconfigured { config: &'static str, error: String },
    /// Configured, but nothing answers (serve is not running) or it refused.
    Down { address: String, error: String },
    Up {
        address: String,
        /// The server's own session with its broker.
        broker_connected: bool,
        /// Stock's telemetry is current.
        stock_online: bool,
        /// The version stock last reported, as it sent it.
        stock_version: Option<String>,
    },
}

pub(crate) fn feeder(root: &Path) -> FeederView {
    let config: ServerConfig = match wellfed_formats::load(&root.join(SERVER_CONFIG)) {
        Ok(config) => config,
        Err(error) => {
            return FeederView::Unconfigured {
                config: SERVER_CONFIG,
                error: format!("{SERVER_CONFIG}: {error}"),
            };
        }
    };
    let api = StockApi::new(&config, TIMEOUT);
    let address = api.address.to_string();

    match api.get::<StockStatus>("/api/status", Auth::Token) {
        Ok(status) => view(address, &status),
        Err(error) => FeederView::Down {
            address,
            error: error.to_string(),
        },
    }
}

fn view(address: String, status: &StockStatus) -> FeederView {
    FeederView::Up {
        address,
        broker_connected: status.broker_connected,
        stock_online: status.device_online,
        stock_version: status
            .attributes
            .get("softwareVersion")
            .and_then(Value::as_str)
            .map(str::to_owned),
    }
}

/// This module's page types, for [`super::bindings`].
#[cfg(test)]
pub(super) fn bindings(cfg: &ts_rs::Config) -> Vec<String> {
    use super::bindings::decl;
    vec![decl::<FeederView>(cfg)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn without_a_config_the_feeder_is_unconfigured() {
        let tmp = tempfile::tempdir().unwrap();

        let view = serde_json::to_value(feeder(tmp.path())).unwrap();

        assert_eq!(view["server"], "unconfigured");
        assert_eq!(view["config"], SERVER_CONFIG);
    }

    #[test]
    fn a_server_that_does_not_answer_is_down() {
        let tmp = tempfile::tempdir().unwrap();
        // A port just released: nothing listens there.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let config = json!({
            "api_token": "t",
            "http_listen": format!("127.0.0.1:{port}"),
            "mqtt_listen": "127.0.0.1:1883",
        });
        std::fs::create_dir_all(tmp.path().join("private")).unwrap();
        std::fs::write(tmp.path().join(SERVER_CONFIG), config.to_string()).unwrap();

        let view = serde_json::to_value(feeder(tmp.path())).unwrap();

        assert_eq!(view["server"], "down");
        assert_eq!(view["address"], format!("127.0.0.1:{port}"));
    }

    #[test]
    fn status_shows_stock() {
        let status: StockStatus = serde_json::from_value(json!({
            "broker_connected": true,
            "device_online": true,
            "attributes": { "softwareVersion": "1.1.15", "powerType": 1 },
        }))
        .unwrap();

        let view = serde_json::to_value(view("127.0.0.1:8080".into(), &status)).unwrap();

        assert_eq!(view["server"], "up");
        assert_eq!(view["broker_connected"], true);
        assert_eq!(view["stock_online"], true);
        assert_eq!(view["stock_version"], "1.1.15");
    }
}
