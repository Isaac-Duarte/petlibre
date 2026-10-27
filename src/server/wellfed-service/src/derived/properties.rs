//! Properties of the derived views over random feeds, foods, links and
//! readings: shares add up, days read apart are the days read whole, a
//! feed only takes from what is left, and "refilled?" is food found after
//! none.

use jiff::civil::date;
use jiff::tz::TimeZone;
use wellfed_api::{FeedOutcome, Food, FoodPeriod, PetLink};
use wellfed_store::{Feed, FoodReading, Projected};
use proptest::prelude::*;

use super::tests::{FEEDER, PETS, TOM, at, feed, food, link, periods, reading, refill, utc_days};
use super::*;
use crate::derived::intake::tallies;

/// Up to three pets' links to the feeder, each a span within ten days.
fn arb_links() -> impl Strategy<Value = Vec<PetLink>> {
    let one = (
        0..3usize,
        0..864_000i64,
        proptest::option::of(1..864_000i64),
    );
    proptest::collection::vec(one, 1..5).prop_map(|links| {
        links
            .into_iter()
            .map(|(pet, from, span)| link(PETS[pet], from, span.map(|span| from + span)))
            .collect()
    })
}

fn arb_feeds() -> impl Strategy<Value = Vec<Projected<Feed>>> {
    let one = (0..864_000i64, 0..6u8, any::<bool>(), 0..5usize);
    proptest::collection::vec(one, 0..30).prop_map(|feeds| {
        feeds
            .into_iter()
            .enumerate()
            .map(|(n, (when, completed, valid, outcome))| {
                let mut fed = feed(
                    u32::try_from(n).unwrap(),
                    when,
                    completed,
                    FeedOutcome::ALL[outcome],
                );
                if !valid {
                    fed.at = None;
                }
                fed
            })
            .collect()
    })
}

fn arb_periods() -> impl Strategy<Value = Vec<FoodPeriod>> {
    let one = (1..400_000i64, 1..1000u32, 1..10_000u32);
    proptest::collection::vec(one, 0..4).prop_map(|changes| {
        let mut from = -100;
        let changes: Vec<(i64, Food)> = changes
            .into_iter()
            .enumerate()
            .map(|(n, (gap, cup, kcal))| {
                from += gap;
                (from, food(u128::try_from(n).unwrap(), cup, kcal))
            })
            .collect();
        periods(&changes)
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Every pet's share of a feed adds up to the feed, less at most a
    /// milligram per pet for the division.
    #[test]
    fn the_shares_add_up_to_what_the_feeder_dispensed(
        feeds in arb_feeds(),
        food_history in arb_periods(),
        links in arb_links(),
    ) {
        let histories = [FeederHistory {
            feeder: FEEDER,
            feeds: &feeds,
            periods: &food_history,
            links: &links,
        }];
        let days = utc_days(date(2026, 9, 30), date(2026, 10, 12));

        let shared: u64 = PETS
            .iter()
            .flat_map(|pet| tallies(*pet, &histories, &days).into_values())
            .map(|tally| tally.mg)
            .sum();
        let (mut whole, mut slack) = (0, 0);
        for fed in &feeds {
            let served = served_at(&links, feed_time(fed)).len();
            if served > 0 && let Some(mg) = weigh(fed, &food_history).mg {
                whole += mg;
                slack += u64::try_from(served).unwrap();
            }
        }

        prop_assert!(shared <= whole);
        prop_assert!(whole - shared < slack.max(1));
    }

    /// Two spans of days read apart are the span read whole.
    #[test]
    fn days_read_apart_are_the_days_read_whole(
        feeds in arb_feeds(),
        food_history in arb_periods(),
        links in arb_links(),
        split in 0..10i8,
        berlin in any::<bool>(),
    ) {
        let histories = [FeederHistory {
            feeder: FEEDER,
            feeds: &feeds,
            periods: &food_history,
            links: &links,
        }];
        let zone = if berlin { TimeZone::get("Europe/Berlin").unwrap() } else { TimeZone::UTC };
        let first = date(2026, 9, 30);
        let last = date(2026, 10, 11);
        let middle = first.checked_add(jiff::Span::new().days(split)).unwrap();
        let read = |from, to| intake(TOM, Some(300), &histories, &Days { from, to, zone: zone.clone() });

        let whole = read(first, last);
        let before = read(first, middle);
        let after = read(middle.tomorrow().unwrap(), last);

        prop_assert_eq!(whole.days.len(), 12);
        let joined: Vec<_> = before.days.iter().chain(&after.days).copied().collect();
        prop_assert_eq!(&whole.days, &joined);
        let anomalies: Vec<_> = before.anomalies.iter().chain(&after.anomalies).copied().collect();
        prop_assert_eq!(whole.anomalies, anomalies);
    }

    /// A feed after the refill never adds to what is left, and the count
    /// is the confirmed portions since the refill.
    #[test]
    fn a_feed_only_takes_from_what_is_left(
        feeds in arb_feeds(),
        food_history in arb_periods(),
        grams in 1..100_000u32,
        refilled_at in 0..864_000i64,
    ) {
        let refilled = refill(refilled_at, grams);
        let now = at(900_000);
        let view = |feeds: &[Projected<Feed>]| {
            food_view(&FoodInputs {
                current: food_history.last(),
                last_refill: Some(&refilled),
                feeds,
                periods: &food_history,
                readings: &[],
                since: at(0),
                now,
            })
            .0
            .unwrap()
        };

        let mut seen = Vec::new();
        let mut before = view(&seen);
        prop_assert_eq!(before.remaining_grams, grams);
        for fed in feeds {
            seen.push(fed);
            let after = view(&seen);
            prop_assert!(after.remaining_grams <= before.remaining_grams);
            prop_assert!(after.portions >= before.portions);
            before = after;
        }

        let counted: u32 = seen
            .iter()
            .filter(|fed| feed_time(fed) >= refilled.at)
            .map(|fed| u32::from(fed.value.completed))
            .sum();
        prop_assert_eq!(before.portions, counted);
    }

    /// "Refilled?" is the newest reading after the refill that found food
    /// where the one before found none, or counted a refill more in the
    /// same boot.
    #[test]
    fn an_unrecorded_refill_is_food_found_after_none(
        steps in proptest::collection::vec((1..5_000i64, proptest::option::of(any::<bool>()), 0..2u32, 0..3u32), 0..12),
        refilled_at in 0..20_000i64,
    ) {
        let mut readings = Vec::new();
        let (mut when, mut refills) = (0, 0);
        for (gap, present, boot, more) in steps {
            when += gap;
            refills += more;
            readings.push(reading(when, boot, present, refills));
        }
        let refilled = refill(refilled_at, 100);

        let (_, found) = food_view(&FoodInputs {
            current: None,
            last_refill: Some(&refilled),
            feeds: &[],
            periods: &[],
            readings: &readings,
            since: at(0),
            now: at(100_000),
        });

        let after: Vec<&FoodReading> =
            readings.iter().filter(|r| r.received >= refilled.at).collect();
        let mut want = None;
        for pair in after.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let grew = a.boot_count == b.boot_count && b.food.refilled > a.food.refilled;
            if b.food.present == Some(true) && (a.food.present == Some(false) || grew) {
                want = Some(b.received);
            }
        }
        prop_assert_eq!(found, want);
    }
}
