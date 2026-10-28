//! Feeding plans in the loop: which occurrence runs, and how each runs at
//! most once ([schedules]).
//!
//! **Deciding.** With a valid clock, each enabled plan's latest occurrence
//! (`wellfed_core::schedule::latest_due`, worked out once a wall-clock
//! minute) meets the missed-meal rule (`wellfed_core::clock::wall::occurrence`)
//! against the plan's mark: at or before it, nothing; at most 15 minutes
//! late, it runs; later, it is passed and reported missed. Without a valid
//! clock or a zone offset, plans hold: nothing runs and nothing is passed.
//!
//! **Claim, then feed.** An occurrence runs in two steps: the writer first
//! saves a claim (the occurrence, and the journal's high water), and only
//! once that is on NOR is the feed queued, under [`Source::Schedule`]. A
//! cut between the two leaves a claim whose feed was never reserved: the
//! next boot finds the journal's high water unchanged, records the
//! occurrence as missed (`Interrupted`) and never runs it. While a claim
//! is being written, or its feed queued, and until the boot has compared
//! a stored claim with the journal, every other feed is refused
//! ([`Schedule::holds`]), so the claimed feed is the very next one the
//! journal reserves. A feed the owner refuses after its claim gives the
//! occurrence back ([`PlansChange::Release`]); it is claimed again after
//! [`RETRY_MS`], within the grace, so a refusing owner costs a few writes
//! rather than two a pass.
//!
//! **Waiting.** An occurrence that comes due while the feeder cannot feed
//! (another feed, a join, an install, a held battery) waits, and runs as
//! soon as it can within the grace; past it, it is missed, with the reason
//! it waited for. Plans due together run one after another, by slot.
//!
//! [schedules]: ../../../../../docs/firmware/schedules.md

use core::fmt;

use wellfed_core::clock::wall::{Occurrence, occurrence};
use wellfed_core::schedule::latest_due;
use wellfed_device_api::mailbox::{Claim, PLANS, PlansChange, PlansResult, PlansView};
use wellfed_proto::api::{Blocked, EventBody, MissedReason, Readiness, Refusal, Source};

use super::Feeds;
use super::events;
use super::feeding;
use super::platform::Platform;

const MINUTE_MS: u64 = 60_000;
/// How long an occurrence whose claimed feed the owner refused waits
/// before it is claimed again: each try writes the plans twice.
pub const RETRY_MS: u32 = 5_000;

/// Where the schedule stands with the writer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// The boot's stored claim has not been looked at: the plans are
    /// still loading.
    Boot,
    /// The plans hold a claim the journal has not been compared with yet.
    Resolving,
    Idle,
    /// A change that must be written, retried until the mailbox takes it.
    Queue(PlansChange, Expect),
    /// A change the writer has, and what it should leave in the view.
    Wait(Expect),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expect {
    Claimed(Claim),
    Released(Claim),
    /// An interrupted claim settled, reported once it is.
    Settled(Claim),
    /// A missed occurrence passed; reported once it is, if `report`.
    Passed {
        plan: u8,
        due_s: u32,
        report: Option<MissedReason>,
    },
}

/// What the schedule did, for the record lines (`PLSCH1`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    /// An occurrence was taken; its feed is queued once the claim is saved.
    Claimed { plan: u8, due_s: u32 },
    /// The claimed occurrence's feed was queued as `feed`.
    Queued { plan: u8, feed: u32 },
    /// The owner refused the claimed feed; the occurrence is given back.
    Refused { plan: u8, refusal: Refusal },
    Missed {
        plan: u8,
        due_s: u32,
        reason: MissedReason,
    },
    /// The writer dropped the schedule's change: it decides again.
    Dropped,
    /// The meal reminder recorded `MealSoon` for this occurrence.
    Reminded { plan: u8, due_s: u32 },
}

/// One record's body (`PLSCH1`).
pub fn describe(record: Record, f: &mut impl fmt::Write) -> fmt::Result {
    match record {
        Record::Claimed { plan, due_s } => write!(f, "claimed plan={plan} due={due_s}"),
        Record::Queued { plan, feed } => write!(f, "queued plan={plan} feed={feed}"),
        Record::Refused { plan, refusal } => {
            write!(
                f,
                "refused plan={plan} refusal={}",
                feeding::refusal_name(refusal)
            )
        }
        Record::Missed {
            plan,
            due_s,
            reason,
        } => write!(
            f,
            "missed plan={plan} due={due_s} reason={}",
            missed_name(reason)
        ),
        Record::Dropped => f.write_str("dropped"),
        Record::Reminded { plan, due_s } => write!(f, "reminded plan={plan} due={due_s}"),
    }
}

