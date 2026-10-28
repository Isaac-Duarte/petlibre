//! Feeding plans on the simulator's loop, in virtual time: each occurrence
//! runs at most once through the feeding owner, whatever restarts, clock
//! jumps and daylight changes do around it, and one that cannot run is
//! recorded missed.

use std::cell::RefCell;
use std::rc::Rc;

use wellfed_core::clock::wall::{Origin, Sync};
use wellfed_core::clock::zone::Zone;
use wellfed_core::schedule::{PLANS, Plan, Table, Weekdays};
use wellfed_device_api::mailbox::{PlansChange, PlansResult, PlansView};
use wellfed_feeder::feeder::Platform;
use wellfed_feeder::feeder::{self, schedule};
use wellfed_feeder::store::events::Read;
use wellfed_feeder_sim::banks::Banks;
use wellfed_feeder_sim::flash::Flash;
use wellfed_feeder_sim::sim::{Record, Sim};
use wellfed_proto::api::{Blocked, EventBody, FeedOutcome, MissedReason, Source};

const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
/// 2026-10-28T00:00:00Z, a Wednesday.
const WEDNESDAY: u64 = 1_793_145_600_000;
/// Denver: 2026-03-08 02:00 MST jumps to 03:00 MDT at 09:00Z.
const SPRING: u64 = 1_772_960_400_000;
/// Denver: 2026-11-01 02:00 MDT falls back to 01:00 MST at 08:00Z.
const FALL: u64 = 1_793_520_000_000;

/// A simulated feeder on its own flash, and its uptime.
struct Feeder {
    flash: Rc<Flash>,
    banks: Rc<RefCell<Banks>>,
    sim: Sim,
    now: u32,
    boots: u32,
    /// Occurrences the loop has claimed: each is a write of the plans.
    claims: usize,
}

impl Feeder {
    fn new() -> Self {
        let flash = Flash::memory();
        let banks = Banks::new();
        let sim = open(&flash, &banks, 1);
        Self {
            flash,
            banks,
            sim,
            now: 0,
            boots: 1,
            claims: 0,
        }
    }

    /// A power cut and a cold boot: RAM, the clock and the uptime start
    /// again from the flash.
    fn restart(&mut self) {
        self.boots += 1;
        self.sim = open(&self.flash, &self.banks, self.boots);
        self.now = 0;
    }

    /// An owner's sync: the wall clock reads `unix_ms` now, at UTC unless
    /// a zone rule is set.
    fn sync(&mut self, unix_ms: u64) {
        let sync = Sync {
            unix_ms,
            offset_s: 0,
            next_change_s: 0,
            next_offset_s: 0,
        };
        let now = self.now;
        self.sim
            .platform()
            .time_sync(now, sync, Origin::Owner)
            .unwrap();
    }

    fn zone(&mut self, rule: &str) {
        let rule = Zone::parse(rule.as_bytes()).unwrap().rule();
        self.sim.platform().set_zone_rule(Some(rule));
    }

    fn wall(&mut self) -> Option<u64> {
        let now = self.now;
        self.sim.platform().wall_ms(now)
    }

    /// Run the loop for `ms`, a pass at least every second.
    fn run(&mut self, ms: u64) {
        let end = u64::from(self.now) + ms;
        while u64::from(self.now) < end {
            let claims = &mut self.claims;
            let delay = self.sim.pass(self.now, |record| {
                if let Record::Feeder(feeder::Record::Schedule(schedule::Record::Claimed {
                    ..
                })) = record
                {
                    *claims += 1;
                }
            });
            let step = u64::from(delay.clamp(1, 1000)).min(end - u64::from(self.now));
            self.now += u32::try_from(step.max(1)).unwrap();
        }
    }

    /// Run until the wall clock reads `unix_ms`.
    fn run_until(&mut self, unix_ms: u64) {
        let wall = self.wall().expect("a valid clock");
        assert!(unix_ms >= wall, "the wall clock is already past");
        self.run(unix_ms - wall);
    }

