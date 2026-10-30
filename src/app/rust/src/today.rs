//! What the Home card and the feeder's Today tab show: the status with the
//! plans, the time settings and the feeding history a session has read. The
//! reckoning is `wellfed_client`'s, on the feeder's own clock and zone, so
//! Dart holds no schedule logic and the card says what the feeder will do.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use wellfed_client::history::Feed;
use wellfed_client::plans::{self, MealState};
use wellfed_client::time::{feeder_zone, next_meal};
use wellfed_model::{
    Battery, Event, EventBody, FeedOutcome, Gap, Plans, Readiness, Settings, Source, Status,
    TimeView,
};

use crate::api::feeder::TimeInfo;
use crate::api::session::{
    Card, FaultView, FedBy, FeedResult, HistoryBody, HistoryEntry, MealStanding, MealView,
    NextMealView,
};
use crate::requests::{plans_view, settings_view, time_info};

/// What a session has read of the feeder.
#[derive(Default)]
pub(crate) struct Facts {
    pub(crate) status: Option<Status>,
    pub(crate) settings: Option<Settings>,
    pub(crate) plans: Option<Plans>,
    pub(crate) time: Option<TimeView>,
    /// The history's feeds so far, oldest first.
    pub(crate) feeds: Vec<Feed>,
}

impl Facts {
    /// The card, once the status is in.
    pub(crate) fn card(&self) -> Option<Card> {
        let status = self.status.as_ref()?;
        let time: Option<TimeInfo> = self.time.as_ref().map(time_info);
        let adapter = status
            .supply
            .and_then(|supply| supply.adapter)
            .or(status.power.map(|power| power.adapter));
        let battery_percent = status.supply.and_then(|supply| match supply.battery {
            Battery::Reading { percent, .. } => Some(percent),
            _ => None,
        });
        let blocked = match status.readiness {
            Some(Readiness::Blocked(reason)) => Some(reason),
            Some(Readiness::Ready) | None => None,
        };
        let fault = status.feed_fault.map(|fault| FaultView {
            feed: fault.feed,
            outcome: fault.outcome,
            reason: fault.reason,
            result: self
                .feeds
                .iter()
                .rev()
                .find(|fed| fed.feed == fault.feed)
                .map(result),
        });

        let (next, meals) = self.meals(status);
        Some(Card {
            build: status.build.clone(),
            food: status.food,
            locked: status.locked,
            ready: status.readiness == Some(Readiness::Ready),
            blocked,
            adapter,
            battery_percent,
            clock_set: status.clock.is_some(),
            rssi: status.rssi,
            fault,
            desiccant: status.desiccant,
            uptime_ms: status.uptime_ms,
            bank: status.bank,
            next_meal: next,
            last_fed: self.feeds.last().map(result),
            meals,
            time,
            settings: self.settings.as_ref().map(settings_view),
            plans: self.plans.as_ref().map(plans_view),
            wifi_generation: status.wifi_generation.0,
        })
    }

    /// The next meal and today's, by the feeder's clock in its own zone. A
    /// feeder with no clock holds its meals, so there are neither.
    fn meals(&self, status: &Status) -> (Option<NextMealView>, Vec<MealView>) {
        let (Some(plans), Some(now)) = (self.plans.as_ref(), status.clock) else {
            return (None, Vec::new());
        };
        let zone = self.time.as_ref().map_or_else(TimeZone::system, |view| {
            feeder_zone(view, TimeZone::system())
        });

        let next = next_meal(plans, now, &zone).map(|meal| NextMealView {
            at_ms: meal.at.as_millisecond(),
            minute: minute_of(meal.at, &zone),
            portions: meal.portions,
        });
        let meals = plans::today(plans, &self.feeds, now, &zone)
            .into_iter()
            .map(|meal| MealView {
                slot: meal.slot,
                minute: meal.minute,
                portions: meal.portions,
                call: meal.call,
                standing: match meal.state {
                    MealState::Fed(feed) => MealStanding::Fed {
                        result: result(&feed),
                    },
                    MealState::Passed => MealStanding::Passed,
                    MealState::Next { at } => MealStanding::Next {
                        at_ms: at.as_millisecond(),
                    },
                    MealState::Later => MealStanding::Later,
                },
            })
            .collect();
        (next, meals)
    }
}

