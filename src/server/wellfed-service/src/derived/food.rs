//! The food view: the hopper as the beam last read it, what is left since
//! the newest refill, and a refill the beam saw that nobody recorded.
//!
//! What was in the hopper before a refill is not known, so what is left is
//! the refill less what was dispensed since. The beam sees only whether
//! food reaches it, so it cannot say how much is left, only that none is:
//! once it reads empty after the refill, nothing is left. A refill the beam
//! saw (empty, then food) with none recorded is shown as "refilled?" and
//! does not start the count again: it may have been a top-up of unknown
//! size.

use jiff::{SignedDuration, Timestamp};
use wellfed_api::{FoodEstimate, FoodPeriod, HopperReading, Refill};
use wellfed_store::{Feed, FoodReading, Projected};

use super::{feed_time, portion_mg, weigh, whole};

/// How far back the dispensing rate is read for the time the hopper runs
/// out.
const RATE_WINDOW: SignedDuration = SignedDuration::from_hours(7 * 24);

/// What the food view is worked out from.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FoodInputs<'a> {
    pub(crate) current: Option<&'a FoodPeriod>,
    pub(crate) last_refill: Option<&'a Refill>,
    /// The feeds received since the refill (or since `since`, without one),
    /// in any order.
    pub(crate) feeds: &'a [Projected<Feed>],
    /// The feeder's food history, oldest first.
    pub(crate) periods: &'a [FoodPeriod],
    /// The food reads received since the refill (or since `since`), oldest
    /// first.
    pub(crate) readings: &'a [FoodReading],
    /// Where the readings start without a refill.
    pub(crate) since: Timestamp,
    pub(crate) now: Timestamp,
}

/// The beam's newest reading, once it has read one.
pub(crate) fn hopper(last: Option<&FoodReading>) -> Option<HopperReading> {
    let last = last?;
    Some(HopperReading {
        present: last.food.present?,
        received: last.received,
    })
}

/// The estimate since the newest refill, and the newest refill the beam
/// saw that nobody recorded.
pub(crate) fn food_view(inputs: &FoodInputs<'_>) -> (Option<FoodEstimate>, Option<Timestamp>) {
    let start = inputs.last_refill.map_or(inputs.since, |refill| refill.at);
    let readings: Vec<&FoodReading> = inputs
        .readings
        .iter()
        .filter(|reading| reading.received >= start)
        .collect();

    let unrecorded = unrecorded_refill(&readings);
    let estimate = inputs
        .last_refill
        .map(|refill| estimate(inputs, refill, &readings));
    (estimate, unrecorded)
}

/// The newest reading that found food after one that found none, or after
/// one in the same boot that counted fewer refills.
fn unrecorded_refill(readings: &[&FoodReading]) -> Option<Timestamp> {
    readings
        .windows(2)
        .filter_map(|pair| {
            let (before, after) = (pair[0], pair[1]);
            let found = after.food.present == Some(true)
                && (before.food.present == Some(false)
                    || (before.boot_count.is_some()
                        && before.boot_count == after.boot_count
                        && after.food.refilled > before.food.refilled));
            found.then_some(after.received)
        })
        .next_back()
}

fn estimate(inputs: &FoodInputs<'_>, refill: &Refill, readings: &[&FoodReading]) -> FoodEstimate {
    let rate_from = inputs
        .now
        .checked_sub(RATE_WINDOW)
        .map_or(refill.at, |week| week.max(refill.at));

    let (mut portions, mut unweighed, mut dispensed, mut recent) = (0, 0, 0, 0);
    for feed in inputs.feeds {
        // A feed the server holds has happened, even when the feeder's
        // clock reads a little ahead of the server's.
        let at = feed_time(feed);
        if at < refill.at {
            continue;
        }
        let weighed = weigh(feed, inputs.periods);
        portions += weighed.portions;
        match weighed.mg {
            Some(mg) => {
                dispensed += mg;
                if at >= rate_from {
                    recent += mg;
                }
            }
            None => unweighed += weighed.portions,
        }
    }

    let emptied = readings
        .last()
        .is_some_and(|reading| reading.food.present == Some(false));
    let remaining = if emptied {
        0
    } else {
        (u64::from(refill.grams) * 1000).saturating_sub(dispensed)
    };
    let current = inputs.current.map(|period| &period.food);
    FoodEstimate {
        portion_mg: current.map(|food| u32::try_from(portion_mg(food)).unwrap_or(u32::MAX)),
        portions: u32::try_from(portions).unwrap_or(u32::MAX),
        unweighed_portions: u32::try_from(unweighed).unwrap_or(u32::MAX),
        dispensed_grams: whole(dispensed),
        remaining_grams: whole(remaining),
        bag_grams: current.map(|food| food.bag_grams),
        empty_by: empty_by(remaining, recent, rate_from, inputs.now),
    }
}

/// When `remaining` milligrams run out at the rate `recent` were dispensed
/// from `from` to `now`.
fn empty_by(remaining: u64, recent: u64, from: Timestamp, now: Timestamp) -> Option<Timestamp> {
    let elapsed = u64::try_from(now.duration_since(from).as_millis()).ok()?;
    if recent == 0 || elapsed == 0 {
        return None;
    }

    let left_ms = u128::from(remaining) * u128::from(elapsed) / u128::from(recent);
    let left = SignedDuration::from_millis(i64::try_from(left_ms).ok()?);
    now.checked_add(left).ok()
}
