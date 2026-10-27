//! A pet's intake, day by day in the household's zone: each feed of a
//! feeder that served the pet at the feed's time, split evenly between the
//! pets it served then, weighed by the food it served then. And the feeds
//! that went wrong among them.

use std::collections::BTreeMap;

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use wellfed_api::{
    AnomalyKind, FeedAnomaly, FeedOutcome, FeederId, FoodPeriod, IntakeDay, PetId, PetIntake,
    PetLink,
};
use wellfed_store::{Feed, Projected};

use super::{feed_time, served_at, weigh, whole};

/// The days asked for, both included, in the household's zone.
#[derive(Clone, Debug)]
pub(crate) struct Days {
    pub(crate) from: Date,
    pub(crate) to: Date,
    pub(crate) zone: TimeZone,
}

impl Days {
    /// Where the first day starts and where the day after the last starts.
    pub(crate) fn span(&self) -> Option<(Timestamp, Timestamp)> {
        let start = |date: Date| {
            date.to_zoned(self.zone.clone())
                .ok()
                .map(|zoned| zoned.timestamp())
        };
        Some((start(self.from)?, start(self.to.tomorrow().ok()?)?))
    }

    fn date(&self, at: Timestamp) -> Date {
        at.to_zoned(self.zone.clone()).date()
    }

    /// Every day from the first to the last.
    fn each(&self) -> Vec<Date> {
        let mut days = Vec::new();
        let mut day = self.from;
        while day <= self.to {
            days.push(day);
            let Ok(next) = day.tomorrow() else {
                break;
            };
            day = next;
        }
        days
    }
}

/// What one feeder holds for the days asked for.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FeederHistory<'a> {
    pub(crate) feeder: FeederId,
    /// Its feeds, in any order; those outside the days are left out.
    pub(crate) feeds: &'a [Projected<Feed>],
    /// Its food history, oldest first.
    pub(crate) periods: &'a [FoodPeriod],
    /// Every pet's links to it over the days.
    pub(crate) links: &'a [PetLink],
}

/// One day's share, before rounding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Tally {
    pub(crate) mg: u64,
    pub(crate) mkcal: u64,
    pub(crate) unweighed_milliportions: u64,
    pub(crate) shared: bool,
}

/// The pet's share of each day's feeds, unrounded, by date.
pub(crate) fn tallies(
    pet: PetId,
    feeders: &[FeederHistory<'_>],
    days: &Days,
) -> BTreeMap<Date, Tally> {
    let mut tallies = BTreeMap::new();
    let Some((start, end)) = days.span() else {
        return tallies;
    };

    for history in feeders {
        for feed in history.feeds {
            let at = feed_time(feed);
            if at < start || at >= end {
                continue;
            }
            let pets = served_at(history.links, at);
            if !pets.contains(&pet) {
                continue;
            }
            // Nonzero: the pet itself is among them.
            let share = u64::try_from(pets.len()).unwrap_or(u64::MAX);

            let weighed = weigh(feed, history.periods);
            let tally: &mut Tally = tallies.entry(days.date(at)).or_default();
            tally.shared |= share > 1;
            match (weighed.mg, weighed.mkcal) {
                (Some(mg), Some(mkcal)) => {
                    tally.mg += mg / share;
                    tally.mkcal += mkcal / share;
                }
                _ => tally.unweighed_milliportions += weighed.portions * 1000 / share,
            }
        }
    }
    tallies
}

/// The pet's intake over `days`, against its daily target.
pub(crate) fn intake(
    pet: PetId,
    target: Option<u32>,
    feeders: &[FeederHistory<'_>],
    days: &Days,
) -> PetIntake {
    let tallies = tallies(pet, feeders, days);
    let days_out = days
        .each()
        .into_iter()
        .map(|date| {
            let tally = tallies.get(&date).copied().unwrap_or_default();
            let kcal = whole(tally.mkcal);
            IntakeDay {
                date,
                grams: whole(tally.mg),
                kcal,
                target_percent: target
                    .filter(|target| *target > 0)
                    .map(|target| percent(tally.mkcal, target)),
                estimate: tally.shared,
                unweighed_milliportions: u32::try_from(tally.unweighed_milliportions)
                    .unwrap_or(u32::MAX),
            }
        })
        .collect();

    PetIntake {
        pet,
        daily_kcal: target,
        days: days_out,
        anomalies: anomalies(pet, feeders, days),
    }
}

/// `mkcal` thousandths of a kcal as a rounded percentage of `target` kcal.
fn percent(mkcal: u64, target: u32) -> u32 {
    let target = u64::from(target) * 1000;
    u32::try_from((mkcal * 100 + target / 2) / target).unwrap_or(u32::MAX)
}

/// The feeds that went wrong, of the feeders while they served the pet,
/// oldest first.
pub(crate) fn anomalies(
    pet: PetId,
    feeders: &[FeederHistory<'_>],
    days: &Days,
) -> Vec<FeedAnomaly> {
    let Some((start, end)) = days.span() else {
        return Vec::new();
    };

    let mut found: Vec<FeedAnomaly> = feeders
        .iter()
        .flat_map(|history| {
            history.feeds.iter().filter_map(move |feed| {
                let at = feed_time(feed);
                let inside = at >= start && at < end;
                if !inside || !served_at(history.links, at).contains(&pet) {
                    return None;
                }
                let value = &feed.value;
                Some(FeedAnomaly {
                    feeder: history.feeder,
                    epoch: feed.epoch,
                    feed: value.feed,
                    at,
                    kind: anomaly(value)?,
                    requested: value.requested,
                    completed: value.completed,
                    reason: value.reason,
                })
            })
        })
        .collect();
    found.sort_by_key(|anomaly| (anomaly.at, anomaly.feeder, anomaly.epoch, anomaly.feed));
    found
}

/// What went wrong with a feed, if anything. A cancelled feed is what
/// someone asked for.
pub(crate) fn anomaly(feed: &Feed) -> Option<AnomalyKind> {
    match feed.outcome {
        FeedOutcome::Expired => Some(AnomalyKind::Missed),
        FeedOutcome::Uncertain => Some(AnomalyKind::Uncertain),
        FeedOutcome::Failed => Some(AnomalyKind::Short),
        FeedOutcome::Done if feed.completed < feed.requested => Some(AnomalyKind::Short),
        FeedOutcome::Done | FeedOutcome::Cancelled => None,
    }
}
