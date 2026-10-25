//! `/api/feeder`: the feeder as `wellfed-local serve` last heard it.
//!
//! The page asks the server, never the feeder: one `GET /api/status` on the
//! server's loopback API, with the token from `private/server.json`. That
//! read changes nothing, so it is safe beside a running bench command or
//! integration run. A server that is down or not configured is a state the
//! page shows, not an error.

use std::path::Path;
use std::time::Duration;

use chrono::SecondsFormat;
use serde::Serialize;

use wellfed_formats::bench::health::Health;
use wellfed_formats::local::LocalStatus;
use wellfed_formats::local_api::{Auth, LocalApi, SERVER_CONFIG, ServerConfig};

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
        broker_connected: bool,
        /// `None` when the server has no replacement-firmware role.
        replacement: Option<Box<ReplacementView>>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct ReplacementView {
    online: bool,
    stale: bool,
    /// The health message's age when the server answered.
    age_ms: Option<i64>,
    /// RFC 3339; a new value means a new transport session.
    received_at: Option<String>,
    health: Option<Health>,
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
    let api = LocalApi::new(&config, TIMEOUT);
    let address = api.address.to_string();

    match api.get::<LocalStatus<Health>>("/api/status", Auth::Token) {
        Ok(status) => view(address, status),
        Err(error) => FeederView::Down {
            address,
            error: error.to_string(),
        },
    }
}

fn view(address: String, status: LocalStatus<Health>) -> FeederView {
    let replacement = status.replacement.map(|r| {
        Box::new(ReplacementView {
            online: r.online,
            stale: r.stale,
            age_ms: r.age_ms,
            // As the server wrote it.
            received_at: r
                .received_at
                .map(|at| at.to_rfc3339_opts(SecondsFormat::AutoSi, true)),
            health: r.health,
        })
    });

    FeederView::Up {
        address,
        broker_connected: status.broker_connected,
        replacement,
    }
}

/// This module's page types, for [`super::bindings`].
#[cfg(test)]
pub(super) fn bindings(cfg: &ts_rs::Config) -> Vec<String> {
    use super::bindings::decl;
    vec![decl::<ReplacementView>(cfg), decl::<FeederView>(cfg)]
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
    fn status_shows_health() {
        let status: LocalStatus<Health> = serde_json::from_value(json!({
            "broker_connected": true,
            "replacement": {
                "online": true,
                "stale": false,
                "age_ms": 1200,
                "received_at": "2026-10-22T10:00:00Z",
                "health": {
                    "build": "4d37c21a00", "bank": 1, "seq": 7, "faults": [],
                    "clock": { "syncs": 2, "unix_ms": 1792000000000i64 },
                    "wifi": { "source": "build" }, "transport": { "connects": 1 }
                }
            }
        }))
        .unwrap();

        let view = serde_json::to_value(view("127.0.0.1:8080".into(), status)).unwrap();

        assert_eq!(view["server"], "up");
        assert_eq!(view["broker_connected"], true);
        let replacement = &view["replacement"];
        assert_eq!(replacement["health"]["bank"], 1);
        assert_eq!(replacement["health"]["wifi"]["source"], "build");
        assert_eq!(replacement["received_at"], "2026-10-22T10:00:00Z");
    }
}
