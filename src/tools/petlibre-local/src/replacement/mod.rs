//! Replacement-firmware health and diagnostic replies (FW08).
//!
//! The firmware publishes a versioned health object and answers a read-only
//! request allowlist. The code in this file validates only what the server
//! relies on and keeps the latest health in memory; none of it commands the
//! feeder.
//!
//! The submodules reach the replacement firmware itself: `request_auth` signs
//! the requests sent over MQTT, `wifi_ble` sends Wi-Fi credentials over
//! Bluetooth LE, and `speak` streams audio to the speaker variant.

pub mod request_auth;
pub mod speak;
pub mod wifi_ble;

use std::collections::VecDeque;

use chrono::{DateTime, TimeDelta, Utc};
use chrono_tz::Tz;
use petlibre_core::tone::Tune;
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::protocol::{self, lenient};
use request_auth::{Request, RequestKey};

pub const HEALTH_SCHEMA: &str = "petlibre.health";
pub const HEALTH_VERSION: u64 = 1;

pub const MAX_HEALTH_BYTES: usize = 704;
pub const MAX_REPLY_BYTES: usize = 256;
pub const REPLIES_KEPT: usize = 8;

/// Two missed 30 s publishes plus margin.
pub const STALE_SECONDS: i64 = 75;

/// FW21: resync a valid firmware clock this often, and an unsynced one no
/// more than this often (health arrives every 30 s).
pub const TIME_RESYNC: TimeDelta = TimeDelta::minutes(10);
pub const TIME_RETRY: TimeDelta = TimeDelta::seconds(60);
/// How far ahead to look for the zone's next offset change.
const TRANSITION_HORIZON_DAYS: i64 = 400;

/// The request allowlist; `name` is the wire spelling, in requests and in
/// the API's answers alike.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(try_from = "String", into = "&'static str")]
pub enum Op {
    Health,
    Ping,
    /// FW25: read the device settings, or change them (signed).
    Settings,
    /// FW25: read, store or clear the Wi-Fi credentials (signed).
    Wifi,
    /// FW24: play one built-in tune (signed).
    Play,
}

impl Op {
    pub(crate) const ALL: [Self; 5] = [
        Self::Health,
        Self::Ping,
        Self::Settings,
        Self::Wifi,
        Self::Play,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Health => "health",
            Self::Ping => "ping",
            Self::Settings => "settings",
            Self::Wifi => "wifi",
            Self::Play => "play",
        }
    }
}

impl From<Op> for &'static str {
    fn from(op: Op) -> Self {
        op.name()
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unsupported operation {0:?}")]
pub struct UnknownOp(String);

impl TryFrom<String> for Op {
    type Error = UnknownOp;

    fn try_from(name: String) -> Result<Self, UnknownOp> {
        Self::ALL
            .into_iter()
            .find(|op| op.name() == name)
            .ok_or(UnknownOp(name))
    }
}

/// A tune by its `petlibre_core::tone` name; an unknown name is refused.
fn tune<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Tune>, D::Error> {
    let Some(name) = Option::<String>::deserialize(deserializer)? else {
        return Ok(None);
    };
    Tune::from_name(&name)
        .map(Some)
        .ok_or_else(|| D::Error::custom(format!("unknown tune {name:?}")))
}

/// The tune names for messages: "a, b or c".
pub(crate) fn tune_names() -> String {
    let [rest @ .., last] = Tune::ALL.map(Tune::name);
    format!("{} or {last}", rest.join(", "))
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub op: Op,
    /// `settings` only: the options to change; none reads them.
    #[serde(default)]
    pub lights: Option<bool>,
    #[serde(default)]
    pub sound: Option<bool>,
    #[serde(default)]
    pub volume: Option<u8>,
    /// `wifi` only: credentials to use from the next boot, or `clear` to
    /// return to the build's. Never logged or echoed.
    #[serde(default)]
    pub ssid: Option<String>,
    #[serde(default)]
    pub psk: Option<String>,
    #[serde(default)]
    pub clear: Option<bool>,
    /// `play` only: a built-in tune, by its `petlibre_core::tone` name.
    #[serde(default, deserialize_with = "tune")]
    pub tune: Option<Tune>,
}

impl std::fmt::Debug for Input {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The credentials stay out of every log.
        f.debug_struct("Input")
            .field("op", &self.op)
            .finish_non_exhaustive()
    }
}

impl Input {
    pub fn op(op: Op) -> Self {
        Self {
            op,
            lights: None,
            sound: None,
            volume: None,
            ssid: None,
            psk: None,
            clear: None,
            tune: None,
        }
    }

