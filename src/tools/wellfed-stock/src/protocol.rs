//! The stock PLAF103 device protocol: topic parsing and the server's replies.
//!
//! The device posts JSON on `dl/PLAF103/<device_id>/device/<channel>/post`
//! and the server answers on the matching `/sub` topic. Only captured commands
//! get a reply (clock, binding, the imported feeding-plan snapshot and event
//! acknowledgements); schedules are served from the snapshot, never edited.
//!
//! Replies are typed structs, but go out through a `Value` so their keys are
//! sorted exactly as the captured server's were (`tests/wire.rs` pins them).

use std::fmt;

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Offset, TimeDelta, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::{clock::Clock, config::Config};

/// The topic channels the stock firmware posts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    System,
    Ntp,
    Event,
    Service,
    Heart,
    Ota,
    Config,
}

impl Channel {
    const ALL: [Self; 7] = [
        Self::System,
        Self::Ntp,
        Self::Event,
        Self::Service,
        Self::Heart,
        Self::Ota,
        Self::Config,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Ntp => "ntp",
            Self::Event => "event",
            Self::Service => "service",
            Self::Heart => "heart",
            Self::Ota => "ota",
            Self::Config => "config",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|channel| channel.name() == name)
    }

    /// Where the server answers or commands the device on this channel.
    pub fn sub_topic(self, config: &Config) -> String {
        config.topic(&format!("{self}/sub"))
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A field read as `T`, or `None` when it is of another type: one odd field
/// must not drop a whole stock message (or capture record).
pub(crate) fn lenient<'de, D, T>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let value = Value::deserialize(deserializer)?;
    Ok(T::deserialize(value).ok())
}

/// The fields of a device message the server acts on, each read leniently.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    #[serde(default, deserialize_with = "lenient")]
    pub software_version: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub power_type: Option<u64>,
    #[serde(default, deserialize_with = "lenient")]
    pub code: Option<i64>,
    #[serde(default, deserialize_with = "lenient")]
    pub err_code: Option<u64>,
    #[serde(default, deserialize_with = "lenient")]
    pub phase: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub state: Option<u8>,
    #[serde(default, deserialize_with = "lenient")]
    pub progress: Option<u8>,
}