/// A moment's minute of the local day in `zone`.
fn minute_of(at: Timestamp, zone: &TimeZone) -> u16 {
    let local = at.to_zoned(zone.clone());
    u16::try_from(i32::from(local.hour()) * 60 + i32::from(local.minute())).unwrap_or(0)
}

pub(crate) fn result(fed: &Feed) -> FeedResult {
    let (confirmed, at_most) = fed.delivered.bounds();
    FeedResult {
        feed: fed.feed,
        requested: fed.requested,
        confirmed,
        at_most,
        outcome: fed.outcome,
        reason: fed.reason,
        uncertain: fed.outcome == FeedOutcome::Uncertain,
        words: fed.delivered.describe(fed.requested, fed.outcome),
        at_ms: at_ms(fed.at),
        by: fed.by.map_or(FedBy::Unknown, source_of),
        hopper: fed.hopper,
    }
}

fn at_ms(at: Option<Timestamp>) -> Option<i64> {
    at.map(Timestamp::as_millisecond)
}

fn source_of(source: Source) -> FedBy {
    match source {
        Source::Panel => FedBy::Panel,
        Source::Client { client } => FedBy::Client {
            fingerprint: client.to_string(),
        },
        Source::Feeder => FedBy::Feeder,
        Source::Schedule { plan } => FedBy::Plan { slot: plan },
    }
}

/// One event as the History tab reads it.
pub(crate) fn event_entry(event: &Event) -> HistoryEntry {
    let body = match event.body {
        EventBody::Fed { .. } => {
            Feed::from_event(event).map_or(HistoryBody::Other, |fed| HistoryBody::Fed {
                result: result(&fed),
            })
        }
        EventBody::Missed { plan, due, reason } => HistoryBody::Missed {
            slot: plan,
            due_ms: due.as_millisecond(),
            reason,
        },
        EventBody::Boot { .. } => HistoryBody::Restarted,
        EventBody::Lock { locked, by } => HistoryBody::Lock {
            locked,
            by: source_of(by),
        },
        EventBody::Settings { by, .. } => HistoryBody::SettingsChanged { by: source_of(by) },
        EventBody::Wifi { by, .. } => HistoryBody::WifiChanged { by: source_of(by) },
        EventBody::Plans { by, .. } => HistoryBody::PlansChanged { by: source_of(by) },
        EventBody::Enrolled { client, .. } => HistoryBody::Joined {
            fingerprint: client.to_string(),
        },
        EventBody::Revoked { client, by } => HistoryBody::Revoked {
            fingerprint: client.to_string(),
            by: source_of(by),
        },
        EventBody::ClockSync { by, .. } => HistoryBody::ClockSet { by: source_of(by) },
        EventBody::Lost { count } => HistoryBody::Lost { count },
        EventBody::FeedingBlocked { reason } => HistoryBody::FeedingBlocked { reason },
        EventBody::MealSoon {
            minute, portions, ..
        } => HistoryBody::MealSoon { minute, portions },
        EventBody::Supply { adapter, .. } => HistoryBody::Supply { adapter },
        EventBody::DispenseRefused { reason } => HistoryBody::DispenseRefused { reason },
        EventBody::Food { level } => HistoryBody::Food { level },
    };
    HistoryEntry {
        seq: event.seq,
        at_ms: at_ms(event.at),
        body,
    }
}

/// A gap as the History tab reads it: events the ring dropped unread.
pub(crate) const fn gap_entry(gap: &Gap) -> HistoryEntry {
    HistoryEntry {
        seq: gap.to,
        at_ms: None,
        body: HistoryBody::Gap {
            from: gap.from,
            to: gap.to,
        },
    }
}
