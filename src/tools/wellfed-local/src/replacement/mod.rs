//! Replacement-firmware health.
//!
//! The firmware publishes a versioned health object over MQTT. Only
//! bank 0's recovery role still does: the normal role answers requests
//! and reports its status over the Noise device API instead, and this server
//! sends it nothing but OTA offer answers. The code here validates only what
//! the server relies on and keeps the latest health in memory.

use chrono::{DateTime, Utc};
use wellfed_formats::local::Replacement;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const HEALTH_SCHEMA: &str = "wellfed.health";
pub const HEALTH_VERSION: u64 = 1;

pub const MAX_HEALTH_BYTES: usize = 704;

/// Two missed 30 s publishes plus margin.
pub const STALE_SECONDS: i64 = 75;

/// A health object as published. Only what the server acts on is typed; every
/// other field is kept (and reported through the API) as sent.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Health {
    pub schema: String,
    /// The schema's major version.
    pub v: u64,
    pub online: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The server's view of the latest health.
pub type Snapshot = Replacement<Health>;

#[derive(Default)]
pub struct Tracker {
    health: Option<(Health, DateTime<Utc>)>,
    rejected: u64,
}

/// A health object the server accepts: schema and major version match and
/// `online` is a boolean. Unknown fields are kept and ignored.
pub fn parse_health(payload: &[u8]) -> Option<Health> {
    if payload.len() > MAX_HEALTH_BYTES {
        return None;
    }
    let health: Health = serde_json::from_slice(payload).ok()?;
    (health.schema == HEALTH_SCHEMA && health.v == HEALTH_VERSION).then_some(health)
}

impl Tracker {
    pub fn observe_health(&mut self, payload: &[u8], now: DateTime<Utc>) -> bool {
        match parse_health(payload) {
            Some(health) => {
                self.health = Some((health, now));
                true
            }
            None => {
                self.rejected += 1;
                false
            }
        }
    }

    pub fn snapshot(&self, now: DateTime<Utc>) -> Snapshot {
        let (health, received_at) = match &self.health {
            Some((health, at)) => (Some(health.clone()), Some(*at)),
            None => (None, None),
        };

        let age_ms = received_at.map(|at| (now - at).num_milliseconds());
        // A clock step backwards also counts as stale rather than fresh.
        let stale = age_ms.is_none_or(|age| !(0..STALE_SECONDS * 1000).contains(&age));
        let online = !stale && health.as_ref().is_some_and(|health| health.online);

        Snapshot {
            health,
            received_at,
            age_ms,
            online,
            stale,
            rejected: self.rejected,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn health(extra: Value) -> Vec<u8> {
        let mut value = json!({"schema":"wellfed.health","v":1,"online":true,"seq":1});
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::to_vec(&value).unwrap()
    }

    #[test]
    fn health_requires_schema_major_version_and_online_and_keeps_unknown_fields() {
        assert!(parse_health(&health(json!({"future":"field"}))).is_some());

        for bad in [
            json!({"schema":"other"}),
            json!({"v":2}),
            json!({"v":"1"}),
            json!({"online":"yes"}),
        ] {
            assert!(parse_health(&health(bad.clone())).is_none(), "{bad}");
        }

        assert!(parse_health(b"[1]").is_none());
        assert!(parse_health(b"not json").is_none());

        let padded = health(json!({"pad":"x".repeat(MAX_HEALTH_BYTES)}));
        assert!(parse_health(&padded).is_none());
    }

    #[test]
    fn health_goes_stale_after_75_seconds_and_offline_follows_the_message() {
        let mut tracker = Tracker::default();
        let t0 = Utc::now();
        assert_eq!(
            tracker.snapshot(t0),
            Snapshot {
                stale: true,
                ..Default::default()
            }
        );

        assert!(tracker.observe_health(&health(json!({})), t0));

        let fresh = tracker.snapshot(t0 + chrono::Duration::seconds(74));
        assert!(fresh.online && !fresh.stale);
        let old = tracker.snapshot(t0 + chrono::Duration::seconds(75));
        assert!(!old.online && old.stale);
        assert!(tracker.snapshot(t0 - chrono::Duration::seconds(1)).stale);

        assert!(tracker.observe_health(&health(json!({"online":false})), t0));
        assert!(!tracker.snapshot(t0).online);
        assert!(!tracker.observe_health(b"{}", t0));
        assert_eq!(tracker.snapshot(t0).rejected, 1);
    }
}