#[derive(Deserialize)]
struct Envelope {
    cmd: String,
    #[serde(rename = "msgId")]
    msg_id: Option<String>,
    ts: Option<i64>,
    #[serde(flatten)]
    fields: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
#[serde(from = "Envelope")]
pub struct Message {
    pub cmd: String,
    pub msg_id: Option<String>,
    pub ts: Option<i64>,
    pub report: Report,
    /// Every other field as sent, for the telemetry allowlist and echoes.
    pub fields: Map<String, Value>,
}

impl From<Envelope> for Message {
    fn from(envelope: Envelope) -> Self {
        // Every field is lenient, so only a non-map could fail, and this is one.
        let report = Report::deserialize(&envelope.fields).unwrap_or_default();

        Self {
            cmd: envelope.cmd,
            msg_id: envelope.msg_id,
            ts: envelope.ts,
            report,
            fields: envelope.fields,
        }
    }
}

impl Message {
    /// The message ID as a request UUID, for correlating with the ledger.
    pub fn request_id(&self) -> Option<Uuid> {
        Uuid::parse_str(self.msg_id.as_deref()?).ok()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Reply {
    pub topic: String,
    pub payload: Value,
}

impl Reply {
    fn new(topic: String, payload: &impl Serialize) -> Self {
        let payload = serde_json::to_value(payload)
            .expect("reply structs have string keys and infallible fields");
        Self { topic, payload }
    }
}

pub fn parse(config: &Config, topic: &str, payload: &[u8]) -> Result<(Channel, Message)> {
    ensure!(payload.len() <= 65_536, "payload too large");
    let prefix = config.topic("");
    let suffix = topic
        .strip_prefix(&prefix)
        .context("unexpected device topic")?;
    let channel = suffix
        .strip_suffix("/post")
        .context("expected device post topic")?;
    let channel = Channel::from_name(channel).context("unknown topic channel")?;

    let message: Message = serde_json::from_slice(payload).context("invalid device JSON")?;
    ensure!(
        !message.cmd.is_empty()
            && message.cmd.len() <= 64
            && message
                .cmd
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'),
        "invalid command name"
    );

    Ok((channel, message))
}

/// The server's acknowledgement of a device event.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Ack<'a> {
    cmd: &'a str,
    code: u8,
    /// Echoed exactly as the device sent it.
    #[serde(skip_serializing_if = "Option::is_none")]
    exec_step: Option<&'a Value>,
    link_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    msg_id: Option<&'a str>,
    /// The imported snapshot, replayed verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    plans: Option<&'a [Value]>,
    ts: i64,
}

fn link_id() -> String {
    Uuid::new_v4().simple().to_string()
}

/// The reply to `msg`, if it gets one. `clock` holds stock's latest clock
/// reading (with `msg`'s own already observed) and the owner's policy, which
/// decide whether an `NTP` reply sets stock's clock ([`crate::clock`]).
pub fn reply(
    config: &Config,
    channel: Channel,
    msg: &Message,
    now: DateTime<Utc>,
    clock: &Clock,
) -> Option<Reply> {
    let mut ack = Ack {
        cmd: &msg.cmd,
        code: 0,
        exec_step: None,
        link_id: link_id(),
        msg_id: msg.msg_id.as_deref(),
        plans: None,
        ts: now.timestamp_millis(),
    };
    let topic = channel.sub_topic(config);

    match (channel, msg.cmd.as_str()) {
        (Channel::Ntp, "NTP") => {
            let calibration = clock.calibration(config.plans.as_deref(), now);
            let ntp = ntp_response(config.timezone, now, calibration.tag());
            return Some(Reply::new(topic, &ntp));
        }

        (Channel::System, "BINDING") => {
            // The captured cloud reply, replayed with this exchange's IDs.
            let mut payload = config.binding_response.clone()?;
            payload.insert("ts".into(), now.timestamp_millis().into());
            payload.insert("msgId".into(), msg.msg_id.clone().into());
            payload.insert("linkId".into(), link_id().into());
            return Some(Reply {
                topic,
                payload: Value::Object(payload),
            });
        }

        // Stock erases and rewrites its plan table on any reply with a
        // `plans` key (and takes one unasked), so only a captured table is
        // sent. With none, the request goes unanswered and stock keeps its
        // own; it asks once, without retrying (static analysis of 1.1.15).
        (Channel::Event, "GET_FEEDING_PLAN_EVENT") => ack.plans = Some(config.plans.as_deref()?),
        (Channel::Event, "GRAIN_OUTPUT_EVENT") => ack.exec_step = msg.fields.get("execStep"),
        (
            Channel::Event,
            "DEVICE_START_EVENT" | "ATTR_PUSH_EVENT" | "DEVICE_LOG_REPORT_EVENT" | "ERROR_EVENT",
        ) => {}
        _ => return None,
    }

    Some(Reply::new(topic, &ack))
}

/// The stock `NTP` reply: the zone's current offset and its next two changes.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ntp {
    pub calibration_tag: bool,
    pub cmd: &'static str,
    pub code: u8,
    pub link_id: String,
    #[serde(rename = "nextDSTOffsetSeconds")]
    pub next_dst_offset_seconds: i32,
    /// Milliseconds; 0 when the zone has no change ahead.
    #[serde(rename = "nextDSTTransitionTs")]
    pub next_dst_transition_ts: i64,
    #[serde(rename = "secondNextDSTOffsetSeconds")]
    pub second_next_dst_offset_seconds: i32,
    #[serde(rename = "secondNextDSTTransitionTs")]
    pub second_next_dst_transition_ts: i64,
    /// Hours.
    pub timezone: f64,
    pub timezone_offset_seconds: i32,
    pub ts: i64,
}

/// How far ahead the stock reply looks for the zone's next two changes.
const NTP_HORIZON_DAYS: i64 = 800;

pub fn ntp_response(zone: Tz, now: DateTime<Utc>, calibrate: bool) -> Ntp {
    let current = offset_seconds(zone, now);
    let horizon = now + TimeDelta::days(NTP_HORIZON_DAYS);

    let first = next_offset_change(zone, now, horizon);
    let second = first.and_then(|(at, _)| next_offset_change(zone, at, horizon));
    let wire = |change: Option<(DateTime<Utc>, i32)>| match change {
        Some((at, offset)) => (offset, at.timestamp_millis()),
        None => (current, 0),
    };
    let (next_offset, next_ts) = wire(first);
    let (second_offset, second_ts) = wire(second);

    Ntp {
        calibration_tag: calibrate,
        cmd: "NTP",
        code: 0,
        link_id: link_id(),
        next_dst_offset_seconds: next_offset,
        next_dst_transition_ts: next_ts,
        second_next_dst_offset_seconds: second_offset,
        second_next_dst_transition_ts: second_ts,
        timezone: f64::from(current) / 3600.0,
        timezone_offset_seconds: current,
        ts: now.timestamp_millis(),
    }
}

/// The zone's UTC offset at `at`, in seconds.
pub(crate) fn offset_seconds(zone: Tz, at: DateTime<Utc>) -> i32 {
    zone.offset_from_utc_datetime(&at.naive_utc())
        .fix()
        .local_minus_utc()
}

/// The first whole second after `after`, up to `until`, with a different UTC
/// offset, and that offset. UTC days are scanned, then the exact second is
/// found by bisection, so no ambiguous local time is involved.
pub(crate) fn next_offset_change(
    zone: Tz,
    after: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Option<(DateTime<Utc>, i32)> {
    let offset_at =
        |seconds: i64| DateTime::from_timestamp(seconds, 0).map(|at| offset_seconds(zone, at));
    let current = offset_at(after.timestamp())?;

    let mut low = after.timestamp();
    let mut high = None;
    while low < until.timestamp() {
        let end = (low + 86_400).min(until.timestamp());
        if offset_at(end)? != current {
            high = Some(end);
            break;
        }
        low = end;
    }

    let mut high = high?;
    while high - low > 1 {
        let middle = low + (high - low) / 2;
        if offset_at(middle)? == current {
            low = middle;
        } else {
            high = middle;
        }
    }

    let at = DateTime::from_timestamp(high, 0)?;
    Some((at, offset_seconds(zone, at)))
}

/// A command carrying only the stock envelope fields.
#[derive(Serialize)]
pub(crate) struct Plain {}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Command<'a, T> {
    cmd: &'a str,
    msg_id: String,
    ts: i64,
    #[serde(flatten)]
    body: &'a T,
}

/// Every command the server sends unasked. A closed set with no plan
/// message: stock acts on `GET_FEEDING_PLAN_EVENT` or `FEEDING_PLAN_SERVICE`
/// whether or not it asked, erasing its stored plans, so the server never
/// sends either on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sent {
    /// `ATTR_GET_SERVICE`: stock reports its attributes.
    Attributes,
    /// `OTA_UPGRADE`: the install trial's one request.
    Upgrade,
}

