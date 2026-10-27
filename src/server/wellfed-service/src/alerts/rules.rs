//! The alert rules: which conditions hold for a feeder now, from typed
//! rows and the time, and so which alerts to raise and which to clear.
//! Pure, so each rule is tested on rows without a store or a feeder.
//!
//! A time-limited condition (offline, a clock invalid, feeding blocked)
//! holds once it has lasted past its limit, and then for as long as it
//! lasts: an alert already standing for it keeps holding before the limit,
//! so a server restart, which forgets how long a feeder has been offline,
//! neither clears nor raises it again.

use jiff::{SignedDuration, Timestamp};
use wellfed_api::{
    Alert, AlertCondition, AlertId, BLOCKED_AFTER_S, Blocked, CLOCK_INVALID_AFTER_S,
    Connection as Link, FeedOutcome, FeederStatus, Install, InstallState, OFFLINE_AFTER_S,
};
use wellfed_store::{Boot, ClockSync, CommandRow, Feed, FoodReading, Projected};
use uuid::Uuid;

use crate::derived::feed_time;

/// What the rules read of one feeder.
#[derive(Clone, Debug)]
pub(crate) struct Facts {
    pub(crate) link: Link,
    /// Since when the server has had no session with it, by the server's
    /// clock; `None` while it has one.
    pub(crate) offline_since: Option<Timestamp>,
    /// Its feeds received lately, in any order.
    pub(crate) feeds: Vec<Projected<Feed>>,
    /// Its feeds the journal resolved `unrecorded`.
    pub(crate) unrecorded: Vec<CommandRow>,
    /// The food beam's newest reading.
    pub(crate) reading: Option<FoodReading>,
    /// When the newest refill was recorded.
    pub(crate) refilled: Option<Timestamp>,
    pub(crate) install: Option<Install>,
    pub(crate) boot: Option<Projected<Boot>>,
    pub(crate) sync: Option<Projected<ClockSync>>,
    /// Its status as the server last read it, and when.
    pub(crate) status: Option<(FeederStatus, Timestamp)>,
    /// The reasons it could not feed since it last could, oldest first.
    pub(crate) blocked: Vec<Projected<Option<Blocked>>>,
}

/// A condition that holds, and since when.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Holds {
    pub(crate) condition: AlertCondition,
    pub(crate) since: Timestamp,
}

/// What an evaluation changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Decision {
    /// Conditions to raise an alert for; one already raised is left alone.
    pub(crate) raise: Vec<Holds>,
    /// Standing alerts whose condition ended.
    pub(crate) clear: Vec<AlertId>,
}

/// Decide from `facts` at `now`, beside the feeder's `standing` alerts
/// (those not cleared that clear themselves).
pub(crate) fn decide(facts: &Facts, standing: &[Alert], now: Timestamp) -> Decision {
    let holds = conditions(facts, standing, now);
    let same = |alert: &Alert, held: &Holds| {
        alert.condition.code() == held.condition.code()
            && alert.condition.subject() == held.condition.subject()
    };

    let clear = standing
        .iter()
        .filter(|alert| alert.condition.clears() && !holds.iter().any(|held| same(alert, held)))
        .map(|alert| alert.id)
        .collect();
    Decision {
        raise: holds,
        clear,
    }
}

