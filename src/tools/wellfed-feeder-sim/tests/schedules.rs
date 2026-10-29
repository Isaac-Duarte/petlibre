//! Feeding plans on the simulator's loop, in virtual time: each occurrence
//! runs at most once through the feeding owner, whatever restarts, clock
//! jumps and daylight changes do around it, and one that cannot run is
//! recorded missed. The meal reminder announces each occurrence ahead of
//! it, and the meal call starts its length ahead of it; neither changes
//! whether or when it runs.

use std::cell::RefCell;
use std::rc::Rc;

use wellfed_core::clock::wall::{Origin, Sync};
use wellfed_core::clock::zone::Zone;
use wellfed_core::schedule::{PLANS, Plan, Table, Weekdays};
use wellfed_core::store::slot::crc32;
use wellfed_device_api::Reader;
use wellfed_device_api::mailbox::{
    CallChange, CallClip, CallResult, MealReminder, PlansChange, PlansResult, PlansView,
    SettingsFields, SettingsResult,
};
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
    /// The meal calls the loop asked of the speaker: the uptime, the plan
    /// and the occurrence.
    calls: Vec<(u32, u8, u32)>,
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
            calls: Vec::new(),
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
            let calls = &mut self.calls;
            let now = self.now;
            let delay = self.sim.pass(self.now, |record| match record {
                Record::Feeder(feeder::Record::Schedule(schedule::Record::Claimed { .. })) => {
                    *claims += 1;
                }
                Record::Feeder(feeder::Record::Schedule(schedule::Record::Called {
                    plan,
                    due_s,
                    ..
                })) => calls.push((now, plan, due_s)),
                _ => {}
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

    /// Upload `stream` as the meal call, a chunk a pass, as a client does.
    fn upload(&mut self, stream: &[u8]) {
        let bytes = u32::try_from(stream.len()).unwrap();
        let crc = crc32(stream);
        let start = self
            .sim
            .platform()
            .call_change(CallChange::Start { bytes, crc });
        assert!(matches!(start, CallResult::Current { pending: true, .. }));
        self.run(1);
        for (i, chunk) in stream.chunks(512).enumerate() {
            let offset = u32::try_from(i * 512).unwrap();
            assert_eq!(self.sim.platform().call_chunk(offset, chunk), Ok(()));
            self.run(1);
        }
        let CallResult::Current { view, .. } = self.sim.platform().call() else {
            panic!("the simulator keeps a meal call");
        };
        assert_eq!(view.clip, CallClip::Stored { bytes, crc });
    }

    /// Every event the ring holds, oldest first.
    fn events(&mut self) -> Vec<EventBody> {
        let mut bodies = Vec::new();
        let mut after = None;
        while let Some(Read::Record(record)) =
            self.sim.platform().event_after(Reader::slot(0), after)
        {
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

    /// Set the meal reminder, as a client's settings write does.
    fn remind(&mut self, reminder: MealReminder) {
        let fields = SettingsFields {
            meal_reminder: Some(reminder),
            ..SettingsFields::default()
        };
        let queued = self.sim.platform().settings_request(fields);
        assert!(matches!(
            queued,
            SettingsResult::Current { pending: true, .. }
        ));
        self.run(1);
        let saved = self.sim.platform().settings().map(|s| s.meal_reminder);
        assert_eq!(saved, Some(reminder));
    }

    /// The `MealSoon` events: when each was recorded, and the plan, due
    /// time, local minute and portions it names.
    fn soon(&mut self) -> Vec<(Option<u32>, u8, u32, u16, u8)> {
        let mut found = Vec::new();
        let mut after = None;
        while let Some(Read::Record(record)) =
            self.sim.platform().event_after(Reader::slot(0), after)
        {
            after = Some(record.seq);
            if let Some(EventBody::MealSoon {
                plan,
                due,
                minute,
                portions,
            }) = wellfed_proto::decode_padded::<EventBody>(&record.body)
            {
                found.push((record.at, plan, due, minute, portions));
            }
        }
        found
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
        call: 0,
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

const BREAKFAST: u64 = WEDNESDAY + 7 * HOUR_MS + 30 * MINUTE_MS;

/// Breakfast with the reminder set `lead` minutes ahead.
fn reminded_breakfast(lead: u8) -> Feeder {
    let mut feeder = breakfast();
    feeder.remind(MealReminder::Before { minutes: lead });
    feeder
}

#[test]
fn the_reminder_is_off_by_default() {
    let mut feeder = breakfast();
    let settings = feeder.sim.platform().settings().unwrap();
    assert_eq!(settings.meal_reminder, MealReminder::Off);

    feeder.run_until(BREAKFAST + MINUTE_MS);
    assert_eq!(feeder.fed().len(), 1);
    assert!(feeder.soon().is_empty());
}

#[test]
fn the_reminder_comes_its_lead_before_the_meal_and_the_meal_runs_once() {
    let mut feeder = reminded_breakfast(10);

    feeder.run_until(BREAKFAST - 10 * MINUTE_MS - 1000);
    assert!(feeder.soon().is_empty(), "nothing before the lead");
    feeder.run(2000);
    let due = seconds(BREAKFAST);
    let at = seconds(BREAKFAST - 10 * MINUTE_MS);
    let reminded = feeder.soon();
    assert_eq!(reminded.len(), 1);
    let (recorded, plan, reminded_due, minute, portions) = reminded[0];
    assert!(recorded.is_some_and(|recorded| recorded.abs_diff(at) <= 1));
    assert_eq!((plan, reminded_due, minute, portions), (0, due, 450, 2));

    // The meal itself, once, after its one reminder.
    feeder.run_until(BREAKFAST + 5 * MINUTE_MS);
    assert_eq!(
        feeder.fed(),
        [(1, 2, FeedOutcome::Done, Source::Schedule(0))]
    );
    assert_eq!(feeder.soon().len(), 1);
    assert_eq!(feeder.claims, 1);
    let order: Vec<bool> = feeder
        .events()
        .into_iter()
        .filter_map(|body| match body {
            EventBody::MealSoon { .. } => Some(true),
            EventBody::Fed { .. } => Some(false),
            _ => None,
        })
        .collect();
    assert_eq!(order, [true, false]);
}

#[test]
fn a_disabled_plan_and_a_feeder_without_time_are_not_reminded() {
    let mut feeder = reminded_breakfast(10);
    let off = Plan {
        enabled: false,
        ..plan(7 * 60 + 30, 2)
    };
    feeder.put(table(&[(0, off)]));
    feeder.run_until(BREAKFAST + MINUTE_MS);
    assert!(feeder.soon().is_empty());
    assert!(feeder.fed().is_empty());

    // A cold boot inside the lead with no time: nothing is reminded.
    let mut feeder = reminded_breakfast(10);
    feeder.run_until(BREAKFAST - 20 * MINUTE_MS);
    feeder.restart();
    feeder.run(15 * MINUTE_MS);
    assert!(feeder.soon().is_empty());
}

#[test]
fn a_plan_edited_inside_the_lead_is_reminded_only_for_a_new_occurrence() {
    // More portions: the same occurrence, already reminded; the meal
    // feeds the new portions.
    let mut feeder = reminded_breakfast(10);
    feeder.run_until(BREAKFAST - 5 * MINUTE_MS);
    assert_eq!(feeder.soon().len(), 1);
    feeder.put(table(&[(0, plan(7 * 60 + 30, 3))]));
    feeder.run_until(BREAKFAST + MINUTE_MS);
    assert_eq!(feeder.soon().len(), 1);
    assert_eq!(
        feeder.fed(),
        [(1, 3, FeedOutcome::Done, Source::Schedule(0))]
    );

    // Moved to 07:28: a new occurrence, already inside the lead, is
    // reminded at once, and runs once; 07:30 is no longer a meal.
    let mut feeder = reminded_breakfast(10);
    feeder.run_until(BREAKFAST - 5 * MINUTE_MS);
    feeder.put(table(&[(0, plan(7 * 60 + 28, 2))]));
    feeder.run(1000);
    let moved = seconds(BREAKFAST - 2 * MINUTE_MS);
    let dues: Vec<(u32, u16)> = feeder
        .soon()
        .into_iter()
        .map(|(_, _, due, minute, _)| (due, minute))
        .collect();
    assert_eq!(dues, [(seconds(BREAKFAST), 450), (moved, 448)]);
    feeder.run_until(BREAKFAST + 5 * MINUTE_MS);
    assert_eq!(
        feeder.fed(),
        [(1, 2, FeedOutcome::Done, Source::Schedule(0))]
    );
    assert_eq!(feeder.claims, 1);
}

#[test]
fn a_restart_inside_the_lead_reminds_the_same_occurrence_again_and_feeds_once() {
    let mut feeder = reminded_breakfast(10);
    feeder.run_until(BREAKFAST - 8 * MINUTE_MS);
    assert_eq!(feeder.soon().len(), 1);

    // A cold boot, synced again inside the lead: the reminder kept nothing
    // on flash, so it names the same occurrence once more.
    feeder.restart();
    feeder.run(100);
    feeder.sync(BREAKFAST - 6 * MINUTE_MS);
    feeder.run(MINUTE_MS);
    let due = seconds(BREAKFAST);
    let named: Vec<(u8, u32)> = feeder
        .soon()
        .into_iter()
        .map(|(_, plan, due, _, _)| (plan, due))
        .collect();
    assert_eq!(named, [(0, due), (0, due)]);

    feeder.run_until(BREAKFAST + 5 * MINUTE_MS);
    assert_eq!(
        feeder.fed(),
        [(1, 2, FeedOutcome::Done, Source::Schedule(0))]
    );
    assert_eq!(feeder.soon().len(), 2);
    assert!(feeder.missed().is_empty());
}

#[test]
fn a_lead_across_a_daylight_change_counts_real_minutes_to_the_occurrence() {
    // 03:10 MDT on March 8 is 09:10Z; 30 minutes before is 01:40 MST, as
    // the clock jumps from 02:00 to 03:00 between them.
    let mut feeder = Feeder::new();
    feeder.run(100);
    feeder.zone("MST7MDT,M3.2.0,M11.1.0");
    feeder.sync(SPRING - 2 * HOUR_MS);
    feeder.remind(MealReminder::Before { minutes: 30 });
    feeder.put(table(&[
        (0, plan(3 * 60 + 10, 1)),
        (1, plan(2 * 60 + 30, 1)),
    ]));
    feeder.run_until(SPRING - 21 * MINUTE_MS);
    assert!(feeder.soon().is_empty());
    feeder.run(2 * MINUTE_MS);
    let first = feeder.soon();
    assert_eq!(first.len(), 1);
    assert_eq!(
        (first[0].1, first[0].2, first[0].3),
        (0, seconds(SPRING + 10 * MINUTE_MS), 3 * 60 + 10)
    );
    // The skipped 02:30 runs at 03:30 MDT, and is reminded at 03:00.
    feeder.run_until(SPRING + MINUTE_MS);
    let skipped: Vec<(u8, u32, u16)> = feeder
        .soon()
        .into_iter()
        .map(|(_, plan, due, minute, _)| (plan, due, minute))
        .collect();
    assert_eq!(
        skipped,
        [
            (0, seconds(SPRING + 10 * MINUTE_MS), 3 * 60 + 10),
            (1, seconds(SPRING + 30 * MINUTE_MS), 3 * 60 + 30),
        ]
    );
    feeder.run_until(SPRING + 40 * MINUTE_MS);
    assert_eq!(feeder.fed().len(), 2);

    // 01:30 happens twice on November 1: one reminder, an hour before the
    // first pass, and none before the second.
    let mut feeder = Feeder::new();
    feeder.run(100);
    feeder.zone("MST7MDT,M3.2.0,M11.1.0");
    feeder.sync(FALL - 3 * HOUR_MS);
    feeder.remind(MealReminder::Before { minutes: 60 });
    feeder.put(table(&[(0, plan(90, 1))]));
    feeder.run_until(FALL + 40 * MINUTE_MS);
    let fall: Vec<(Option<u32>, u32)> = feeder
        .soon()
        .into_iter()
        .map(|(at, _, due, _, _)| (at, due))
        .collect();
    assert_eq!(fall.len(), 1);
    assert_eq!(fall[0].1, seconds(FALL - 30 * MINUTE_MS));
    assert!(
        fall[0]
            .0
            .is_some_and(|at| at.abs_diff(seconds(FALL - 90 * MINUTE_MS)) <= 1)
    );
    assert_eq!(feeder.fed().len(), 1);
}

#[test]
fn a_backward_jump_past_a_meal_already_run_gives_no_reminder() {
    let mut feeder = reminded_breakfast(10);
    feeder.run_until(BREAKFAST + 2 * MINUTE_MS);
    assert_eq!(feeder.soon().len(), 1);
    assert_eq!(feeder.fed().len(), 1);

    // Set back to 07:25: 07:30 is inside the lead again, but its mark
    // says it ran, so it is neither reminded nor fed again.
    feeder.sync(BREAKFAST - 5 * MINUTE_MS);
    feeder.run(10 * MINUTE_MS);
    assert_eq!(feeder.soon().len(), 1);
    assert_eq!(feeder.fed().len(), 1);
}

#[test]
fn a_plan_saved_without_a_clock_is_reminded_once_the_clock_returns() {
    let mut feeder = Feeder::new();
    feeder.run(100);
    feeder.remind(MealReminder::Before { minutes: 30 });
    // Saved with no clock, so with no mark; the clock then reads 07:10,
    // inside the lead. The loop passes yesterday's occurrence, which
    // marks the plan, and today's is reminded.
    feeder.put(table(&[(0, plan(7 * 60 + 30, 2))]));
    feeder.sync(BREAKFAST - 20 * MINUTE_MS);
    feeder.run(MINUTE_MS);
    let reminded: Vec<(u8, u32)> = feeder
        .soon()
        .into_iter()
        .map(|(_, plan, due, _, _)| (plan, due))
        .collect();
    assert_eq!(reminded, [(0, seconds(BREAKFAST))]);

    feeder.run_until(BREAKFAST + MINUTE_MS);
    assert_eq!(feeder.fed().len(), 1);
}

/// A restart asked for while a plan's meal is claimed but not yet fed (a
/// factory reset held in the claim's own pass) waits for the meal: it runs
/// once, before the restart, and the boot after neither runs it again nor
/// records it interrupted.
#[test]
fn a_restart_asked_for_while_a_meal_is_claimed_waits_for_the_meal() {
    use wellfed_core::buttons::{Button, Buttons, Gesture};
    use wellfed_feeder_sim::feeding;

    let held = |button| Gesture::Hold {
        chord: Buttons::of(button),
        level: 1,
    };
    let mut feeder = breakfast();
    feeder.run_until(WEDNESDAY + 7 * HOUR_MS + 29 * MINUTE_MS + 50_000);
    feeder.sim.gesture(held(Button::Lock));
    while feeder.wall().expect("a valid clock") < WEDNESDAY + 7 * HOUR_MS + 30 * MINUTE_MS {
        feeder.run(1);
    }

    // The meal's claim and the reset hold in the same pass.
    feeder.sim.gesture(held(Button::Reset));
    feeder.run(1);
    assert_eq!(feeder.claims, 1, "claimed in the hold's pass");
    let mut rebooted = false;
    for _ in 0..10_000 {
        feeder.run(1);
        if feeder.sim.take_reboot() {
            rebooted = true;
            break;
        }
    }
    assert!(rebooted, "the reset's restart came");
    assert_eq!(
        feeder.fed(),
        [(1, 2, FeedOutcome::Done, Source::Schedule(0))],
        "the meal ran before the restart"
    );
    assert!(feeder.missed().is_empty());

    feeder.restart();
    feeder.run(10_000);
    assert_eq!(feeding::reserved(&feeder.flash), Some(1), "fed once");
    assert!(feeder.fed().is_empty(), "the reset's new history");
    assert!(feeder.missed().is_empty());
}

/// A feeder whose journal will not open keeps a stored claim unresolved
/// for the whole boot (no high water to compare it with). That is no claim
/// in flight: a factory reset's restart and setup mode's still come.
#[test]
fn a_stored_claim_with_a_faulted_journal_holds_no_restart() {
    use wellfed_core::buttons::{Button, Buttons, Gesture};

    let dir = tempfile::tempdir().unwrap();
    let held = |chord| Gesture::Hold { chord, level: 1 };
    // Breakfast runs, which leaves its claim stored, then the journal's
    // sectors are spoiled.
    let faulted = |name: &str| {
        let path = dir.path().join(name);
        let flash = Flash::file(&path).unwrap();
        let banks = Banks::new();
        let sim = open(&flash, &banks, 1);
        let mut feeder = Feeder {
            flash,
            banks,
            sim,
            now: 0,
            boots: 1,
            claims: 0,
            calls: Vec::new(),
        };
        feeder.run(100);
        feeder.sync(WEDNESDAY + 7 * HOUR_MS);
        feeder.put(table(&[(0, plan(7 * 60 + 30, 2))]));
        feeder.run_until(WEDNESDAY + 7 * HOUR_MS + 31 * MINUTE_MS);
        assert_eq!(feeder.fed().len(), 1);
        assert!(feeder.plans().claim.is_some(), "the claim is stored");

        let mut bytes = std::fs::read(&path).unwrap();
        bytes[..0x2000].fill(0);
        std::fs::write(&path, bytes).unwrap();
        feeder.flash = Flash::file(&path).unwrap();
        feeder.restart();
        feeder.run(1_000);
        assert!(feeder.plans().claim.is_some());
        let ready = feeder.sim.platform().plans();
        assert!(matches!(ready, PlansResult::Current { .. }));
        feeder
    };
    let restarts = |feeder: &mut Feeder, ms: u64| {
        let end = u64::from(feeder.now) + ms;
        while u64::from(feeder.now) < end {
            feeder.run(1_000);
            if feeder.sim.take_reboot() {
                return true;
            }
        }
        false
    };

    let mut feeder = faulted("reset.bin");
    feeder.sim.gesture(held(Buttons::of(Button::Lock)));
    feeder.run(100);
    feeder.sim.gesture(held(Buttons::of(Button::Reset)));
    assert!(restarts(&mut feeder, 10_000), "the reset's restart came");

    let mut feeder = faulted("setup.bin");
    feeder.sim.gesture(held(Buttons::of(Button::Lock)));
    feeder.run(100);
    feeder
        .sim
        .gesture(held(Buttons::of(Button::Lock).with(Button::Mic)));
    assert!(
        restarts(&mut feeder, u64::from(feeder::setup::IDLE_MS) + 60_000),
        "setup mode's restart came"
    );
}

/// Breakfast calling twice, with a 2,000-byte clip (500 ms a play): the
/// call starts 2 s ahead of the meal (both plays and the spare second).
fn called_breakfast() -> Feeder {
    let mut feeder = breakfast();
    let stream: Vec<u8> = (0..2_000).map(|i| (i % 251) as u8).collect();
    feeder.upload(&stream);
    let calling = Plan {
        call: 2,
        ..plan(7 * 60 + 30, 2)
    };
    feeder.put(table(&[(0, calling), (1, plan(7 * 60 + 40, 1))]));
    feeder
}

#[test]
fn the_call_starts_its_length_ahead_and_the_meal_runs_on_time_once() {
    let mut feeder = called_breakfast();

    feeder.run_until(BREAKFAST - 3_000);
    assert!(feeder.calls.is_empty(), "nothing before the lead");
    let due_at = feeder.now + 3_000;
    feeder.run(2_500);
    let [(at, plan, due)] = feeder.calls[..] else {
        panic!("one call: {:?}", feeder.calls);
    };
    assert_eq!((plan, due), (0, seconds(BREAKFAST)));
    assert!((due_at - 2_000..=due_at - 1_000).contains(&at), "{at}");

    // The meal runs at its minute, as it would with no call; the plan
    // without a call is fed without one.
    feeder.run_until(BREAKFAST + 15 * MINUTE_MS);
    assert_eq!(
        feeder.fed(),
        [
            (1, 2, FeedOutcome::Done, Source::Schedule(0)),
            (2, 1, FeedOutcome::Done, Source::Schedule(1)),
        ]
    );
    assert_eq!(feeder.calls.len(), 1);
    assert_eq!(feeder.claims, 2);
}

#[test]
fn no_clip_or_a_plan_that_will_not_run_plays_no_call() {
    // A plan with a call but no clip stored.
    let mut feeder = breakfast();
    let calling = Plan {
        call: 3,
        ..plan(7 * 60 + 30, 2)
    };
    feeder.put(table(&[(0, calling)]));
    feeder.run_until(BREAKFAST + MINUTE_MS);
    assert!(feeder.calls.is_empty());
    assert_eq!(feeder.fed().len(), 1);

    // A clip, and the clock set back inside the lead after the meal ran:
    // its mark says it ran, so it is neither called nor fed again.
    feeder.upload(&[0x77; 1_024]);
    feeder.sync(BREAKFAST - 1_000);
    feeder.run(5 * MINUTE_MS);
    assert!(feeder.calls.is_empty());
    assert_eq!(feeder.fed().len(), 1);
}

/// A feeder whose journal will not open runs no meal this boot, so it
/// plays no call for one either.
#[test]
fn a_feeder_that_cannot_feed_this_boot_plays_no_call() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.bin");
    let flash = Flash::file(&path).unwrap();
    let banks = Banks::new();
    let sim = open(&flash, &banks, 1);
    let mut feeder = Feeder {
        flash,
        banks,
        sim,
        now: 0,
        boots: 1,
        claims: 0,
        calls: Vec::new(),
    };
    feeder.run(100);
    feeder.upload(&[0x77; 1_024]);

    // The journal's sectors spoiled, then a cold boot.
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[..0x2000].fill(0);
    std::fs::write(&path, bytes).unwrap();
    feeder.flash = Flash::file(&path).unwrap();
    feeder.restart();
    feeder.run(1_000);
    feeder.sync(WEDNESDAY + 7 * HOUR_MS);
    let calling = Plan {
        call: 2,
        ..plan(7 * 60 + 30, 2)
    };
    feeder.put(table(&[(0, calling)]));
    feeder.run_until(BREAKFAST + MINUTE_MS);
    assert!(feeder.calls.is_empty());
    assert!(feeder.fed().is_empty());
}