    fn plans(&mut self) -> PlansView {
        let PlansResult::Current { view, .. } = self.sim.platform().plans() else {
            panic!("the simulator keeps plans");
        };
        view
    }

    /// A client's whole table, saved at the wall clock now, as the device
    /// API saves one; written on the next pass.
    fn put(&mut self, table: Table) {
        let at_s = self.wall().map(|ms| u32::try_from(ms / 1000).unwrap());
        let change = PlansChange::Replace { table, at_s };
        let queued = self.sim.platform().plans_change(change);
        assert!(matches!(queued, PlansResult::Current { pending: true, .. }));
        self.run(1);
    }

    /// Every event the ring holds, oldest first.
    fn events(&mut self) -> Vec<EventBody> {
        let mut bodies = Vec::new();
        let mut after = None;
        while let Some(Read::Record(record)) = self.sim.platform().event_after(after) {
            after = Some(record.seq);
            bodies.extend(wellfed_proto::decode_padded::<EventBody>(&record.body));
        }
        bodies
    }

    /// The `Fed` events: feed, requested portions, outcome and source.
    fn fed(&mut self) -> Vec<(u32, u8, FeedOutcome, Source)> {
        self.events()
            .into_iter()
            .filter_map(|body| match body {
                EventBody::Fed {
                    feed,
                    requested,
                    outcome,
                    by,
                    ..
                } => Some((feed, requested, outcome, by)),
                _ => None,
            })
            .collect()
    }

    fn missed(&mut self) -> Vec<(u8, u32, MissedReason)> {
        self.events()
            .into_iter()
            .filter_map(|body| match body {
                EventBody::Missed { plan, due, reason } => Some((plan, due, reason)),
                _ => None,
            })
            .collect()
    }
}

fn open(flash: &Rc<Flash>, banks: &Rc<RefCell<Banks>>, boot: u32) -> Sim {
    let random = Box::new(|| Some([0x5a; 32]));
    Sim::open(flash, banks, None, random, 0, boot, None).unwrap()
}

fn plan(minute: u16, portions: u8) -> Plan {
    Plan {
        minute,
        portions,
        days: Weekdays::EVERY_DAY,
        enabled: true,
    }
}

fn table(plans: &[(usize, Plan)]) -> Table {
    let mut table = [None; PLANS];
    for &(slot, plan) in plans {
        table[slot] = Some(plan);
    }
    table
}

fn seconds(unix_ms: u64) -> u32 {
    u32::try_from(unix_ms / 1000).unwrap()
}

/// A feeder synced to 07:00Z on Wednesday with breakfast at 07:30.
fn breakfast() -> Feeder {
    let mut feeder = Feeder::new();
    feeder.run(100);
    feeder.sync(WEDNESDAY + 7 * HOUR_MS);
    feeder.put(table(&[(0, plan(7 * 60 + 30, 2))]));
    feeder
}

#[test]
fn a_plan_feeds_once_at_its_minute() {
    let mut feeder = breakfast();

    feeder.run_until(WEDNESDAY + 7 * HOUR_MS + 30 * MINUTE_MS - 1000);
    assert!(feeder.fed().is_empty(), "nothing before its minute");

    feeder.run(10 * MINUTE_MS);
    assert_eq!(
        feeder.fed(),
        [(1, 2, FeedOutcome::Done, Source::Schedule(0))]
    );
    // The next day's breakfast is the next feed. An unlearned clock
    // holds about 6.6 hours, so it is synced again first.
    feeder.run(4 * HOUR_MS);
    feeder.sync(WEDNESDAY + 31 * HOUR_MS);
    feeder.run_until(WEDNESDAY + 31 * HOUR_MS + 31 * MINUTE_MS);
    assert_eq!(feeder.fed().len(), 2);
    assert!(feeder.missed().is_empty());
}

