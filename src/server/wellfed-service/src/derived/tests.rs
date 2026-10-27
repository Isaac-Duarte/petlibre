//! The derived views' rules on rows a test makes: worked examples, and
//! properties over random feeds, foods, links and readings.

use jiff::civil::{Date, date};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use wellfed_api::{
    AnomalyKind, FeedOutcome, FeedReason, FeederId, Food, FoodId, FoodPeriod, FoodStats, PetId,
    PetLink, Refill, RefillId, Source, UserId,
};
use wellfed_store::{Feed, FoodReading, Projected};
use uuid::Uuid;

use super::*;
use crate::derived::intake::anomaly;

pub(super) const FEEDER: FeederId = FeederId(Uuid::from_u128(1));
pub(super) const TOM: PetId = PetId(Uuid::from_u128(10));
const ANA: PetId = PetId(Uuid::from_u128(11));
const BEA: PetId = PetId(Uuid::from_u128(12));
pub(super) const PETS: [PetId; 3] = [TOM, ANA, BEA];
/// 2026-10-01 00:00 UTC.
const BASE_S: i64 = 1_790_812_800;

pub(super) fn at(seconds: i64) -> Timestamp {
    Timestamp::from_second(BASE_S + seconds).unwrap()
}

pub(super) fn food(n: u128, grams_per_cup: u32, kcal_per_kg: u32) -> Food {
    Food {
        id: FoodId(Uuid::from_u128(n)),
        brand: "Acme".into(),
        name: format!("food {n}"),
        kcal_per_kg,
        bag_grams: 2000,
        grams_per_cup,
    }
}

/// Consecutive periods from each `(from, food)`, oldest first.
pub(super) fn periods(changes: &[(i64, Food)]) -> Vec<FoodPeriod> {
    changes
        .iter()
        .enumerate()
        .map(|(i, (from, food))| FoodPeriod {
            food: food.clone(),
            from: at(*from),
            to: changes.get(i + 1).map(|(to, _)| at(*to)),
        })
        .collect()
}

pub(super) fn feed(n: u32, when: i64, completed: u8, outcome: FeedOutcome) -> Projected<Feed> {
    Projected {
        epoch: 0,
        seq: u64::from(n),
        at: Some(at(when)),
        received: at(when + 5),
        value: Feed {
            feed: n,
            by: Source::Feeder,
            requested: completed.max(1),
            completed,
            outcome,
            reason: FeedReason::Done,
        },
    }
}

pub(super) fn link(pet: PetId, from: i64, to: Option<i64>) -> PetLink {
    PetLink {
        pet,
        feeder: FEEDER,
        from: at(from),
        to: to.map(at),
    }
}

pub(super) fn utc_days(from: Date, to: Date) -> Days {
    Days {
        from,
        to,
        zone: TimeZone::UTC,
    }
}

pub(super) fn reading(
    seconds: i64,
    boot: u32,
    present: Option<bool>,
    refilled: u32,
) -> FoodReading {
    FoodReading {
        received: at(seconds),
        boot_count: Some(boot),
        food: FoodStats {
            present,
            emptied: 0,
            refilled,
            feeds_since_refill: None,
        },
    }
}

pub(super) fn refill(seconds: i64, grams: u32) -> Refill {
    Refill {
        id: RefillId(Uuid::from_u128(5)),
        feeder: FEEDER,
        at: at(seconds),
        grams,
        full_bag: false,
        food: None,
        user: UserId(Uuid::from_u128(6)),
    }
}

#[test]
fn a_portion_is_a_twelfth_of_a_cup_of_the_food_served_then() {
    let dry = food(1, 120, 3600);
    let wet = food(2, 240, 1000);
    let history = periods(&[(0, dry), (3600, wet)]);

    let before = weigh(&feed(1, 60, 2, FeedOutcome::Done), &history);
    let after = weigh(&feed(2, 3600, 3, FeedOutcome::Done), &history);
    let unset = weigh(&feed(3, -60, 1, FeedOutcome::Done), &history);

    // 120 g a cup: 10 g a portion, 36 kcal at 3600 kcal/kg.
    assert_eq!((before.mg, before.mkcal), (Some(20_000), Some(72_000)));
    // From the change's own instant: 20 g a portion of 1000 kcal/kg.
    assert_eq!((after.mg, after.mkcal), (Some(60_000), Some(60_000)));
    assert_eq!((unset.portions, unset.mg), (1, None), "no food set yet");
}

