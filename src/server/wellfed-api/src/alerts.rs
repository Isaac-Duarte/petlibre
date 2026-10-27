//! `/alerts`: what the server noticed about a feeder that someone should
//! look at. The server decides them from what it holds (the feeders'
//! histories, links, food readings, installs and status), records each, and
//! sends every change on the stream as an `alert` update. Everyone reads
//! them; an owner acknowledges one.
//!
//! An alert whose condition can end (a feeder back online, a hopper read
//! full again) clears itself when it does, acknowledged or not, and the
//! next occurrence is a new alert. A feed's alert names one feed, whose
//! result never changes, so it stays until an owner acknowledges it.

use jiff::Timestamp;
use wellfed_model::{Blocked, FeedOutcome, FeedReason};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{AlertId, FeederId, InstallFailure, InstallId, UserId};

/// How long a feeder may be without a session before it is an alert.
pub const OFFLINE_AFTER_S: u32 = 10 * 60;
/// How long a feeder's clock may be invalid before it is an alert.
pub const CLOCK_INVALID_AFTER_S: u32 = 3600;
/// How long a feeder may be unable to feed before it is an alert.
pub const BLOCKED_AFTER_S: u32 = 30 * 60;

/// What an alert is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AlertCondition {
    /// The server has had no session with the feeder for more than 10
    /// minutes.
    Offline,
    /// A feed ended `uncertain`, `failed` or `expired`, as the feeder's
    /// history says.
    Feed {
        epoch: u32,
        feed: u32,
        outcome: FeedOutcome,
        reason: FeedReason,
        requested: u8,
        completed: u8,
    },
    /// A feed sent through the server that the feeder's history has no
    /// record of after a restart: it never ran, or its record was lost.
    /// It clears if the history later shows the feed.
    Unrecorded {
        /// The journal entry's key (`GET /commands/{key}`).
        #[schema(value_type = String, format = Uuid)]
        command: Uuid,
    },
    /// The food beam last read an empty hopper. It clears when a later
    /// reading finds food, or an owner records a refill.
    HopperEmpty,
    /// The feeder's last install failed. It clears when another install
    /// starts on it.
    InstallFailed {
        install: InstallId,
        failure: Option<InstallFailure>,
    },
    /// The feeder's clock has been invalid for more than an hour, so its
    /// own schedule cannot run.
    ClockInvalid,
    /// The feeder has been unable to feed for more than 30 minutes, for
    /// the reason it gave first.
    Blocked { reason: Blocked },
}

impl AlertCondition {
    /// The stored kind, the same as the JSON tag.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Offline => "offline",
            Self::Feed { .. } => "feed",
            Self::Unrecorded { .. } => "unrecorded",
            Self::HopperEmpty => "hopper_empty",
            Self::InstallFailed { .. } => "install_failed",
            Self::ClockInvalid => "clock_invalid",
            Self::Blocked { .. } => "blocked",
        }
    }

    /// What tells two alerts of one kind on one feeder apart: the feed,
    /// the journal entry or the install it names, and nothing for a
    /// condition a feeder is in or not.
    pub fn subject(&self) -> String {
        match self {
            Self::Feed { epoch, feed, .. } => format!("{epoch}/{feed}"),
            Self::Unrecorded { command } => command.simple().to_string(),
            Self::InstallFailed { install, .. } => install.0.simple().to_string(),
            Self::Offline | Self::HopperEmpty | Self::ClockInvalid | Self::Blocked { .. } => {
                String::new()
            }
        }
    }

    /// Whether the alert clears itself when its condition ends. A feed's
    /// result never changes, so its alert waits for an owner.
    pub const fn clears(&self) -> bool {
        !matches!(self, Self::Feed { .. })
    }
}

/// Where an alert stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AlertState {
    /// Its condition holds, and no owner has acknowledged it.
    Open,
    /// An owner has seen it; its condition may still hold.
    Acknowledged,
    /// Its condition ended.
    Cleared,
}

impl AlertState {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Acknowledged => "acknowledged",
            Self::Cleared => "cleared",
        }
    }
}

/// Who acknowledged an alert, and when.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Acknowledgement {
    pub by: UserId,
    pub at: Timestamp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Alert {
    pub id: AlertId,
    pub feeder: FeederId,
    pub condition: AlertCondition,
    pub state: AlertState,
    /// When the condition began, as far as the server knows: when it saw
    /// the feeder go, the feed end, the beam read empty, and so on.
    pub since: Timestamp,
    /// When the server raised it.
    pub opened: Timestamp,
    pub acknowledged: Option<Acknowledgement>,
    /// When its condition ended.
    pub cleared: Option<Timestamp>,
}

/// A page of alerts, oldest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AlertPage {
    pub items: Vec<Alert>,
    /// The `after` for the next page; `None` at the end.
    pub next: Option<String>,
}

/// Which alerts to list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AlertQuery {
    /// Only alerts in this state; every alert when absent.
    pub state: Option<AlertState>,
    /// Only this feeder's alerts.
    pub feeder: Option<FeederId>,
    /// The previous page's `next`, as it came.
    pub after: Option<String>,
    /// At most this many (1-1000, default 100).
    pub limit: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_condition_names_its_kind_as_its_tag() {
        let install = InstallId(Uuid::from_u128(7));
        let conditions = [
            AlertCondition::Offline,
            AlertCondition::Feed {
                epoch: 1,
                feed: 2,
                outcome: FeedOutcome::Uncertain,
                reason: FeedReason::Power,
                requested: 2,
                completed: 0,
            },
            AlertCondition::Unrecorded {
                command: Uuid::from_u128(9),
            },
            AlertCondition::HopperEmpty,
            AlertCondition::InstallFailed {
                install,
                failure: Some(InstallFailure::Lost),
            },
            AlertCondition::ClockInvalid,
            AlertCondition::Blocked {
                reason: Blocked::Fault,
            },
        ];

        for condition in conditions {
            let json = serde_json::to_value(condition).unwrap();
            assert_eq!(json["kind"], condition.code(), "{condition:?}");
            let back: AlertCondition = serde_json::from_value(json).unwrap();
            assert_eq!(back, condition);
        }
        assert_eq!(conditions[1].subject(), "1/2");
        assert!(!conditions[1].clears());
        assert_eq!(conditions[0].subject(), "");
        assert!(conditions[4].clears());
    }
}