impl Sent {
    pub const fn cmd(self) -> &'static str {
        match self {
            Self::Attributes => "ATTR_GET_SERVICE",
            Self::Upgrade => "OTA_UPGRADE",
        }
    }
}

/// The server command `sent` with request `id` on `channel`; `body` adds its
/// fields beside the envelope's.
pub fn command(
    config: &Config,
    channel: Channel,
    sent: Sent,
    id: Uuid,
    now: DateTime<Utc>,
    body: &impl Serialize,
) -> Reply {
    let command = Command {
        cmd: sent.cmd(),
        msg_id: id.to_string(),
        ts: now.timestamp_millis(),
        body,
    };
    Reply::new(channel.sub_topic(config), &command)
}

/// A `service` command without fields of its own.
pub fn service(config: &Config, sent: Sent, id: Uuid, now: DateTime<Utc>) -> Reply {
    command(config, Channel::Service, sent, id, now, &Plain {})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denver_time_response_matches_captured_dst_transitions() {
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 21, 17, 41).unwrap();
        let response = ntp_response(chrono_tz::America::Denver, now, true);

        assert_eq!(response.timezone_offset_seconds, -21600);
        assert_eq!(response.next_dst_offset_seconds, -25200);
        assert_eq!(response.next_dst_transition_ts, 1793520000000_i64);
        assert_eq!(response.second_next_dst_offset_seconds, -21600);
        assert_eq!(response.second_next_dst_transition_ts, 1805014800000_i64);
    }

    #[test]
    fn zones_without_dst_do_not_invent_transition_dates() {
        let response = ntp_response(chrono_tz::UTC, Utc::now(), false);

        assert_eq!(response.next_dst_transition_ts, 0);
        assert!(!response.calibration_tag);
    }

    #[test]
    fn odd_fields_are_skipped_without_dropping_the_message() {
        let message: Message = serde_json::from_value(serde_json::json!({
            "cmd": "OTA_PROGRESS",
            "softwareVersion": 7,
            "progress": "half",
            "powerType": 1,
            "state": 300,
        }))
        .unwrap();

        assert_eq!(message.report.software_version, None);
        assert_eq!(message.report.progress, None);
        assert_eq!(message.report.state, None);
        assert_eq!(message.report.power_type, Some(1));
        assert_eq!(message.fields["softwareVersion"], 7);
    }
}
