//! The stock PLAF103 device protocol: topic parsing and the server's replies.
//!
//! The device posts JSON on `dl/PLAF103/<device_id>/device/<channel>/post`
//! and the server answers on the matching `/sub` topic. Only captured commands
//! get a reply (clock, binding, the imported feeding-plan snapshot and event
//! acknowledgements); schedules are served from the snapshot, never edited.

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Offset, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::config::Config;

#[derive(Debug, Deserialize)]
pub struct Message {
    pub cmd: String,
    #[serde(rename = "msgId")]
    pub msg_id: Option<String>,
    pub ts: Option<i64>,
    #[serde(flatten)]
    pub fields: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Reply {
    pub topic: String,
    pub payload: Value,
}

pub fn parse(config: &Config, topic: &str, payload: &[u8]) -> Result<(String, Message)> {
    ensure!(payload.len() <= 65_536, "payload too large");
    let prefix = config.topic("");
    let suffix = topic
        .strip_prefix(&prefix)
        .context("unexpected device topic")?;
    let channel = suffix
        .strip_suffix("/post")
        .context("expected device post topic")?;
    ensure!(
        matches!(
            channel,
            "system" | "ntp" | "event" | "service" | "heart" | "ota" | "config"
        ),
        "unknown topic channel"
    );

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

    Ok((channel.to_owned(), message))
}

pub fn reply(config: &Config, channel: &str, msg: &Message, now: DateTime<Utc>) -> Option<Reply> {
    let mut payload = json!({"cmd":msg.cmd, "ts":now.timestamp_millis(), "code":0, "linkId":Uuid::new_v4().simple().to_string()});
    if let Some(id) = &msg.msg_id {
        payload["msgId"] = json!(id);
    }

    match (channel, msg.cmd.as_str()) {
        ("ntp", "NTP") => {
            // Only calibrate when there are no imported schedules. Large clock
            // jumps with plans require a separately validated skipped-feed policy.
            let calibration = config.plans.is_empty();
            payload = ntp_response(config.timezone, now, calibration);
        }
        ("system", "BINDING") => {
            payload = config.binding_response.clone()?;
            payload["ts"] = json!(now.timestamp_millis());
            payload["msgId"] = json!(msg.msg_id);
            payload["linkId"] = json!(Uuid::new_v4().simple().to_string());
        }
        ("event", "GET_FEEDING_PLAN_EVENT") => {
            payload["plans"] = json!(config.plans);
        }
        ("event", "GRAIN_OUTPUT_EVENT") => {
            if let Some(step) = msg.fields.get("execStep") {
                payload["execStep"] = step.clone();
            }
        }
        (
            "event",
            "DEVICE_START_EVENT" | "ATTR_PUSH_EVENT" | "DEVICE_LOG_REPORT_EVENT" | "ERROR_EVENT",
        ) => {}
        _ => return None,
    }

    Some(Reply {
        topic: config.topic(&format!("{channel}/sub")),
        payload,
    })
}

pub fn ntp_response(zone: Tz, now: DateTime<Utc>, calibrate: bool) -> Value {
    let current = now.with_timezone(&zone).offset().fix().local_minus_utc();
    let offset_at = |seconds| {
        zone.timestamp_opt(seconds, 0)
            .single()
            .expect("bounded timestamp")
            .offset()
            .fix()
            .local_minus_utc()
    };

    let mut transitions = Vec::new();
    let mut previous = now.timestamp();
    let mut previous_offset = current;

    // Scan UTC days, then find the exact transition second; no ambiguous local times.
    for day in 1..=800 {
        let end = now.timestamp() + day * 86_400;
        let offset = offset_at(end);
        if offset != previous_offset {
            let (mut low, mut high) = (previous, end);
            while high - low > 1 {
                let middle = low + (high - low) / 2;
                if offset_at(middle) == previous_offset {
                    low = middle;
                } else {
                    high = middle;
                }
            }
            transitions.push((offset, high * 1000));
            if transitions.len() == 2 {
                break;
            }
        }
        previous = end;
        previous_offset = offset;
    }

    let (next_offset, next_ts) = transitions.first().copied().unwrap_or((current, 0));
    let (second_offset, second_ts) = transitions.get(1).copied().unwrap_or((current, 0));

    json!({"cmd":"NTP", "ts":now.timestamp_millis(), "code":0, "calibrationTag":calibrate,
        "timezone":f64::from(current)/3600.0, "timezoneOffsetSeconds":current,
        "nextDSTOffsetSeconds":next_offset, "nextDSTTransitionTs":next_ts,
        "secondNextDSTOffsetSeconds":second_offset, "secondNextDSTTransitionTs":second_ts,
        "linkId":Uuid::new_v4().simple().to_string()})
}

pub fn service(config: &Config, command: &str, id: Uuid, now: DateTime<Utc>) -> Reply {
    Reply {
        topic: config.topic("service/sub"),
        payload: json!({"cmd":command,"msgId":id.to_string(),"ts":now.timestamp_millis()}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denver_time_response_matches_captured_dst_transitions() {
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 21, 17, 41).unwrap();
        let response = ntp_response(chrono_tz::America::Denver, now, true);

        assert_eq!(response["timezoneOffsetSeconds"], -21600);
        assert_eq!(response["nextDSTOffsetSeconds"], -25200);
        assert_eq!(response["nextDSTTransitionTs"], 1793520000000_i64);
        assert_eq!(response["secondNextDSTOffsetSeconds"], -21600);
        assert_eq!(response["secondNextDSTTransitionTs"], 1805014800000_i64);
    }

    #[test]
    fn zones_without_dst_do_not_invent_transition_dates() {
        let response = ntp_response(chrono_tz::UTC, Utc::now(), false);

        assert_eq!(response["nextDSTTransitionTs"], 0);
        assert_eq!(response["calibrationTag"], false);
    }
}