#[test]
fn a_feed_with_an_invalid_clock_is_placed_by_when_it_was_received() {
    let mut late = feed(1, 0, 1, FeedOutcome::Done);
    late.at = None;
    late.received = at(86_400 + 10);

    assert_eq!(feed_time(&late), at(86_400 + 10));
}

#[test]
fn a_shared_feeder_splits_its_feeds_and_marks_the_day_an_estimate() {
    let history = periods(&[(0, food(1, 120, 3600))]);
    let feeds = [
        feed(1, 3600, 3, FeedOutcome::Done),          // Tom alone
        feed(2, 7200 + 60, 3, FeedOutcome::Done),     // Tom and Ana
        feed(3, 86_400 + 60, 2, FeedOutcome::Failed), // the next day
    ];
    let links = [link(TOM, 0, None), link(ANA, 7200, None)];
    let histories = [FeederHistory {
        feeder: FEEDER,
        feeds: &feeds,
        periods: &history,
        links: &links,
    }];
    let days = utc_days(date(2026, 10, 1), date(2026, 10, 2));

    let tom = intake(TOM, Some(200), &histories, &days);
    let ana = intake(ANA, None, &histories, &days);

    // 30 g alone, then half of 30 g: 45 g, 162 kcal of 200.
    assert_eq!(
        (
            tom.days[0].grams,
            tom.days[0].kcal,
            tom.days[0].target_percent
        ),
        (45, 162, Some(81))
    );
    assert!(tom.days[0].estimate);
    assert_eq!((ana.days[0].grams, ana.days[0].target_percent), (15, None));
    assert_eq!(tom.days[1].grams, 10, "half of the failed feed's 20 g");
    assert_eq!(tom.anomalies.len(), 1);
    assert_eq!(tom.anomalies[0].kind, AnomalyKind::Short);
    assert_eq!(tom.anomalies[0].feed, 3);
}

#[test]
fn a_day_is_the_household_zones() {
    let zone = TimeZone::get("Europe/Berlin").unwrap();
    let history = periods(&[(0, food(1, 120, 3600))]);
    // 23:30 UTC on October 1 is 01:30 on October 2 in Berlin.
    let feeds = [feed(1, 23 * 3600 + 1800, 1, FeedOutcome::Done)];
    let links = [link(TOM, 0, None)];
    let histories = [FeederHistory {
        feeder: FEEDER,
        feeds: &feeds,
        periods: &history,
        links: &links,
    }];
    let days = Days {
        from: date(2026, 10, 1),
        to: date(2026, 10, 2),
        zone,
    };

    let tom = intake(TOM, None, &histories, &days);

    let grams: Vec<u32> = tom.days.iter().map(|day| day.grams).collect();
    assert_eq!(grams, [0, 10]);
}

#[test]
fn each_outcome_is_an_anomaly_or_not() {
    let kind = |completed, requested, outcome| {
        anomaly(&Feed {
            feed: 1,
            by: Source::Panel,
            requested,
            completed,
            outcome,
            reason: FeedReason::Done,
        })
    };

    assert_eq!(kind(0, 2, FeedOutcome::Expired), Some(AnomalyKind::Missed));
    assert_eq!(
        kind(1, 2, FeedOutcome::Uncertain),
        Some(AnomalyKind::Uncertain)
    );
    assert_eq!(kind(1, 2, FeedOutcome::Failed), Some(AnomalyKind::Short));
    assert_eq!(kind(1, 2, FeedOutcome::Done), Some(AnomalyKind::Short));
    assert_eq!(kind(2, 2, FeedOutcome::Done), None);
    assert_eq!(kind(0, 2, FeedOutcome::Cancelled), None);
}

