//! Bounded observations of stock OTA reports. This module sends no commands.
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::protocol::Message;

pub const REPORT_LIMIT: usize = 16;

#[derive(Clone, Debug, Serialize)]
pub struct OtaReport {
    pub received_at: DateTime<Utc>,
    pub command: String,
    pub message_id: Option<String>,
    pub phase: Option<String>,
    pub state: Option<u8>,
    pub progress: Option<u8>,
    pub code: Option<i64>,
    pub error_code: Option<u32>,
}

fn token(value: &str, limit: usize) -> Option<String> {
    (!value.is_empty()
        && value.len() <= limit
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')))
    .then(|| value.to_owned())
}

impl OtaReport {
    pub fn from_message(message: &Message, now: DateTime<Utc>) -> Option<Self> {
        if !matches!(
            message.cmd.as_str(),
            "OTA_UPGRADE" | "OTA_PROGRESS" | "OTA_INFORM"
        ) {
            return None;
        }

        let report = &message.report;
        Some(Self {
            received_at: now,
            command: message.cmd.clone(),
            message_id: message.msg_id.as_deref().and_then(|id| token(id, 64)),
            phase: report.phase.as_deref().and_then(|phase| token(phase, 32)),
            state: report.state,
            progress: report.progress.filter(|value| *value <= 100),
            code: report.code,
            error_code: report.err_code.and_then(|n| u32::try_from(n).ok()),
        })
    }
}