/// The plans' state in the loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Schedule {
    step: Step,
    /// Each slot's latest occurrence (Unix ms), worked out for `computed`.
    dues: [Option<u64>; PLANS],
    /// The wall-clock minute and the table revision `dues` are for.
    computed: Option<(u64, u32)>,
    /// The occurrence held while the feeder cannot feed, and why.
    waiting: Option<(u8, u64, Blocked)>,
    /// No claim before this uptime: the owner refused the last claimed
    /// feed.
    retry_at: Option<u32>,
}

impl Default for Schedule {
    fn default() -> Self {
        Self::new()
    }
}

impl Schedule {
    pub const fn new() -> Self {
        Self {
            step: Step::Boot,
            dues: [None; PLANS],
            computed: None,
            waiting: None,
            retry_at: None,
        }
    }

    /// Whether every other feed must wait: a claim is being written, given
    /// back or settled, so the next feed the journal reserves is the
    /// claim's; or a stored claim has not yet been compared with the
    /// journal, which another feed would move first. Plans still loading
    /// hold nothing, so a store that never loads never stops feeding; the
    /// writer publishes it as it opens, before the feeding owner opens.
    pub fn holds(&self) -> bool {
        match self.step {
            Step::Queue(_, expect) | Step::Wait(expect) => !matches!(expect, Expect::Passed { .. }),
            Step::Resolving => true,
            Step::Boot | Step::Idle => false,
        }
    }

    /// Whether a claim is being written, given back or settled: a meal the
    /// schedule has committed to and whose feed is not yet queued. A
    /// restart waits for it, so the meal is not missed. Unlike
    /// [`Self::holds`], a stored claim still waiting for the journal
    /// (`Resolving`) is not one: with a faulted journal that wait lasts the
    /// whole boot.
    pub fn claim_in_flight(&self) -> bool {
        match self.step {
            Step::Queue(_, expect) | Step::Wait(expect) => !matches!(expect, Expect::Passed { .. }),
            Step::Resolving | Step::Boot | Step::Idle => false,
        }
    }

    /// One pass at `now_ms`; `ready` is the feeder's readiness without the
    /// schedule's own hold.
    pub fn step<P: Platform, F: Feeds>(
        &mut self,
        platform: &mut P,
        feeds: &mut F,
        now_ms: u32,
        ready: Readiness,
        mut record: impl FnMut(Record),
    ) {
        let (view, pending) = match platform.plans() {
            PlansResult::Current { view, pending } => (view, pending),
            // Loading: the boot's claim is still unknown.
            PlansResult::Busy => return,
            // No plans this boot, so no claim to resolve and nothing runs.
            PlansResult::ReadOnly | PlansResult::Unserved => {
                self.step = Step::Idle;
                return;
            }
        };

        match self.step {
            Step::Boot | Step::Resolving => self.boot(&view, feeds),
            Step::Queue(change, expect) => match platform.plans_change(change) {
                PlansResult::Current { pending: true, .. } => self.step = Step::Wait(expect),
                // A client's change holds the slot: ask again next pass.
                PlansResult::Busy => {}
                PlansResult::Current { pending: false, .. }
                | PlansResult::ReadOnly
                | PlansResult::Unserved => self.step = Step::Idle,
            },
            // Ours, or a client's queued behind it.
            Step::Wait(_) if pending => {}
            Step::Wait(expect) => {
                self.settle(expect, &view, platform, feeds, now_ms, ready, &mut record)
            }
            // A client's change is being written: decide on what it leaves.
            Step::Idle if pending => {}
            Step::Idle => self.decide(&view, platform, feeds, now_ms, ready, &mut record),
        }
    }

    /// A claim stored before this boot: its feed was reserved if the
    /// journal moved past the claim's high water, and the journal reported
    /// it then. Otherwise the cut came before the feed was queued.
    fn boot<F: Feeds>(&mut self, view: &PlansView, feeds: &F) {
        let Some(claim) = view.claim else {
            self.step = Step::Idle;
            return;
        };
        let Some(high_water) = feeds.high_water() else {
            self.step = Step::Resolving;
            return;
        };
        self.step = if high_water == claim.high_water {
            Step::Queue(PlansChange::Settle { claim }, Expect::Settled(claim))
        } else {
            Step::Idle
        };
    }