#[test]
fn a_restart_and_a_backward_jump_never_run_it_twice() {
    let mut feeder = breakfast();
    feeder.run_until(WEDNESDAY + 7 * HOUR_MS + 31 * MINUTE_MS);
    assert_eq!(feeder.fed().len(), 1);

    // A cold boot within the grace, synced again: already run.
    feeder.restart();
    feeder.run(100);
    feeder.sync(WEDNESDAY + 7 * HOUR_MS + 32 * MINUTE_MS);
    feeder.run(2 * MINUTE_MS);
    // And the clock set back before the meal: still run once.
    feeder.sync(WEDNESDAY + 7 * HOUR_MS + 29 * MINUTE_MS);
    feeder.run(5 * MINUTE_MS);

    assert_eq!(feeder.fed().len(), 1);
    assert!(feeder.missed().is_empty());
}

#[test]
fn a_forward_jump_runs_a_meal_within_the_grace_and_misses_one_past_it() {
    let mut feeder = breakfast();
    feeder.put(table(&[
        (0, plan(7 * 60 + 30, 2)),
        (1, plan(7 * 60 + 10, 1)),
    ]));

    // From 07:00 to 07:40: breakfast is 10 minutes late and runs; the
    // 07:10 meal is 30 minutes late and is missed.
    feeder.sync(WEDNESDAY + 7 * HOUR_MS + 40 * MINUTE_MS);
    feeder.run(MINUTE_MS);

    assert_eq!(
        feeder.fed(),
        [(1, 2, FeedOutcome::Done, Source::Schedule(0))]
    );
    let due = seconds(WEDNESDAY + 7 * HOUR_MS + 10 * MINUTE_MS);
    assert_eq!(feeder.missed(), [(1, due, MissedReason::Late)]);
}

#[test]
fn without_a_valid_clock_plans_hold_and_a_cold_boot_misses_what_fell_meanwhile() {
    let mut feeder = breakfast();
    feeder.restart();
    feeder.run(2 * HOUR_MS);
    assert!(feeder.fed().is_empty(), "no meal runs on an invalid clock");
    assert!(feeder.missed().is_empty());

    // Synced at 09:00: breakfast fell 90 minutes ago while there was no
    // time, and is reported, never run.
    feeder.sync(WEDNESDAY + 9 * HOUR_MS);
    feeder.run(MINUTE_MS);
    assert!(feeder.fed().is_empty());
    let due = seconds(WEDNESDAY + 7 * HOUR_MS + 30 * MINUTE_MS);
    assert_eq!(feeder.missed(), [(0, due, MissedReason::Late)]);
}

#[test]
fn plans_due_together_run_one_after_another() {
    let mut feeder = breakfast();
    feeder.put(table(&[
        (0, plan(7 * 60 + 30, 2)),
        (3, plan(7 * 60 + 30, 1)),
    ]));

    feeder.run_until(WEDNESDAY + 7 * HOUR_MS + 35 * MINUTE_MS);
    assert_eq!(
        feeder.fed(),
        [
            (1, 2, FeedOutcome::Done, Source::Schedule(0)),
            (2, 1, FeedOutcome::Done, Source::Schedule(3)),
        ]
    );
}

#[test]
fn a_meal_due_during_a_long_feed_waits_and_is_missed_past_the_grace() {
    let mut feeder = breakfast();
    feeder.put(table(&[
        (0, plan(7 * 60 + 30, 2)),
        (1, plan(7 * 60 + 31, 1)),
    ]));
    // The first meal's motor stops moving: the second waits for it.
    feeder.run_until(WEDNESDAY + 7 * HOUR_MS + 30 * MINUTE_MS + 100);
    feeder.sim.stall_feeds(true);
    feeder.run(20 * MINUTE_MS);
    feeder.sim.stall_feeds(false);
    feeder.run(MINUTE_MS);

    let due = seconds(WEDNESDAY + 7 * HOUR_MS + 31 * MINUTE_MS);
    assert_eq!(
        feeder.missed(),
        [(1, due, MissedReason::Blocked(Blocked::Feeding))]
    );
    assert_eq!(feeder.fed().len(), 1, "only the first meal fed");
}