    /// Setting fields only with `settings`, a volume of at most 100, and a
    /// known tune exactly with `play`, as the firmware's parser requires.
    pub fn valid(&self) -> bool {
        let fields = self.lights.is_some() || self.sound.is_some() || self.volume.is_some();
        let wifi = match (&self.ssid, &self.psk, self.clear) {
            (None, None, None) => true,
            (None, None, Some(true)) => self.op == Op::Wifi,
            (Some(ssid), Some(psk), None) => {
                self.op == Op::Wifi
                    && (1..=32).contains(&ssid.len())
                    && (8..=63).contains(&psk.len())
                    && psk.bytes().all(|b| (0x20..0x7f).contains(&b))
            }
            _ => false,
        };
        let tune = self.tune.is_some() == (self.op == Op::Play);
        (self.op == Op::Settings || !fields) && self.volume.is_none_or(|v| v <= 100) && wifi && tune
    }

    /// The request without its closing brace, fields in the firmware's order.
    pub fn wire(&self, id: Uuid) -> request_auth::Request {
        let flag = |on: bool| i64::from(on);
        let mut request = request_auth::Request::new(id, self.op.name());
        if let Some(on) = self.lights {
            request = request.field("lights", flag(on));
        }
        if let Some(on) = self.sound {
            request = request.field("sound", flag(on));
        }
        if let Some(volume) = self.volume {
            request = request.field("volume", i64::from(volume));
        }
        if let (Some(ssid), Some(psk)) = (&self.ssid, &self.psk) {
            request = request
                .hex_field("ssid", ssid.as_bytes())
                .hex_field("psk", psk.as_bytes());
        }
        if self.clear == Some(true) {
            request = request.field("clear", 1);
        }
        if let Some(tune) = self.tune {
            request = request.name_field("tune", tune.name());
        }
        request
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RequestError {
    /// No firmware role in this server's configuration.
    #[error("no replacement-firmware role is configured")]
    Unconfigured,
    /// No fresh online health: a QoS 0 request would be lost unseen.
    #[error("replacement firmware is offline or its health is stale")]
    Offline,
    /// The firmware session's request sequence is used up; it needs a new
    /// session (a new nonce) before another signed request.
    #[error("request sequence exhausted for this firmware session")]
    Exhausted,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Reply {
    pub received_at: DateTime<Utc>,
    pub message: Value,
}

/// A health object as published. Only what the server acts on is typed; every
/// other field is kept (and reported through the API) as sent.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Health {
    pub schema: String,
    /// The schema's major version.
    pub v: u64,
    pub online: bool,
    /// This session's request nonce, from firmware built with a request key.
    #[serde(
        default,
        deserialize_with = "nonce",
        skip_serializing_if = "Option::is_none"
    )]
    pub nonce: Option<String>,
    /// The firmware's wall clock (FW21); anything but an object is ignored.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub clock: Option<Clock>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Clock {
    /// Present once the clock has been set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unix_ms: Option<u64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A present nonce must be 16 lowercase hex characters; null is refused too.
fn nonce<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    let nonce = String::deserialize(deserializer)?;
    if !request_auth::valid_nonce(&nonce) {
        return Err(D::Error::custom("malformed health nonce"));
    }
    Ok(Some(nonce))
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Snapshot {
    pub health: Option<Health>,
    pub received_at: Option<DateTime<Utc>>,
    pub age_ms: Option<i64>,
    pub online: bool,
    pub stale: bool,
    pub rejected: u64,
    pub replies: Vec<Reply>,
}

#[derive(Default)]
pub struct Tracker {
    health: Option<(Health, DateTime<Utc>)>,
    replies: VecDeque<Reply>,
    rejected: u64,
    time_sent: Option<DateTime<Utc>>,
    /// The nonce last signed under and the last `seq` used with it.
    signed: Option<(String, u32)>,
}

/// The zone's next UTC offset change after `now`, to the second, and the
/// offset after it; `None` within the horizon (a zone without DST).
pub fn next_transition(zone: Tz, now: DateTime<Utc>) -> Option<(DateTime<Utc>, i32)> {
    let horizon = now + TimeDelta::days(TRANSITION_HORIZON_DAYS);
    protocol::next_offset_change(zone, now, horizon)
}

/// The FW21 `time` request for `zone` at `now`, in wire field order.
pub fn time_request(id: Uuid, now: DateTime<Utc>, zone: Tz) -> Request {
    let offset = protocol::offset_seconds(zone, now);
    let (next_change_s, next_offset_s) = match next_transition(zone, now) {
        Some((at, next)) => (at.timestamp(), next),
        None => (0, offset),
    };
    Request::new(id, "time")
        .field("unix_ms", now.timestamp_millis())
        .field("offset_s", offset.into())
        .field("next_change_s", next_change_s)
        .field("next_offset_s", next_offset_s.into())
}

fn object(payload: &[u8], limit: usize) -> Option<Map<String, Value>> {
    if payload.len() > limit {
        return None;
    }
    serde_json::from_slice(payload).ok()
}

/// A health object the server accepts: schema and major version match,
/// `online` is a boolean, and a `nonce`, if any, is 16 lowercase hex
/// characters. Unknown fields are kept and ignored.
pub fn parse_health(payload: &[u8]) -> Option<Health> {
    if payload.len() > MAX_HEALTH_BYTES {
        return None;
    }
    let health: Health = serde_json::from_slice(payload).ok()?;
    (health.schema == HEALTH_SCHEMA && health.v == HEALTH_VERSION).then_some(health)
}

/// A reply the server keeps: an object whose `id` is a UUID. The rest is
/// reported through the API unread.
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