/// Every condition that holds for the feeder at `now`.
pub(crate) fn conditions(facts: &Facts, standing: &[Alert], now: Timestamp) -> Vec<Holds> {
    let standing_kind = |code: &str| standing.iter().any(|alert| alert.condition.code() == code);
    let lasted = |since: Timestamp, limit_s: u32, code: &str| {
        now.duration_since(since) > SignedDuration::from_secs(i64::from(limit_s))
            || standing_kind(code)
    };
    let mut holds = Vec::new();

    if let (true, Some(since)) = (facts.link != Link::Online, facts.offline_since)
        && lasted(since, OFFLINE_AFTER_S, AlertCondition::Offline.code())
    {
        holds.push(Holds {
            condition: AlertCondition::Offline,
            since,
        });
    }

    holds.extend(facts.feeds.iter().filter_map(failed_feed));
    holds.extend(facts.unrecorded.iter().map(|row| Holds {
        condition: AlertCondition::Unrecorded {
            command: Uuid::from_bytes(row.key),
        },
        since: row.updated,
    }));
    holds.extend(hopper_empty(facts));
    holds.extend(install_failed(facts.install.as_ref()));

    if let Some(since) = clock_invalid_since(facts)
        && lasted(
            since,
            CLOCK_INVALID_AFTER_S,
            AlertCondition::ClockInvalid.code(),
        )
    {
        holds.push(Holds {
            condition: AlertCondition::ClockInvalid,
            since,
        });
    }

    if let Some(blocked) = blocked(&facts.blocked)
        && lasted(blocked.since, BLOCKED_AFTER_S, blocked.condition.code())
    {
        holds.push(blocked);
    }
    holds
}

/// A feed that ended uncertain, failed or expired.
fn failed_feed(feed: &Projected<Feed>) -> Option<Holds> {
    let value = &feed.value;
    let alerting = matches!(
        value.outcome,
        FeedOutcome::Uncertain | FeedOutcome::Failed | FeedOutcome::Expired
    );
    alerting.then(|| Holds {
        condition: AlertCondition::Feed {
            epoch: feed.epoch,
            feed: value.feed,
            outcome: value.outcome,
            reason: value.reason,
            requested: value.requested,
            completed: value.completed,
        },
        since: feed_time(feed),
    })
}

/// The beam's newest reading is empty, and no refill was recorded after it.
fn hopper_empty(facts: &Facts) -> Option<Holds> {
    let reading = facts.reading.as_ref()?;
    let empty = reading.food.present == Some(false);
    let refilled_after = facts
        .refilled
        .is_some_and(|refilled| refilled >= reading.received);
    (empty && !refilled_after).then_some(Holds {
        condition: AlertCondition::HopperEmpty,
        since: reading.received,
    })
}

/// The feeder's last install failed.
fn install_failed(install: Option<&Install>) -> Option<Holds> {
    let install = install.filter(|install| install.state == InstallState::Failed)?;
    Some(Holds {
        condition: AlertCondition::InstallFailed {
            install: install.id,
            failure: install.failure,
        },
        since: install.finished.unwrap_or(install.started),
    })
}

/// Since when the feeder's clock has been invalid, when the newest of what
/// the server saw of it says it is: a boot with no time, the status read
/// with no clock. A clock sync, a boot with a time or a status read with a
/// clock says it is valid. The first invalid sign after the last valid one
/// is when it began.
pub(crate) fn clock_invalid_since(facts: &Facts) -> Option<Timestamp> {
    let mut signs: Vec<(Timestamp, bool)> = Vec::new();
    if let Some(boot) = &facts.boot {
        signs.push((boot.received, boot.at.is_some()));
    }
    if let Some(sync) = &facts.sync {
        signs.push((sync.received, true));
    }
    if let Some((status, read)) = &facts.status {
        signs.push((*read, status.clock.is_some()));
    }
    // At one instant, a valid sign wins: a sync answers the boot before it.
    signs.sort_by_key(|&(at, valid)| (at, !valid));

    let last_valid = signs.iter().rposition(|&(_, valid)| valid);
    let first_invalid = last_valid.map_or(0, |index| index + 1);
    signs.get(first_invalid).map(|&(at, _)| at)
}

/// The feeder cannot feed, since its first reason after it last could.
fn blocked(changes: &[Projected<Option<Blocked>>]) -> Option<Holds> {
    let (first, last) = (changes.first()?, changes.last()?);
    last.value?;
    Some(Holds {
        condition: AlertCondition::Blocked {
            reason: first.value?,
        },
        since: first.received,
    })
}