#[test]
fn a_cut_between_the_claim_and_the_feed_is_missed_and_never_fed() {
    let mut feeder = breakfast();
    feeder.run_until(WEDNESDAY + 7 * HOUR_MS + 30 * MINUTE_MS - 500);
    // Step until the claim is on flash, then cut before the next pass
    // queues its feed.
    while feeder.plans().claim.is_none() {
        feeder.run(1);
    }
    let claim = feeder.plans().claim.unwrap();
    assert_eq!(claim.high_water, 0);
    feeder.restart();
    feeder.run(100);
    feeder.sync(WEDNESDAY + 7 * HOUR_MS + 31 * MINUTE_MS);
    feeder.run(2 * MINUTE_MS);

    assert!(feeder.fed().is_empty(), "the claimed feed never ran");
    assert_eq!(
        feeder.missed(),
        [(0, claim.due_s, MissedReason::Interrupted)]
    );
    assert_eq!(feeder.plans().claim, None);
}

#[test]
fn a_cut_mid_feed_resolves_as_the_plans_uncertain_feed_and_never_runs_again() {
    let mut feeder = breakfast();
    feeder.run_until(WEDNESDAY + 7 * HOUR_MS + 30 * MINUTE_MS + 100);
    feeder.sim.stall_feeds(true);
    feeder.run(2000);
    feeder.restart();
    feeder.run(100);
    feeder.sync(WEDNESDAY + 7 * HOUR_MS + 32 * MINUTE_MS);
    feeder.run(2 * MINUTE_MS);

    assert_eq!(
        feeder.fed(),
        [(1, 2, FeedOutcome::Uncertain, Source::Schedule(0))]
    );
    assert!(feeder.missed().is_empty());
}

#[test]
fn a_plan_that_runs_once_disables_itself() {
    let mut feeder = breakfast();
    let once = Plan {
        days: Weekdays::ONCE,
        ..plan(7 * 60 + 30, 1)
    };
    feeder.put(table(&[(0, once)]));
    let revision = feeder.plans().revision;

    feeder.run_until(WEDNESDAY + 7 * HOUR_MS + 40 * MINUTE_MS);
    feeder.sync(WEDNESDAY + 31 * HOUR_MS);
    feeder.run(40 * MINUTE_MS);
    assert_eq!(feeder.fed().len(), 1, "once, not again the next day");
    let view = feeder.plans();
    assert_eq!(view.table[0].map(|plan| plan.enabled), Some(false));
    assert_ne!(view.revision, revision, "a client's base moves on");
}

#[test]
fn a_plan_saved_after_its_minute_waits_for_the_next_day() {
    let mut feeder = Feeder::new();
    feeder.run(100);
    feeder.sync(WEDNESDAY + 7 * HOUR_MS + 35 * MINUTE_MS);
    feeder.put(table(&[(0, plan(7 * 60 + 30, 2))]));
    feeder.run(10 * MINUTE_MS);

    assert!(
        feeder.fed().is_empty(),
        "five minutes late, but saved after"
    );
    assert!(feeder.missed().is_empty());
}

#[test]
fn a_skipped_minute_runs_past_the_spring_change_and_a_repeated_one_once() {
    let mut feeder = Feeder::new();
    feeder.run(100);
    feeder.zone("MST7MDT,M3.2.0,M11.1.0");
    // 02:30 does not exist on March 8: it runs at 03:30 MDT, 09:30Z.
    feeder.sync(SPRING - HOUR_MS);
    feeder.put(table(&[(0, plan(2 * 60 + 30, 1))]));
    feeder.run_until(SPRING + 29 * MINUTE_MS);
    assert!(feeder.fed().is_empty());
    feeder.run(2 * MINUTE_MS);
    assert_eq!(feeder.fed().len(), 1);

    // 01:30 happens twice on November 1: it runs on the first pass only.
    let mut feeder = Feeder::new();
    feeder.run(100);
    feeder.zone("MST7MDT,M3.2.0,M11.1.0");
    feeder.sync(FALL - 2 * HOUR_MS);
    feeder.put(table(&[(0, plan(90, 1))]));
    feeder.run_until(FALL + 40 * MINUTE_MS);
    assert_eq!(feeder.fed().len(), 1);
    assert!(feeder.missed().is_empty());
}

