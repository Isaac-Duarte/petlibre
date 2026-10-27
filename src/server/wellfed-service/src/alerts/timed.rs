//! The rules that turn on time passing: a clock invalid past an hour,
//! feeding blocked past half an hour, and each only past its limit.

use jiff::{SignedDuration, Timestamp};
use wellfed_api::{
    AlertCondition, Blocked, Connection as Link, FeederStatus, Generation, Source, WifiSource,
};
use wellfed_store::{Boot, ClockSync};

use super::rules::{Facts, Holds, clock_invalid_since, conditions, decide};
use super::tests::{at, kinds, minutes, projected, quiet, standing};

fn status(clock: Option<Timestamp>) -> FeederStatus {
    FeederStatus {
        build: "test".into(),
        bank: 0,
        uptime_ms: 0,
        boot_count: 1,
        reset_flags: 0,
        faults: 0,
        rssi: -50,
        wifi: WifiSource::Build,
        link_losses: 0,
        dns_losses: 0,
        clock,
        power: None,
        locked: None,
        settings_generation: Generation(1),
        wifi_generation: Generation(1),
        last_event: None,
        sessions: 1,
        readiness: None,
    }
}

#[test]
fn a_clock_invalid_past_an_hour_is_an_alert_until_it_is_set() {
    let boot = Boot {
        bank: 0,
        reset_flags: 0,
    };
    let invalid = Facts {
        boot: Some(projected(0, false, boot)),
        status: Some((status(None), at(60))),
        ..quiet()
    };

    assert_eq!(clock_invalid_since(&invalid), Some(at(0)));
    assert!(conditions(&invalid, &[], at(3600)).is_empty());
    assert_eq!(
        kinds(&conditions(&invalid, &[], at(3601))),
        ["clock_invalid"]
    );

    let synced = Facts {
        sync: Some(projected(
            120,
            true,
            ClockSync {
                by: Source::Feeder,
                jump_ms: None,
            },
        )),
        ..invalid.clone()
    };
    assert_eq!(clock_invalid_since(&synced), None);
    let raised = standing(AlertCondition::ClockInvalid, 0);
    assert_eq!(decide(&synced, &[raised], at(7200)).clear, [raised.id]);

    // A boot with no time after the sync starts it again, from that boot.
    let rebooted = Facts {
        boot: Some(projected(500, false, boot)),
        ..synced
    };
    assert_eq!(clock_invalid_since(&rebooted), Some(at(500)));
    let read_valid = Facts {
        status: Some((status(Some(at(600))), at(600))),
        ..rebooted
    };
    assert_eq!(clock_invalid_since(&read_valid), None);
}

#[test]
fn blocked_past_half_an_hour_is_an_alert_for_its_first_reason_until_it_can_feed() {
    let blocked = Facts {
        blocked: vec![
            projected(0, true, Some(Blocked::Battery)),
            projected(600, true, Some(Blocked::Fault)),
        ],
        ..quiet()
    };

    assert!(conditions(&blocked, &[], at(minutes(30))).is_empty());
    let holds = conditions(&blocked, &[], at(minutes(30) + 1));
    assert_eq!(
        holds,
        [Holds {
            condition: AlertCondition::Blocked {
                reason: Blocked::Battery
            },
            since: at(0)
        }]
    );

    let raised = standing(holds[0].condition, 0);
    let free = Facts {
        blocked: Vec::new(),
        ..quiet()
    };
    assert_eq!(decide(&free, &[raised], at(minutes(40))).clear, [raised.id]);
    assert!(
        decide(&blocked, &[raised], at(minutes(40)))
            .clear
            .is_empty()
    );
}

#[test]
fn time_turns_a_condition_on_only_past_its_limit() {
    let limit = |code: &str| -> i64 {
        match code {
            "offline" => i64::from(wellfed_api::OFFLINE_AFTER_S),
            "clock_invalid" => i64::from(wellfed_api::CLOCK_INVALID_AFTER_S),
            _ => i64::from(wellfed_api::BLOCKED_AFTER_S),
        }
    };
    let facts = Facts {
        link: Link::Offline,
        offline_since: Some(at(0)),
        boot: Some(projected(
            0,
            false,
            Boot {
                bank: 0,
                reset_flags: 0,
            },
        )),
        blocked: vec![projected(0, true, Some(Blocked::Journal))],
        ..quiet()
    };

    for code in ["offline", "clock_invalid", "blocked"] {
        let edge = at(limit(code));
        let after = edge.checked_add(SignedDuration::from_secs(1)).unwrap();
        assert!(
            !kinds(&conditions(&facts, &[], edge)).contains(&code),
            "{code}"
        );
        assert!(
            kinds(&conditions(&facts, &[], after)).contains(&code),
            "{code}"
        );
    }
}
