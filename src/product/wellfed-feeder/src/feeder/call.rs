//! The meal call: the owner's clip, played before each scheduled meal
//! whose plan asks for it ([schedules]).
//!
//! **Before, never in the way.** Stock played its call and then ran the
//! motor, so the call pushed the meal back by its length. Here the call
//! starts its length (every play, plus [`SPARE_MS`]) ahead of the meal's
//! instant, so it ends as the meal is due and the meal runs on time. Like
//! the reminder it only reads the plans and asks the speaker; it never
//! claims, passes, delays or blocks a meal, and a call that cannot play
//! (sound off, the speaker busy, no clip) is simply not heard.
//!
//! **Which occurrence.** Each enabled plan with a call, at its next
//! occurrence (`wellfed_core::schedule::next_due`, worked out once a
//! wall-clock minute), once the wall clock is within the call's lead and
//! before the meal is due. A plan whose mark is at or past the occurrence,
//! or with none, is skipped, as the reminder skips it: its meal may not
//! run. With no valid clock, a plans store that cannot be written, no clip,
//! a feeding owner stopped on a fault or a journal that did not open (no
//! meal runs this boot), or BLE setup mode running (whose own tunes use
//! the speaker), no call plays.
//!
//! **Once.** The occurrence last called is kept per slot in RAM. A speaker
//! busy with something else is asked again each pass until the meal is
//! due; after that the call is dropped for that meal. A restart inside the
//! lead plays it again, which costs nothing but a repeat.
//!
//! [schedules]: ../../../../../docs/firmware/schedules.md

use wellfed_core::clip;
use wellfed_core::schedule::next_due;
use wellfed_device_api::mailbox::{
    CallClip, CallResult, PLANS, PlansResult, PlansView, PlayResult,
};

use super::platform::Platform;
use super::schedule::Record;

const MINUTE_MS: u64 = 60_000;
/// Beyond the clip's own length: the amplifier's settle before the first
/// sample, the ring's drain and a loop pass or two, so the call ends
/// before the meal is due.
pub const SPARE_MS: u32 = 1_000;

/// How far ahead of its meal a call of a `bytes` clip played `plays`
/// times starts.
pub const fn lead_ms(bytes: u32, plays: u8) -> u32 {
    clip::ms(bytes)
        .saturating_mul(plays as u32)
        .saturating_add(SPARE_MS)
}

/// The longest lead, every play of the longest clip, is under a minute: a
/// meal's next occurrence, worked out once a minute, is the one to call.
const _: () = assert!((lead_ms(clip::BYTES_MAX, clip::PLAYS_MAX) as u64) < MINUTE_MS);

/// The call's state in the loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Call {
    /// Each slot's next occurrence (Unix ms), worked out for `computed`.
    next: [Option<u64>; PLANS],
    /// The wall-clock minute and the table revision `next` is for.
    computed: Option<(u64, u32)>,
    /// The occurrence (Unix seconds) each slot's call last started for, or
    /// was found muted for.
    called: [Option<u32>; PLANS],
    /// The occurrence each slot's call last found the speaker busy for,
    /// so the retries record nothing more.
    busy: [Option<u32>; PLANS],
}

impl Default for Call {
    fn default() -> Self {
        Self::new()
    }
}

impl Call {
    pub const fn new() -> Self {
        Self {
            next: [None; PLANS],
            computed: None,
            called: [None; PLANS],
            busy: [None; PLANS],
        }
    }

    /// One pass at `now_ms`: start the call of every occurrence whose lead
    /// has begun, unless no meal can run or the speaker is setup mode's
    /// (`quiet`). Out of line, so its
    /// copy of the plans never shares the loop's deepest frame.
    #[inline(never)]
    pub fn step<P: Platform>(
        &mut self,
        platform: &mut P,
        now_ms: u32,
        quiet: bool,
        mut record: impl FnMut(Record),
    ) {
        if quiet {
            return;
        }
        let CallResult::Current {
            view:
                wellfed_device_api::mailbox::CallView {
                    clip: CallClip::Stored { bytes, .. },
                    ..
                },
            ..
        } = platform.call()
        else {
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

        for slot in 0..PLANS {
            let (Some(due), Some(Some(plan))) = (self.next[slot], view.table.get(slot)) else {
                continue;
            };
            if !plan.enabled || plan.call == 0 {
                continue;
            }
            let due_s = u32::try_from(due / 1000).unwrap_or(u32::MAX);
            let runs = view.mark(slot).is_some_and(|mark| mark < due_s);
            let lead = u64::from(lead_ms(bytes, plan.call));
            let within = wall_ms < due && wall_ms.saturating_add(lead) >= due;
            if !runs || !within || self.called[slot] == Some(due_s) {
                continue;
            }

            let result = platform.play_call(plan.call);
            let plan_slot = u8::try_from(slot).unwrap_or(u8::MAX);
            match result {
                PlayResult::Started | PlayResult::Muted => self.called[slot] = Some(due_s),
                // Asked again next pass; recorded once.
                PlayResult::Busy if self.busy[slot] == Some(due_s) => continue,
                PlayResult::Busy => self.busy[slot] = Some(due_s),
            }
            record(Record::Called {
                plan: plan_slot,
                due_s,
                result,
            });
        }
    }

    /// Each enabled plan's next occurrence, worked out again only when the
    /// wall-clock minute or the table changes.
    fn compute<P: Platform>(&mut self, view: &PlansView, platform: &P, wall_ms: u64) {
        let key = (wall_ms / MINUTE_MS, view.revision);
        if self.computed == Some(key) {
            return;
        }
        for (next, plan) in self.next.iter_mut().zip(&view.table) {
            *next = plan
                .filter(|plan| plan.enabled && plan.call != 0)
                .and_then(|plan| next_due(&plan, wall_ms, |ms| platform.offset_at(ms)));
        }
        self.computed = Some(key);
    }
}
