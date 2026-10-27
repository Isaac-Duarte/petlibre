//! One test per alert rule, on rows a test makes: when it holds, since
//! when, and when it ends.

use jiff::Timestamp;
use wellfed_api::{
    Alert, AlertCondition, AlertId, AlertState, CommandKind, CommandOutcome, CommandState,
    Connection as Link, FeedOutcome, FeedReason, FeederId, FoodStats, Hex, Install, InstallFailure,
    InstallId, InstallState, Source, UserId,
};
use wellfed_store::{CommandRow, Feed, FoodReading, Projected};
use uuid::Uuid;

use super::rules::{Facts, Holds, conditions, decide};

const FEEDER: FeederId = FeederId(Uuid::from_u128(1));
const BASE_S: i64 = 1_790_812_800;

pub(super) fn at(seconds: i64) -> Timestamp {
    Timestamp::from_second(BASE_S + seconds).unwrap()
}

pub(super) fn minutes(n: i64) -> i64 {
    n * 60
}

/// A feeder online, with nothing wrong.
pub(super) fn quiet() -> Facts {
    Facts {
        link: Link::Online,
        offline_since: None,
        feeds: Vec::new(),
        unrecorded: Vec::new(),
        reading: None,
        refilled: None,
        install: None,
        boot: None,
        sync: None,
        status: None,
        blocked: Vec::new(),
    }
}

pub(super) fn standing(condition: AlertCondition, since: i64) -> Alert {
    Alert {
        id: AlertId(Uuid::from_u128(u128::from(since.unsigned_abs()) + 100)),
        feeder: FEEDER,
        condition,
        state: AlertState::Open,
        since: at(since),
        opened: at(since),
        acknowledged: None,
        cleared: None,
    }
}

pub(super) fn kinds(holds: &[Holds]) -> Vec<&'static str> {
    holds.iter().map(|held| held.condition.code()).collect()
}

pub(super) fn projected<T>(seconds: i64, valid: bool, value: T) -> Projected<T> {
    Projected {
        epoch: 0,
        seq: seconds.unsigned_abs(),
        at: valid.then(|| at(seconds)),
        received: at(seconds),
        value,
    }
}

#[test]
fn a_quiet_feeder_has_no_alert() {
    assert!(conditions(&quiet(), &[], at(0)).is_empty());
}

#[test]
fn offline_is_an_alert_after_ten_minutes_and_clears_online() {
    let offline = Facts {
        link: Link::Connecting,
        offline_since: Some(at(0)),
        ..quiet()
    };

    assert!(conditions(&offline, &[], at(minutes(10))).is_empty());
    let late = conditions(&offline, &[], at(minutes(10) + 1));
    assert_eq!(
        late,
        [Holds {
            condition: AlertCondition::Offline,
            since: at(0)
        }]
    );

    // After a restart the server counts from its start again; the alert
    // standing from before holds on rather than clearing.
    let restarted = Facts {
        offline_since: Some(at(minutes(60))),
        ..offline.clone()
    };
    let raised = standing(AlertCondition::Offline, 0);
    let kept = decide(&restarted, &[raised], at(minutes(61)));
    assert!(kept.clear.is_empty());

    let back = decide(&quiet(), &[raised], at(minutes(62)));
    assert_eq!(back.clear, [raised.id]);
    assert!(back.raise.is_empty());
}

#[test]
fn a_feed_that_ended_uncertain_failed_or_expired_is_an_alert_that_never_clears() {
    let fed = |n: u32, outcome| {
        projected(
            i64::from(n),
            true,
            Feed {
                feed: n,
                by: Source::Panel,
                requested: 2,
                completed: 1,
                outcome,
                reason: FeedReason::Power,
            },
        )
    };
    let facts = Facts {
        feeds: FeedOutcome::ALL
            .iter()
            .zip(1..)
            .map(|(outcome, n)| fed(n, *outcome))
            .collect(),
        ..quiet()
    };

    let holds = conditions(&facts, &[], at(100));
    let outcomes: Vec<FeedOutcome> = holds
        .iter()
        .filter_map(|held| match held.condition {
            AlertCondition::Feed { outcome, .. } => Some(outcome),
            _ => None,
        })
        .collect();
    assert_eq!(
        outcomes,
        [
            FeedOutcome::Expired,
            FeedOutcome::Failed,
            FeedOutcome::Uncertain
        ]
    );

    // Its feed gone from the lookback, a feed's alert still stands.
    let raised = standing(holds[0].condition, 1);
    assert!(decide(&quiet(), &[raised], at(200)).clear.is_empty());
}

