//! The meal reminder: a `MealSoon` event the settings' lead ahead of each
//! scheduled occurrence, for the app, the server and anything else on the
//! event stream ([schedules]).
//!
//! **Read only.** The reminder reads the plans the schedule runs and the
//! settings, and records an event; it never claims, passes or writes a
//! plan, and nothing in the schedule reads what it did. So it cannot
//! delay, take or block a meal, and a meal runs exactly as it would with
//! the reminder off.
//!
//! **Which occurrence.** Each enabled plan's next occurrence
//! (`wellfed_core::schedule::next_due`, the same daylight rules the
//! schedule runs by), worked out once a wall-clock minute, is reminded
//! once the wall clock is within the lead of it and before it is due. A
//! plan with no mark (saved without a valid clock), or whose mark is at or
//! past the occurrence (a backward clock jump), is not reminded: its
//! occurrence may not run. Without a valid clock with an offset, or with a
//! plans store that cannot be written (which runs nothing), nothing is
//! reminded.
//!
//! **Once.** The occurrence last reminded is kept per slot in RAM, never on
//! flash: nothing about a reminder may touch what decides a meal. A plan
//! saved inside the lead is reminded at once; one whose minute or days
//! change gets a new occurrence and so a new reminder, while a change of
//! portions alone keeps the reminder already sent. A restart inside the
//! lead forgets what was sent, so the occurrence is reminded again, with
//! the same plan and due time, which a client can match.
//!
//! [schedules]: ../../../../../docs/firmware/schedules.md

use wellfed_core::schedule::next_due;
use wellfed_device_api::mailbox::{PLANS, PlansResult, PlansView};
use wellfed_proto::api::EventBody;

use super::events;
use super::platform::Platform;
use super::schedule::Record;

const MINUTE_MS: u64 = 60_000;
/// The day in minutes, and a minute in ms, for local time's signed sums.
const DAY_MINUTES: i64 = 24 * 60;
const LOCAL_MINUTE_MS: i64 = 60_000;

/// The reminder's state in the loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reminder {
    /// Each slot's next occurrence (Unix ms), worked out for `computed`.
    next: [Option<u64>; PLANS],
    /// The wall-clock minute and the table revision `next` is for.
    computed: Option<(u64, u32)>,
    /// The occurrence (Unix seconds) each slot was last reminded of.
    sent: [Option<u32>; PLANS],
}

impl Default for Reminder {
    fn default() -> Self {
        Self::new()
    }
}

impl Reminder {
    pub const fn new() -> Self {
        Self {
            next: [None; PLANS],
            computed: None,
            sent: [None; PLANS],
        }
    }

    /// One pass at `now_ms`: remind every occurrence that has come within
    /// the lead and was not reminded yet.
    pub fn step<P: Platform>(
        &mut self,
        platform: &mut P,
        now_ms: u32,
        mut record: impl FnMut(Record),
    ) {
        let Some(lead) = platform.settings().and_then(|s| s.meal_reminder.minutes()) else {
            return;
        };
        let PlansResult::Current { view, .. } = platform.plans() else {
            return;
        };
        // A store the schedule cannot write runs nothing.
        if !view.writable {
            return;
        }
        let Some(wall_ms) = platform.wall_ms(now_ms) else {
            return;
        };
        self.compute(&view, platform, wall_ms);

        let lead_ms = u64::from(lead) * MINUTE_MS;
        let slots = self.next.iter().zip(&view.table).zip(self.sent.iter_mut());
        for (slot, ((&next, plan), sent)) in slots.enumerate() {
            let (Some(due), Some(plan)) = (next, plan.filter(|plan| plan.enabled)) else {
                continue;
            };
            let due_s = u32::try_from(due / 1000).unwrap_or(u32::MAX);
            let runs = view.mark(slot).is_some_and(|mark| mark < due_s);
            let within = wall_ms < due && wall_ms.saturating_add(lead_ms) >= due;
            if !runs || !within || *sent == Some(due_s) {
                continue;
            }

            *sent = Some(due_s);
            let slot = u8::try_from(slot).unwrap_or(u8::MAX);
            let body = EventBody::MealSoon {
                plan: slot,
                due: due_s,
                minute: local_minute(due, platform),
                portions: plan.portions,
            };
            events::record(platform, now_ms, body);
            record(Record::Reminded { plan: slot, due_s });
        }
    }

    /// Each enabled plan's next occurrence, worked out again only when the
    /// wall-clock minute or the table changes, as the schedule's are.
    fn compute<P: Platform>(&mut self, view: &PlansView, platform: &P, wall_ms: u64) {
        let key = (wall_ms / MINUTE_MS, view.revision);
        if self.computed == Some(key) {
            return;
        }
        for (next, plan) in self.next.iter_mut().zip(&view.table) {
            *next = plan
                .filter(|plan| plan.enabled)
                .and_then(|plan| next_due(&plan, wall_ms, |ms| platform.offset_at(ms)));
        }
        self.computed = Some(key);
    }
}

/// The minute of the local day the occurrence `due_ms` falls at: the
/// plan's own minute, or past it when a spring change skipped it.
fn local_minute<P: Platform>(due_ms: u64, platform: &P) -> u16 {
    let offset_ms = i64::from(platform.offset_at(due_ms).unwrap_or(0)) * 1000;
    let local_ms = i64::try_from(due_ms)
        .unwrap_or(i64::MAX)
        .saturating_add(offset_ms);
    let minute = local_ms.div_euclid(LOCAL_MINUTE_MS).rem_euclid(DAY_MINUTES);
    u16::try_from(minute).unwrap_or(0)
}