#[test]
fn a_plan_that_ran_once_and_is_enabled_again_runs_at_its_next_occurrence() {
    let mut feeder = breakfast();
    let once = Plan {
        days: Weekdays::ONCE,
        ..plan(7 * 60 + 30, 1)
    };
    feeder.put(table(&[(0, once)]));
    feeder.run_until(WEDNESDAY + 7 * HOUR_MS + 40 * MINUTE_MS);
    assert_eq!(feeder.fed().len(), 1);

    // Enabled again at noon: today's 07:30 is behind the save, so it is
    // neither run nor missed; tomorrow's runs.
    feeder.run_until(WEDNESDAY + 12 * HOUR_MS);
    feeder.put(table(&[(0, once)]));
    feeder.run(MINUTE_MS);
    feeder.sync(WEDNESDAY + 31 * HOUR_MS);
    feeder.run(40 * MINUTE_MS);

    assert_eq!(feeder.fed().len(), 2);
    assert!(feeder.missed().is_empty());
    assert_eq!(
        feeder.plans().table[0].map(|plan| plan.enabled),
        Some(false)
    );
}

#[test]
fn a_refused_feed_gives_its_occurrence_back_and_retries_without_spinning() {
    let mut feeder = breakfast();
    feeder.sim.refuse_feeds(true);
    feeder.run_until(WEDNESDAY + 7 * HOUR_MS + 30 * MINUTE_MS + 20_000);
    assert_eq!(feeder.plans().claim, None, "every claim was given back");
    let retries = schedule::RETRY_MS / 1000;
    assert!(
        feeder.claims <= usize::try_from(20 / retries + 1).unwrap(),
        "claimed {} times in 20 s",
        feeder.claims
    );
    feeder.sim.refuse_feeds(false);
    feeder.run(10_000);

    assert_eq!(
        feeder.fed(),
        [(1, 2, FeedOutcome::Done, Source::Schedule(0))]
    );

    // Refused past the grace, it is missed, naming what blocked it.
    let mut feeder = breakfast();
    feeder.sim.refuse_feeds(true);
    feeder.run_until(WEDNESDAY + 7 * HOUR_MS + 50 * MINUTE_MS);
    let due = seconds(WEDNESDAY + 7 * HOUR_MS + 30 * MINUTE_MS);
    assert_eq!(
        feeder.missed(),
        [(0, due, MissedReason::Blocked(Blocked::Feeding))]
    );
    assert!(feeder.fed().is_empty());
}

#[test]
fn a_plan_saved_without_a_clock_starts_after_the_clock_returns() {
    let mut feeder = Feeder::new();
    feeder.run(100);
    // Saved with no clock; the clock then reads 07:32, two minutes past
    // the plan's minute: that occurrence may have fallen before the save.
    feeder.put(table(&[(0, plan(7 * 60 + 30, 2))]));
    feeder.sync(WEDNESDAY + 7 * HOUR_MS + 32 * MINUTE_MS);
    feeder.run(5 * MINUTE_MS);
    assert!(feeder.fed().is_empty());
    assert!(feeder.missed().is_empty());

    feeder.sync(WEDNESDAY + 31 * HOUR_MS + 29 * MINUTE_MS);
    feeder.run(2 * MINUTE_MS);
    assert_eq!(feeder.fed().len(), 1);
}

#[test]
fn a_plan_that_runs_once_saved_without_a_clock_feeds_at_its_minute() {
    let mut feeder = Feeder::new();
    feeder.run(100);
    let once = Plan {
        days: Weekdays::ONCE,
        ..plan(7 * 60 + 30, 1)
    };
    feeder.put(table(&[(0, once)]));
    // Synced before its minute: yesterday's occurrence passes unexpected,
    // and today's runs, once.
    feeder.sync(WEDNESDAY + 7 * HOUR_MS + 10 * MINUTE_MS);
    feeder.run(25 * MINUTE_MS);

    assert_eq!(feeder.fed().len(), 1);
    assert!(feeder.missed().is_empty());
    assert_eq!(
        feeder.plans().table[0].map(|plan| plan.enabled),
        Some(false)
    );
}
