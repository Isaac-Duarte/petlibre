//! When the feeder reads its food beam with no feed running.
//!
//! A feed reads the beam as a side effect, but a feeder left alone never
//! feeds its way out of a stale level: a refill would stay "low" and a
//! restart "unread" until the next meal. So the loop also lights the emitters
//! for a short window at rest: once soon after boot, then every
//! [`FOOD_READ_EVERY_MS`].
//!
//! This is the schedule alone. The owner lights the emitters and judges the
//! window (`wellfed_hal::owner::Owner::read_food`), and the interlock keeps
//! storage writes off it (`wellfed_hal::interlock`). `held` here gathers every
//! reason a reading must not run now, because a reading always gives way: it
//! never waits for anything itself, and whatever stopped it costs only a try
//! after [`FOOD_READ_RETRY_MS`].

use crate::deadline_passed;

/// How long a reading lights the emitters: the sensor filter's 100 ms of
/// settling and 1 s of continuous readings without food, and four 20 ms
/// samples for the first one's lag. Food shows on its first settled sample,
/// but the window is the same either way, so one stray sample is overruled
/// and the level does not depend on which way it read.
pub const FOOD_READ_MS: u32 = 1_200;

/// The supervisor's limit on one window, which the owner's own end should
/// always beat by a few samples: past it the timer interrupt cuts the
/// emitters, whatever the loop is doing.
pub const FOOD_READ_BUDGET_MS: u32 = FOOD_READ_MS + 200;

/// The longest a Wi-Fi join waits for a reading: the budget, and room for a
/// held-off loop to see that the supervisor cut it.
pub const FOOD_READ_BOUND_MS: u32 = FOOD_READ_BUDGET_MS + 1_000;

/// How long after the owner is first free to read that the boot's reading
/// comes: past the first flurry of store writes (the `Boot` event, the
/// settings' load), which would only make it give way.
pub const FOOD_READ_FIRST_MS: u32 = 5_000;

/// Between readings at rest. A refill shows within this long, and the
/// emitters' duty stays at 0.2 %.
pub const FOOD_READ_EVERY_MS: u32 = 10 * 60_000;

/// Until the next try after a reading that gave way or settled on nothing.
/// Longer than the confirmation's 30 s of outputs verified stopped
/// (`wellfed_hal::ota::confirm::HEALTHY_MS`, which asserts this), or windows
/// that never settle would restart that wait for ever. The worst duty, every
/// window retried, is 1.2 s in 36.2 s: about 3.3 %.
pub const FOOD_READ_RETRY_MS: u32 = 35_000;

/// What a step of the schedule says to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoodStep {
    /// Nothing changes.
    Rest,
    /// A reading is due and nothing holds it.
    Start,
    /// A reading runs and something now holds it: end it, unjudged.
    Yield,
}

/// The owner's side of a reading, for the schedule.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FoodView {
    /// A reading's emitters are lit.
    pub reading: bool,
    /// Readings the owner has judged this boot.
    pub reads: u32,
}

/// The schedule; one per owner, stepped by the task that steps the owner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FoodReads {
    /// When the next reading is due, once the first has been scheduled.
    due: Option<u32>,
    /// A reading started and its end has not been seen.
    running: bool,
    /// [`FoodView::reads`] when the last reading's end was seen.
    reads: u32,
}

impl FoodReads {
    pub const fn new() -> Self {
        Self {
            due: None,
            running: false,
            reads: 0,
        }
    }

    /// One step at `now_ms`, with whatever holds readings back as `held`.
    pub fn step(&mut self, now_ms: u32, view: FoodView, held: bool) -> FoodStep {
        if view.reading {
            self.running = true;
            return if held {
                FoodStep::Yield
            } else {
                FoodStep::Rest
            };
        }

        // Judged, the level is fresh for a whole interval. Otherwise the
        // feeder may be showing a stale level, so it tries again soon.
        if core::mem::take(&mut self.running) {
            let wait = if view.reads == self.reads {
                FOOD_READ_RETRY_MS
            } else {
                FOOD_READ_EVERY_MS
            };
            self.reads = view.reads;
            self.due = Some(now_ms.wrapping_add(wait));
        }

        // The first is timed from when the feeder is first free to read, so a
        // trial's reading waits for its confirmation and then some.
        if held {
            // Kept from lying half a clock wrap behind, where it would read
            // as not due for weeks after a long hold.
            if self.due.is_some_and(|due| deadline_passed(now_ms, due)) {
                self.due = Some(now_ms);
            }
            return FoodStep::Rest;
        }
        let due = *self
            .due
            .get_or_insert(now_ms.wrapping_add(FOOD_READ_FIRST_MS));
        if deadline_passed(now_ms, due) {
            FoodStep::Start
        } else {
            FoodStep::Rest
        }
    }

