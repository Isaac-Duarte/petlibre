//! Replacement-firmware health and diagnostic replies (FW08).
//!
//! The firmware publishes a versioned health object and answers a read-only
//! request allowlist. This module validates only what the server relies on and
//! keeps the latest health in memory; nothing here commands the feeder.
use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

pub const HEALTH_SCHEMA: &str = "wellfed.health";
pub const HEALTH_VERSION: u64 = 1;

pub const MAX_HEALTH_BYTES: usize = 512;
pub const MAX_REPLY_BYTES: usize = 256;
pub const REPLIES_KEPT: usize = 8;

/// Two missed 30 s publishes plus margin.
pub const STALE_SECONDS: i64 = 75;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    Health,
    Ping,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub op: Op,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    /// No firmware role in this server's configuration.
    Unconfigured,
    /// No fresh online health: a QoS 0 request would be lost unseen.
    Offline,
}

impl RequestError {
    pub fn message(self) -> &'static str {
        match self {
            Self::Unconfigured => "no replacement-firmware role is configured",
            Self::Offline => "replacement firmware is offline or its health is stale",
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Reply {
    pub received_at: DateTime<Utc>,
    pub message: Value,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Snapshot {
    pub health: Option<Value>,
    pub received_at: Option<DateTime<Utc>>,
    pub age_ms: Option<i64>,
    pub online: bool,
    pub stale: bool,
    pub rejected: u64,
    pub replies: Vec<Reply>,
}

#[derive(Default)]
pub struct Tracker {
    health: Option<(Value, DateTime<Utc>)>,
    replies: VecDeque<Reply>,
    rejected: u64,
}

fn object(payload: &[u8], limit: usize) -> Option<Map<String, Value>> {
    if payload.len() > limit {
        return None;
    }
    match serde_json::from_slice(payload).ok()? {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

/// A health object the server accepts: schema and major version match, and
/// `online` is a boolean. Unknown fields are kept and ignored.
pub fn parse_health(payload: &[u8]) -> Option<Value> {
    let map = object(payload, MAX_HEALTH_BYTES)?;
    (map.get("schema")?.as_str()? == HEALTH_SCHEMA
        && map.get("v")?.as_u64()? == HEALTH_VERSION
        && map.get("online")?.is_boolean())
    .then_some(Value::Object(map))
}

/// A reply the server keeps: an object whose `id` is a UUID.
pub fn parse_reply(payload: &[u8]) -> Option<Value> {
    let map = object(payload, MAX_REPLY_BYTES)?;
    Uuid::parse_str(map.get("id")?.as_str()?).ok()?;
    Some(Value::Object(map))
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

    pub fn observe_reply(&mut self, payload: &[u8], now: DateTime<Utc>) -> bool {
        match parse_reply(payload) {
            Some(message) => {
                if self.replies.len() == REPLIES_KEPT {
                    self.replies.pop_front();
                }
                self.replies.push_back(Reply {
                    received_at: now,
                    message,
                });
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
        let online = !stale
            && health
                .as_ref()
                .and_then(|h| h["online"].as_bool())
                .unwrap_or(false);

        Snapshot {
            health,
            received_at,
            age_ms,
            online,
            stale,
            rejected: self.rejected,
            replies: self.replies.iter().cloned().collect(),
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

    #[test]
    fn replies_need_a_uuid_id_are_bounded_and_keep_the_latest_eight() {
        let mut tracker = Tracker::default();
        let now = Utc::now();
        assert!(!tracker.observe_reply(br#"{"id":"nope","ok":true}"#, now));
        let long = format!(
            r#"{{"id":"{}","pad":"{}"}}"#,
            Uuid::new_v4(),
            "x".repeat(300)
        );
        assert!(!tracker.observe_reply(long.as_bytes(), now));

        let ids: Vec<_> = (0..10).map(|_| Uuid::new_v4()).collect();
        for id in &ids {
            let reply = format!(r#"{{"id":"{id}","op":"ping","ok":true}}"#);
            assert!(tracker.observe_reply(reply.as_bytes(), now));
        }

        let kept: Vec<_> = tracker
            .snapshot(now)
            .replies
            .iter()
            .map(|r| r.message["id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            kept,
            ids[2..].iter().map(Uuid::to_string).collect::<Vec<_>>()
        );
    }

    #[test]
    fn only_allowlisted_operations_parse() {
        assert_eq!(
            serde_json::from_str::<Input>(r#"{"op":"ping"}"#)
                .unwrap()
                .op,
            Op::Ping
        );
        assert_eq!(
            serde_json::from_str::<Input>(r#"{"op":"health"}"#)
                .unwrap()
                .op,
            Op::Health
        );

        for bad in [r#"{"op":"reboot"}"#, r#"{"op":"ping","x":1}"#, r#"{}"#] {
            assert!(serde_json::from_str::<Input>(bad).is_err(), "{bad}");
        }
    }
}
