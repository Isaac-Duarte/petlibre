//! Bounded observations of stock OTA reports. This module sends no commands.
use chrono::{DateTime, Utc};
pub use wellfed_formats::stock::{OtaCommand, OtaReport};

use crate::protocol::Message;

pub const REPORT_LIMIT: usize = 16;

fn token(value: &str, limit: usize) -> Option<String> {
    (!value.is_empty()
        && value.len() <= limit
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')))
    .then(|| value.to_owned())
}

/// The report a stock message carries, if it is an OTA report.
pub fn from_message(message: &Message, now: DateTime<Utc>) -> Option<OtaReport> {
    let command = OtaCommand::from_cmd(&message.cmd)?;

    let report = &message.report;
    Some(OtaReport {
        received_at: now,
        command,
        message_id: message.msg_id.as_deref().and_then(|id| token(id, 64)),
        phase: report.phase.as_deref().and_then(|phase| token(phase, 32)),
        state: report.state,
        progress: report.progress.filter(|value| *value <= 100),
        code: report.code,
        error_code: report.err_code.and_then(|n| u32::try_from(n).ok()),
    })
}