#[test]
fn the_estimate_counts_from_the_refill_until_the_beam_reads_empty() {
    let dry = food(1, 120, 3600);
    let history = periods(&[(0, dry)]);
    let current = history[0].clone();
    let refilled = refill(1000, 500);
    let feeds = [
        feed(1, 900, 4, FeedOutcome::Done), // before the refill
        feed(2, 1000 + 3600, 3, FeedOutcome::Done),
        feed(3, 1000 + 7200, 2, FeedOutcome::Uncertain),
    ];
    let full = [reading(1000 + 3700, 1, Some(true), 0)];
    let mut inputs = FoodInputs {
        current: Some(&current),
        last_refill: Some(&refilled),
        feeds: &feeds,
        periods: &history,
        readings: &full,
        since: at(0),
        now: at(1000 + 10_800),
    };

    let (estimate, unrecorded) = food_view(&inputs);
    let estimate = estimate.unwrap();
    assert_eq!(
        (
            estimate.portions,
            estimate.dispensed_grams,
            estimate.remaining_grams
        ),
        (5, 50, 450)
    );
    assert_eq!(
        (estimate.portion_mg, estimate.bag_grams),
        (Some(10_000), Some(2000))
    );
    // 50 g in 10 800 s: 450 g in 97 200 s more.
    assert_eq!(estimate.empty_by, Some(at(1000 + 10_800 + 97_200)));
    assert_eq!(unrecorded, None);

    let empty = [reading(1000 + 7300, 1, Some(false), 0)];
    inputs.readings = &empty;
    assert_eq!(food_view(&inputs).0.unwrap().remaining_grams, 0);
}

#[test]
fn a_refill_the_beam_saw_is_shown_and_does_not_reset_the_count() {
    let history = periods(&[(0, food(1, 120, 3600))]);
    let refilled = refill(1000, 500);
    let feeds = [
        feed(1, 2000, 2, FeedOutcome::Done),
        feed(2, 6000, 2, FeedOutcome::Done),
    ];
    let readings = [
        reading(2100, 1, Some(false), 0),
        reading(6100, 1, Some(true), 1),
    ];
    let inputs = FoodInputs {
        current: None,
        last_refill: Some(&refilled),
        feeds: &feeds,
        periods: &history,
        readings: &readings,
        since: at(0),
        now: at(7000),
    };

    let (estimate, unrecorded) = food_view(&inputs);

    assert_eq!(unrecorded, Some(at(6100)));
    assert_eq!(
        estimate.unwrap().portions,
        4,
        "counted from the recorded one"
    );
}

#[test]
fn the_hopper_is_the_newest_reading_once_the_beam_has_read() {
    assert_eq!(hopper(None), None);
    assert_eq!(hopper(Some(&reading(5, 1, None, 0))), None);
    let read = hopper(Some(&reading(5, 1, Some(false), 0))).unwrap();
    assert_eq!((read.present, read.received), (false, at(5)));
}

#[test]
fn the_rate_window_is_a_week() {
    // A refill long ago: the rate is read over the last 7 days only.
    let history = periods(&[(-10, food(1, 120, 3600))]);
    let refilled = refill(0, 10_000);
    let week = 7 * 86_400;
    let feeds = [
        feed(1, 10, 100, FeedOutcome::Done),          // 1 kg, long ago
        feed(2, 2 * week + 10, 7, FeedOutcome::Done), // 70 g this week
    ];
    let now = at(2 * week + 20);

    let (estimate, _) = food_view(&FoodInputs {
        current: None,
        last_refill: Some(&refilled),
        feeds: &feeds,
        periods: &history,
        readings: &[],
        since: at(0),
        now,
    });

    let estimate = estimate.unwrap();
    assert_eq!(estimate.remaining_grams, 10_000 - 1000 - 70);
    // 8 930 g at 70 g a week.
    let left = SignedDuration::from_secs(8930 * week / 70);
    assert_eq!(estimate.empty_by, Some(now.checked_add(left).unwrap()));
}
