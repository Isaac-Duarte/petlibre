//! Explicit, non-replayed configuration queries for the pinned C experiment.
//!
//! On a confirmed C-probe (1.1.17) installation, one `GET_CONFIG` query at a
//! time proves the diagnostic ran: the reply must carry `localReProbe`, an
//! FNV-1a value of the device's own reply ID, within `QUERY_SECONDS`. A query
//! without proof by then becomes uncertain and is never resent.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    config::Config,
    protocol::{self, Message, Reply},
};

pub const QUERY_SECONDS: i64 = 60;
const CLOCK_SKEW_MILLIS: i64 = 10_000;

pub fn command(config: &Config, id: Uuid, now: DateTime<Utc>) -> Reply {
    let mut command = protocol::service(config, "GET_CONFIG", id, now);
    command.topic = config.topic("config/sub");
    command
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub request_id: Uuid,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Submitted,
    ProofReported,
    Uncertain,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Proof {
    pub message_id: String,
    pub value: String,
    pub received_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Record {
    pub request_id: Uuid,
    pub trial_request_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub phase: Phase,
    pub proof: Option<Proof>,
}

/// FNV-1a matches the compiled diagnostic. This is not an authentication hash.
pub fn expected(message_id: &str) -> Option<String> {
    if message_id.is_empty()
        || message_id.len() >= 48
        || !message_id.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return None;
    }

    let mut value = 2_166_136_261_u32;
    for byte in message_id.bytes() {
        value = (value ^ u32::from(byte)).wrapping_mul(16_777_619);
    }

    Some(format!("c1-{value:08x}"))
}

impl Record {
    /// Stock GET_CONFIG generates its own reply ID rather than echoing the
    /// request ID. Only one explicit query is outstanding per installation.
    pub fn report(&self, channel: &str, message: &Message, now: DateTime<Utc>) -> Option<Proof> {
        if self.phase != Phase::Submitted
            || channel != "config"
            || message.cmd != "GET_CONFIG"
            || !(0..QUERY_SECONDS).contains(&(now - self.created_at).num_seconds())
            || !message.ts.is_some_and(|ts| {
                // The stock clock has reported a roughly two-second offset.
                // Use a bounded skew allowance; receiver time and unique IDs
                // still gate the single outstanding query.
                ts >= self
                    .created_at
                    .timestamp_millis()
                    .saturating_sub(CLOCK_SKEW_MILLIS)
                    && ts <= now.timestamp_millis().saturating_add(CLOCK_SKEW_MILLIS)
            })
            || message
                .fields
                .get("softwareVersion")
                .and_then(serde_json::Value::as_str)
                != Some("1.1.17")
        {
            return None;
        }

        let message_id = message.msg_id.as_deref()?;
        let value = expected(message_id)?;

        (message
            .fields
            .get("localReProbe")
            .and_then(serde_json::Value::as_str)
            == Some(value.as_str()))
        .then(|| Proof {
            message_id: message_id.to_owned(),
            value,
            received_at: now,
        })
    }
}