#[test]
fn an_unrecorded_feed_is_an_alert_until_the_history_shows_it() {
    let row = CommandRow {
        seq: 1,
        key: [7; 16],
        user: UserId(Uuid::from_u128(2)),
        token: None,
        feeder: FEEDER,
        kind: CommandKind::Feed,
        body_hash: [0; 32],
        request_id: [0; 16],
        boot_count: Some(3),
        history: None,
        state: CommandState::Resolved,
        outcome: Some(CommandOutcome::Unrecorded),
        feed: None,
        sent: at(0),
        updated: at(30),
    };
    let facts = Facts {
        unrecorded: vec![row],
        ..quiet()
    };

    let holds = conditions(&facts, &[], at(60));
    let condition = AlertCondition::Unrecorded {
        command: Uuid::from_bytes([7; 16]),
    };
    assert_eq!(
        holds,
        [Holds {
            condition,
            since: at(30)
        }]
    );

    let raised = standing(condition, 30);
    assert_eq!(decide(&quiet(), &[raised], at(90)).clear, [raised.id]);
}

#[test]
fn an_empty_hopper_is_an_alert_until_food_is_read_or_a_refill_recorded() {
    let read = |seconds, present| FoodReading {
        received: at(seconds),
        boot_count: Some(1),
        food: FoodStats {
            present,
            emptied: 1,
            refilled: 0,
            feeds_since_refill: None,
        },
    };
    let empty = Facts {
        reading: Some(read(10, Some(false))),
        ..quiet()
    };

    assert_eq!(kinds(&conditions(&empty, &[], at(20))), ["hopper_empty"]);
    let refilled = Facts {
        refilled: Some(at(15)),
        ..empty.clone()
    };
    assert!(conditions(&refilled, &[], at(20)).is_empty());
    let older_refill = Facts {
        refilled: Some(at(5)),
        ..empty.clone()
    };
    assert_eq!(
        kinds(&conditions(&older_refill, &[], at(20))),
        ["hopper_empty"]
    );
    let full = Facts {
        reading: Some(read(30, Some(true))),
        ..quiet()
    };
    let unknown = Facts {
        reading: Some(read(30, None)),
        ..quiet()
    };
    assert!(conditions(&full, &[], at(40)).is_empty());
    assert!(conditions(&unknown, &[], at(40)).is_empty());
}

#[test]
fn a_failed_install_is_an_alert_until_the_next_install() {
    let install = |n: u128, state, failure| Install {
        id: InstallId(Uuid::from_u128(n)),
        feeder: FEEDER,
        release: Hex([0; 32]),
        version: 2,
        by: UserId(Uuid::from_u128(2)),
        started: at(0),
        state,
        written: 0,
        image_len: 10,
        failure,
        finished: Some(at(5)),
    };
    let failed = Facts {
        install: Some(install(1, InstallState::Failed, Some(InstallFailure::Lost))),
        ..quiet()
    };

    let holds = conditions(&failed, &[], at(10));
    let condition = AlertCondition::InstallFailed {
        install: InstallId(Uuid::from_u128(1)),
        failure: Some(InstallFailure::Lost),
    };
    assert_eq!(
        holds,
        [Holds {
            condition,
            since: at(5)
        }]
    );

    let next = Facts {
        install: Some(install(2, InstallState::Running, None)),
        ..quiet()
    };
    let raised = standing(condition, 5);
    assert_eq!(decide(&next, &[raised], at(20)).clear, [raised.id]);
}