    /// The writer is done with the schedule's change: act on what it left.
    #[expect(clippy::too_many_arguments, reason = "one pass's whole context")]
    fn settle<P: Platform, F: Feeds>(
        &mut self,
        expect: Expect,
        view: &PlansView,
        platform: &mut P,
        feeds: &mut F,
        now_ms: u32,
        ready: Readiness,
        record: &mut impl FnMut(Record),
    ) {
        self.step = Step::Idle;
        match expect {
            Expect::Claimed(claim) => {
                let held = view
                    .claim
                    .filter(|held| held.plan == claim.plan && held.due_s == claim.due_s)
                    .filter(|held| held.high_water == claim.high_water);
                let Some(held) = held else {
                    record(Record::Dropped);
                    return;
                };
                // The plan's portions, even if taking it disabled the plan.
                let Some(Some(plan)) = view.table.get(usize::from(held.plan)) else {
                    self.step =
                        Step::Queue(PlansChange::Release { claim: held }, Expect::Released(held));
                    return;
                };
                let by = Source::Schedule(held.plan);
                match feeding::queue(feeds, ready, plan.portions, by) {
                    Ok(feed) => {
                        self.waiting = None;
                        record(Record::Queued {
                            plan: held.plan,
                            feed,
                        });
                    }
                    Err(refusal) => {
                        record(Record::Refused {
                            plan: held.plan,
                            refusal,
                        });
                        let due = u64::from(held.due_s) * 1000;
                        self.waiting = Some((held.plan, due, blocked(refusal)));
                        self.retry_at = Some(now_ms.wrapping_add(RETRY_MS));
                        self.step = Step::Queue(
                            PlansChange::Release { claim: held },
                            Expect::Released(held),
                        );
                    }
                }
            }
            Expect::Released(_) => {}
            Expect::Settled(claim) => {
                if view.claim.is_none() {
                    let reason = MissedReason::Interrupted;
                    missed(platform, now_ms, claim.plan, claim.due_s, reason, record);
                }
            }
            Expect::Passed {
                plan,
                due_s,
                report,
            } => {
                let passed = view
                    .mark(usize::from(plan))
                    .is_some_and(|mark| mark >= due_s);
                if !passed {
                    record(Record::Dropped);
                    return;
                }
                if self.waiting.is_some_and(|(slot, _, _)| slot == plan) {
                    self.waiting = None;
                }
                if let Some(reason) = report {
                    missed(platform, now_ms, plan, due_s, reason, record);
                }
            }
        }
    }

    /// Run the first occurrence that is due, or pass the first that was
    /// missed.
    fn decide<P: Platform, F: Feeds>(
        &mut self,
        view: &PlansView,
        platform: &mut P,
        feeds: &mut F,
        now_ms: u32,
        ready: Readiness,
        record: &mut impl FnMut(Record),
    ) {
        // Marks must be kept to run anything at most once.
        if !view.writable {
            return;
        }
        let Some(wall_ms) = platform.wall_ms(now_ms) else {
            self.waiting = None;
            self.retry_at = None;
            return;
        };
        self.compute(view, platform, wall_ms);

        let judged: [Option<(usize, u64, Occurrence)>; PLANS] = core::array::from_fn(|slot| {
            let due = self.dues[slot]?;
            let mark = view.mark(slot).map(|s| u64::from(s) * 1000);
            let judged = match occurrence(due, mark, wall_ms) {
                // A plan saved without a valid clock starts from its first
                // occurrence after the clock returns: one before then may
                // have fallen before the save, so it passes, unreported.
                Occurrence::Run if mark.is_none() => Occurrence::Missed,
                judged => judged,
            };
            Some((slot, due, judged))
        });
        let first = |wanted| judged.iter().flatten().find(|j| j.2 == wanted).copied();

        if let Some((slot, due, _)) = first(Occurrence::Run) {
            self.run(view, platform, feeds, (slot, due), now_ms, ready, record);
            return;
        }
        // The refused occurrence is past running: a deadline kept longer
        // would read as ahead again once the uptime wraps half its range.
        self.retry_at = None;
        if let Some((slot, due, _)) = first(Occurrence::Missed) {
            let plan = u8::try_from(slot).unwrap_or(u8::MAX);
            let reason = match self.waiting {
                Some((held, held_due, blocked)) if held == plan && held_due == due => {
                    MissedReason::Blocked(blocked)
                }
                _ => MissedReason::Late,
            };
            // A plan saved without a valid clock has no mark: its first
            // occurrence was never expected, so it passes unreported.
            let expected = view.mark(slot).is_some() || reason != MissedReason::Late;
            let due_s = seconds(due);
            let change = PlansChange::Pass {
                plan,
                due_s,
                revision: view.revision,
            };
            if let PlansResult::Current { pending: true, .. } = platform.plans_change(change) {
                self.step = Step::Wait(Expect::Passed {
                    plan,
                    due_s,
                    report: expected.then_some(reason),
                });
            }
        }
    }