    /// Whether the firmware's clock wants a sync: only firmware whose health
    /// reports a `clock` (FW21), never sooner than `TIME_RETRY` after the last
    /// sync, and then when unsynced or `TIME_RESYNC` after the last one.
    pub fn time_due(&self, now: DateTime<Utc>) -> bool {
        let Some((health, _)) = &self.health else {
            return false;
        };
        let Some(clock) = &health.clock else {
            return false;
        };
        if !health.online {
            return false;
        }
        let since = self.time_sent.map(|at| now - at);
        if since.is_some_and(|since| (TimeDelta::zero()..TIME_RETRY).contains(&since)) {
            return false;
        }
        clock.unix_ms.is_none() || since.is_none_or(|since| since >= TIME_RESYNC)
    }

    /// The latest health's request nonce; `None` for firmware without a key.
    pub fn nonce(&self) -> Option<&str> {
        self.health.as_ref()?.0.nonce.as_deref()
    }

    /// The payload for `request`: signed with the next `seq` under the
    /// latest health's nonce when the server has a key and the firmware
    /// reports a nonce, else unsigned as before request authentication.
    /// `seq` restarts at 1 for each new nonce; once it would pass `u32::MAX`
    /// the request is refused rather than wrapped.
    pub fn finish(
        &mut self,
        request: Request,
        key: Option<&RequestKey>,
    ) -> Result<String, RequestError> {
        let (Some(key), Some(nonce)) = (key, self.nonce()) else {
            return Ok(request.unsigned());
        };
        let nonce = nonce.to_owned();
        let seq = match &self.signed {
            Some((last, seq)) if *last == nonce => {
                seq.checked_add(1).ok_or(RequestError::Exhausted)?
            }
            _ => 1,
        };
        let payload = request.signed(key, &nonce, seq);
        self.signed = Some((nonce, seq));
        Ok(payload)
    }