    /// The reading the last step asked for has lit: its end, even one that
    /// comes before the next step, is scheduled from.
    pub fn started(&mut self) {
        self.running = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDLE: FoodView = FoodView {
        reading: false,
        reads: 0,
    };

    #[test]
    fn the_first_reading_comes_soon_after_the_first_free_step() {
        let mut reads = FoodReads::new();
        assert_eq!(reads.step(100, IDLE, true), FoodStep::Rest, "held");
        assert_eq!(reads.step(90_000, IDLE, true), FoodStep::Rest, "held");
        assert_eq!(reads.step(100_000, IDLE, false), FoodStep::Rest);
        let due = 100_000 + FOOD_READ_FIRST_MS;
        assert_eq!(reads.step(due - 1, IDLE, false), FoodStep::Rest);
        assert_eq!(reads.step(due, IDLE, false), FoodStep::Start);
        assert_eq!(reads.step(due + 20, IDLE, false), FoodStep::Start, "again");
    }

    #[test]
    fn a_judged_reading_waits_the_interval_and_any_other_the_retry() {
        let mut reads = FoodReads::new();
        assert_eq!(reads.step(0, IDLE, false), FoodStep::Rest);
        assert_eq!(reads.step(FOOD_READ_FIRST_MS, IDLE, false), FoodStep::Start);
        reads.started();

        // Judged: it counted.
        let judged = FoodView {
            reading: false,
            reads: 1,
        };
        assert_eq!(reads.step(10_000, judged, false), FoodStep::Rest);
        let due = 10_000 + FOOD_READ_EVERY_MS;
        assert_eq!(reads.step(due - 1, judged, false), FoodStep::Rest);
        assert_eq!(reads.step(due, judged, false), FoodStep::Start);
        reads.started();

        // Unjudged: it did not.
        assert_eq!(reads.step(due + 1_000, judged, false), FoodStep::Rest);
        let retry = due + 1_000 + FOOD_READ_RETRY_MS;
        assert_eq!(reads.step(retry - 1, judged, false), FoodStep::Rest);
        assert_eq!(reads.step(retry, judged, false), FoodStep::Start);
    }

    #[test]
    fn a_reading_yields_to_whatever_comes_to_hold_it() {
        let mut reads = FoodReads::new();
        let lit = FoodView {
            reading: true,
            reads: 0,
        };
        assert_eq!(reads.step(0, lit, false), FoodStep::Rest);
        assert_eq!(reads.step(20, lit, true), FoodStep::Yield);

        // Its end is seen though the hold lasts, and the retry is timed from it.
        assert_eq!(reads.step(40, IDLE, true), FoodStep::Rest);
        assert_eq!(
            reads.step(40 + FOOD_READ_RETRY_MS - 1, IDLE, false),
            FoodStep::Rest
        );
        assert_eq!(
            reads.step(40 + FOOD_READ_RETRY_MS, IDLE, false),
            FoodStep::Start
        );
    }

    /// A hold longer than half the clock's range still reads as due at its end.
    #[test]
    fn a_long_hold_leaves_the_reading_due() {
        let mut reads = FoodReads::new();
        assert_eq!(reads.step(0, IDLE, false), FoodStep::Rest);
        let mut now = 0u32;
        for _ in 0..1_000 {
            now = now.wrapping_add(10_000_000);
            assert_eq!(reads.step(now, IDLE, true), FoodStep::Rest);
        }
        assert_eq!(reads.step(now, IDLE, false), FoodStep::Start);
    }
}