    /// Claim slot `slot`'s occurrence `due` if the feeder can feed now;
    /// otherwise hold it, and why.
    #[expect(clippy::too_many_arguments, reason = "one pass's whole context")]
    fn run<P: Platform, F: Feeds>(
        &mut self,
        view: &PlansView,
        platform: &mut P,
        feeds: &F,
        (slot, due): (usize, u64),
        now_ms: u32,
        ready: Readiness,
        record: &mut impl FnMut(Record),
    ) {
        let plan = u8::try_from(slot).unwrap_or(u8::MAX);
        if let Some(at) = self.retry_at {
            if !wellfed_core::deadline_passed(now_ms, at) {
                return;
            }
            self.retry_at = None;
        }
        let blocked = match ready {
            Readiness::Blocked(blocked) => Some(blocked),
            Readiness::Ready if feeds.busy() => Some(Blocked::Feeding),
            Readiness::Ready => None,
        };
        let high_water = feeds.high_water();
        let (None, Some(high_water)) = (blocked, high_water) else {
            self.waiting = Some((plan, due, blocked.unwrap_or(Blocked::Starting)));
            return;
        };

        let claim = Claim {
            plan,
            due_s: seconds(due),
            high_water,
            disabled: false,
        };
        let change = PlansChange::Claim {
            claim,
            revision: view.revision,
        };
        if let PlansResult::Current { pending: true, .. } = platform.plans_change(change) {
            self.step = Step::Wait(Expect::Claimed(claim));
            record(Record::Claimed {
                plan,
                due_s: claim.due_s,
            });
        }
    }

    /// Each enabled plan's latest occurrence, worked out again only when
    /// the wall-clock minute or the table changes: the zone's rule is
    /// costly to evaluate every pass.
    fn compute<P: Platform>(&mut self, view: &PlansView, platform: &P, wall_ms: u64) {
        let key = (wall_ms / MINUTE_MS, view.revision);
        if self.computed == Some(key) {
            return;
        }
        for (due, plan) in self.dues.iter_mut().zip(&view.table) {
            *due = plan
                .filter(|plan| plan.enabled)
                .and_then(|plan| latest_due(&plan, wall_ms, |ms| platform.offset_at(ms)));
        }
        self.computed = Some(key);
    }

    /// The source a feed's outcome names: the claim's plan for the feed
    /// that followed it, which a restart's uncertain resolution reports as
    /// the feeder's own.
    pub fn source<P: Platform>(platform: &mut P, feed: u32, by: Source) -> Source {
        let PlansResult::Current { view, .. } = platform.plans() else {
            return by;
        };
        match (by, view.claim) {
            (Source::Feeder, Some(claim)) if claim.high_water.wrapping_add(1) == feed => {
                Source::Schedule(claim.plan)
            }
            _ => by,
        }
    }
}

/// Record a missed occurrence: the event and the line.
fn missed<P: Platform>(
    platform: &mut P,
    now_ms: u32,
    plan: u8,
    due_s: u32,
    reason: MissedReason,
    record: &mut impl FnMut(Record),
) {
    events::record(
        platform,
        now_ms,
        EventBody::Missed {
            plan,
            due: due_s,
            reason,
        },
    );
    record(Record::Missed {
        plan,
        due_s,
        reason,
    });
}

/// What a refused claimed feed waited on, for a `Missed` event if the
/// refusals outlast the grace.
const fn blocked(refusal: Refusal) -> Blocked {
    match refusal {
        Refusal::Fault => Blocked::Fault,
        Refusal::NotWritable => Blocked::Journal,
        Refusal::Unavailable => Blocked::Unserved,
        Refusal::Busy
        | Refusal::Forbidden
        | Refusal::Stale { .. }
        | Refusal::Invalid
        | Refusal::Full
        | Refusal::NotFound
        | Refusal::Muted
        | Refusal::Time
        | Refusal::Unverified
        | Refusal::Portions { .. }
        | Refusal::NoClock
        | Refusal::Update(_) => Blocked::Feeding,
    }
}

/// Unix seconds of an occurrence, which is whole seconds (offsets are).
fn seconds(due_ms: u64) -> u32 {
    u32::try_from(due_ms / 1000).unwrap_or(u32::MAX)
}

const fn missed_name(reason: MissedReason) -> &'static str {
    match reason {
        MissedReason::Late => "late",
        MissedReason::Interrupted => "interrupted",
        MissedReason::Blocked(_) => "blocked",
    }
}