    pub fn time_sent(&mut self, now: DateTime<Utc>) {
        self.time_sent = Some(now);
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
        let online = !stale && health.as_ref().is_some_and(|health| health.online);

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

    #[test]
    fn wifi_inputs_are_checked_and_sent_as_hex() {
        let parse = |text: &str| {
            serde_json::from_str::<Input>(text)
                .ok()
                .filter(Input::valid)
        };
        let set = parse(r#"{"op":"wifi","ssid":"Lab \"2\"","psk":"pass word!"}"#).unwrap();
        // `Lab "2"` and `pass word!` as hex: no escaping reaches the firmware.
        assert_eq!(
            set.wire(Uuid::nil()).unsigned(),
            format!(
                r#"{{"id":"{}","op":"wifi","ssid":"4c616220223222","psk":"7061737320776f726421"}}"#,
                Uuid::nil()
            )
        );
        assert!(
            !format!("{set:?}").contains("pass"),
            "no credentials in Debug"
        );
        let clear = parse(r#"{"op":"wifi","clear":true}"#).unwrap();
        assert!(
            clear
                .wire(Uuid::nil())
                .unsigned()
                .ends_with(r#","clear":1}"#)
        );
        for bad in [
            r#"{"op":"wifi","ssid":"x"}"#,
            r#"{"op":"wifi","ssid":"x","psk":"short"}"#,
            r#"{"op":"wifi","ssid":"","psk":"password"}"#,
            r#"{"op":"wifi","ssid":"x","psk":"pass\tword"}"#,
            r#"{"op":"wifi","clear":false}"#,
            r#"{"op":"wifi","clear":true,"ssid":"x","psk":"password"}"#,
            r#"{"op":"ping","clear":true}"#,
        ] {
            assert!(parse(bad).is_none(), "{bad}");
        }

        // The largest signed request fits the firmware's limit.
        let big = parse(&format!(
            r#"{{"op":"wifi","ssid":"{}","psk":"{}"}}"#,
            "s".repeat(32),
            "p".repeat(63)
        ))
        .unwrap();
        let key = request_auth::RequestKey::parse(&"ab".repeat(32)).unwrap();
        let signed = big
            .wire(Uuid::max())
            .signed(&key, "0123456789abcdef", u32::MAX);
        assert!(
            signed.len() <= request_auth::MAX_REQUEST,
            "{}",
            signed.len()
        );
    }

    #[test]
    fn play_inputs_name_a_known_tune() {
        let parse = |text: &str| {
            serde_json::from_str::<Input>(text)
                .ok()
                .filter(Input::valid)
        };
        let play = parse(r#"{"op":"play","tune":"twinkle"}"#).unwrap();
        assert_eq!(
            play.wire(Uuid::nil()).unsigned(),
            format!(r#"{{"id":"{}","op":"play","tune":"twinkle"}}"#, Uuid::nil())
        );
        for bad in [
            r#"{"op":"play"}"#,
            r#"{"op":"play","tune":"siren"}"#,
            r#"{"op":"play","tune":"coin\"}"#,
            r#"{"op":"ping","tune":"coin"}"#,
        ] {
            assert!(parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn settings_inputs_are_checked_and_written_in_firmware_order() {
        let parse = |text: &str| {
            serde_json::from_str::<Input>(text)
                .ok()
                .filter(Input::valid)
        };
        let read = parse(r#"{"op":"settings"}"#).unwrap();
        assert_eq!(
            read.wire(Uuid::nil()).unsigned(),
            format!(r#"{{"id":"{}","op":"settings"}}"#, Uuid::nil())
        );

        let change = parse(r#"{"op":"settings","volume":20,"lights":false}"#).unwrap();
        assert_eq!(
            change.wire(Uuid::nil()).unsigned(),
            format!(
                r#"{{"id":"{}","op":"settings","lights":0,"volume":20}}"#,
                Uuid::nil()
            )
        );
        for bad in [
            r#"{"op":"settings","volume":101}"#,
            r#"{"op":"ping","lights":true}"#,
            r#"{"op":"settings","lights":1}"#,
            r#"{"op":"settings","bowl":1}"#,
        ] {
            assert!(parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn the_time_request_carries_the_zone_and_its_next_change() {
        let now = DateTime::parse_from_rfc3339("2026-10-22T13:00:00Z")
            .unwrap()
            .to_utc();
        let id = Uuid::nil();
        let value: Value =
            serde_json::from_str(&time_request(id, now, chrono_tz::America::Denver).unsigned())
                .unwrap();
        assert_eq!(
            value,
            json!({"id": id, "op": "time", "unix_ms": 1_792_674_000_000_i64, "offset_s": -21_600,
                   "next_change_s": 1_793_520_000, "next_offset_s": -25_200})
        );
        // 2026-11-01 02:00 MDT is 08:00 UTC.
        assert_eq!(
            DateTime::from_timestamp(1_793_520_000, 0)
                .unwrap()
                .to_rfc3339(),
            "2026-11-01T08:00:00+00:00"
        );
        let utc: Value =
            serde_json::from_str(&time_request(id, now, chrono_tz::UTC).unsigned()).unwrap();
        assert_eq!(
            (utc["offset_s"].clone(), utc["next_change_s"].clone()),
            (json!(0), json!(0))
        );
        assert!(serde_json::to_vec(&value).unwrap().len() <= request_auth::MAX_REQUEST);
    }

    #[test]
    fn the_largest_signed_time_request_fits_and_keeps_its_field_order() {
        let now = DateTime::parse_from_rfc3339("2026-10-22T13:00:00Z")
            .unwrap()
            .to_utc();
        let key = RequestKey::parse(&"5a".repeat(32)).unwrap();
        // Widest offsets in use (-12 h / +14 h), the widest seq.
        for zone in [
            chrono_tz::America::Denver,
            chrono_tz::Etc::GMTPlus12,
            chrono_tz::Pacific::Kiritimati,
        ] {
            let payload =
                time_request(Uuid::max(), now, zone).signed(&key, "ffffffffffffffff", u32::MAX);
            assert!(
                payload.len() <= request_auth::MAX_REQUEST,
                "{} bytes",
                payload.len()
            );
            assert!(request_auth::verify(&key, &payload));
        }

        let payload = time_request(Uuid::nil(), now, chrono_tz::America::Denver).signed(
            &key,
            "0123456789abcdef",
            1,
        );
        let (prefix, _) = request_auth::split_signed(&payload).unwrap();
        assert_eq!(
            prefix,
            r#"{"id":"00000000-0000-0000-0000-000000000000","op":"time","unix_ms":1792674000000,"offset_s":-21600,"next_change_s":1793520000,"next_offset_s":-25200,"nonce":"0123456789abcdef","seq":1"#
        );
    }

    #[test]
    fn requests_are_signed_only_with_a_key_and_a_nonce_and_seq_restarts_per_nonce() {
        let now = Utc::now();
        let key = RequestKey::parse(&"5a".repeat(32)).unwrap();
        let ping = || Request::new(Uuid::nil(), Op::Ping.name());
        let seq = |payload: &str| serde_json::from_str::<Value>(payload).unwrap()["seq"].clone();
        let mut tracker = Tracker::default();

        // No health yet, or health from firmware without a key: unsigned.
        assert_eq!(
            tracker.finish(ping(), Some(&key)).unwrap(),
            ping().unsigned()
        );
        tracker.observe_health(&health(json!({})), now);
        assert_eq!(
            tracker.finish(ping(), Some(&key)).unwrap(),
            ping().unsigned()
        );

        // A nonce but no server key: unsigned, exactly as today.
        tracker.observe_health(&health(json!({"nonce":"0123456789abcdef"})), now);
        assert_eq!(tracker.finish(ping(), None).unwrap(), ping().unsigned());

        let first = tracker.finish(ping(), Some(&key)).unwrap();
        assert!(request_auth::verify(&key, &first));
        assert_eq!(seq(&first), 1);
        assert_eq!(seq(&tracker.finish(ping(), Some(&key)).unwrap()), 2);
        // Repeated health with the same nonce keeps counting.
        tracker.observe_health(&health(json!({"nonce":"0123456789abcdef","seq":2})), now);
        assert_eq!(seq(&tracker.finish(ping(), Some(&key)).unwrap()), 3);

        // A new session's nonce restarts at 1.
        tracker.observe_health(&health(json!({"nonce":"fedcba9876543210"})), now);
        let next = tracker.finish(ping(), Some(&key)).unwrap();
        assert_eq!(seq(&next), 1);
        assert!(next.contains(r#""nonce":"fedcba9876543210""#));

        // The sequence never wraps.
        tracker.signed = Some(("fedcba9876543210".to_owned(), u32::MAX));
        assert_eq!(
            tracker.finish(ping(), Some(&key)),
            Err(RequestError::Exhausted)
        );
    }

    #[test]
    fn a_malformed_health_nonce_is_refused() {
        assert!(parse_health(&health(json!({"nonce":"0123456789abcdef"}))).is_some());
        for bad in [
            json!("0123456789ABCDEF"),
            json!("0123"),
            json!(7),
            json!(null),
        ] {
            assert!(
                parse_health(&health(json!({"nonce": bad}))).is_none(),
                "{bad}"
            );
        }
    }

    #[test]
    fn time_is_sent_only_to_clock_firmware_when_due() {
        let now = DateTime::parse_from_rfc3339("2026-10-22T13:00:00Z")
            .unwrap()
            .to_utc();
        let mut tracker = Tracker::default();
        assert!(!tracker.time_due(now));

        tracker.observe_health(&health(json!({})), now);
        assert!(
            !tracker.time_due(now),
            "pre-FW21 firmware is never sent time"
        );

        tracker.observe_health(&health(json!({"clock": {"syncs": 0}})), now);
        assert!(tracker.time_due(now));
        tracker.time_sent(now);
        assert!(!tracker.time_due(now + TimeDelta::seconds(30)));
        assert!(tracker.time_due(now + TIME_RETRY), "still unsynced: retry");

        tracker.observe_health(&health(json!({"clock": {"syncs": 1, "unix_ms": 1}})), now);
        assert!(!tracker.time_due(now + TIME_RETRY));
        assert!(tracker.time_due(now + TIME_RESYNC));

        let mut offline = Tracker::default();
        offline.observe_health(
            &health(json!({"online": false, "clock": {"syncs": 0}})),
            now,
        );
        assert!(!offline.time_due(now));
    }

    fn health(extra: Value) -> Vec<u8> {
        let mut value = json!({"schema":"petlibre.health","v":1,"online":true,"seq":1});
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
