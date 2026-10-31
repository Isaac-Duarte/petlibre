//! The simulator's feeding owner: no motor, but the feeder's own feed
//! journal (`wellfed_core::store::feed_journal`) on the storage region's
//! journal sectors, so feed numbers, reservations and the resolution of an
//! interrupted feed behave as the firmware's do.
//!
//! - A feed is queued under the next journal id, which is its number, and
//!   reserved durably in the journal at the owner's next step, as the
//!   firmware's owner does it on its own turn: the device API answers
//!   `Feeding` only once it is reserved, so a session that ends in between
//!   leaves a feed that runs with no answer sent. Numbers keep counting
//!   across restarts, as the journal's high water does.
//! - It then "drives" for [`PORTION_MS`] per portion of the loop's time, and
//!   its result goes into the journal before its `Fed` event is recorded.
//!   A cancel ends it at the next step with the portions driven so far.
//! - A restart while a feed is reserved leaves the reservation pending: the
//!   next boot resolves it as uncertain under its own number, records that
//!   `Fed` event (by the feeder) and refuses feeds as `Resolving` until it
//!   has. The firmware's owner resolves it once it has opened, some time
//!   into the boot, and its loop queues the event for the settings writer;
//!   the simulator resolves it as it opens and records the event at its
//!   first step, so a client sees `resolving` only briefly. A feed queued
//!   but not yet reserved is lost with the restart, as on the feeder: the
//!   motor never ran, and no `Fed` event names it.
//! - The pack's floor is the firmware's (`wellfed_core::supply::Floor`
//!   against `wellfed_hal::power::FLOOR`), judged on every step from the
//!   simulated supply: on the pack alone, a feed is refused `Battery` until
//!   the pack is judged above it. A feed already running is not stopped by
//!   a sag; the firmware's owner and its plant (`wellfed-sim`) cover that.
//! - [`SimFeeds::stall`] holds a feed in flight where it is, as a motor
//!   that stopped moving would, so a test can act while a feed runs.
//! - [`SimFeeds::fault`] makes the next feed end on a fault once it has
//!   driven a portion's time, as the firmware's owner ends it: a blocked
//!   outlet or a jam no reversal freed, `Failed` with one portion delivered
//!   (none for a feed of one, which ends before its mark), never resumed
//!   or retried.
//! - The hopper has food until [`SimFeeds::fill`] empties it. Each feed
//!   reads it as the food beam would, runs whatever it reads (as the
//!   firmware's and stock's do) and records the reading in its `Fed`
//!   event; the reading stays the feeder's food level until the next one.
//!   With no feed the beam is read on the firmware's own schedule
//!   ([`FoodReads`]), on the adapter only, each reading taking no time here.

use std::cell::Cell;
use std::rc::Rc;

use wellfed_board_plaf103::storage;
use wellfed_core::feed::controller::{FeedReason, FeedRequest};
use wellfed_core::feed::food_read::{FoodReads, FoodStep, FoodView};
use wellfed_core::feed::progress::{BowlMode, Direction};
use wellfed_core::feed::storage::{OwnerOutcome, OwnerRecovery, OwnerResult};
use wellfed_core::store::feed_journal::{FeedJournal, JournalConfig, JournalError};
use wellfed_core::supply::Floor;
use wellfed_feeder::feed::{self, Durability};
use wellfed_feeder::feeder::Feeds;
use wellfed_proto::api::{Blocked, EventBody, PORTIONS_MAX, Refusal, Source};

use crate::flash::{Flash, Region};
use crate::platform::SimSupply;

/// How long one portion takes to drive.
pub const PORTION_MS: u32 = 200;

/// The simulator's journal: its own binding (nonzero, never a real
/// board's), the firmware's bowl and per-feed cap.
const JOURNAL: JournalConfig = JournalConfig {
    binding: 0x5349_4d5f_4645_4544,
    mode: BowlMode::Single,
    max_portions: PORTIONS_MAX as u16,
};

/// The feed the owner holds, from its reservation to its result.
#[derive(Debug)]
struct Current {
    feed: u64,
    portions: u16,
    by: Source,
    /// Written to the journal: it drives, and the device API answers.
    reserved: bool,
    /// How long it has driven so far.
    driven_ms: u32,
    /// When it was last stepped.
    stepped_ms: Option<u32>,
    cancelled: Cell<bool>,
}

#[derive(Debug)]
pub struct SimFeeds {
    journal: FeedJournal<Region>,
    /// The journal would not open: no feed this boot.
    failed: bool,
    /// A feed an earlier boot left reserved, resolved as uncertain, whose
    /// `Fed` event is not recorded yet.
    interrupted: Option<OwnerResult>,
    current: Option<Current>,
    /// The last feed that ended without a reservation, and why.
    unreserved: Option<(u64, Refusal)>,
    stalled: bool,
    /// Refuse every feed `Busy`, as an owner that lost a race does.
    refusing: bool,
    /// The pack against its floor, and whether the adapter was in at the
    /// last step.
    floor: Floor,
    adapter: bool,
    /// Whether the hopper has food in front of the beam.
    hopper: bool,
    /// What the beam last read this boot, in a feed or at rest.
    food: Option<bool>,
    /// When to read it at rest, and the readings at rest this boot.
    food_reads: FoodReads,
    reads: u32,
    /// The fault the next feed ends on, after its first portion.
    fault: Option<FeedReason>,
}

impl SimFeeds {
    /// The owner as it opens at boot on `flash`'s journal, creating the
    /// journal on erased sectors, and resolving a feed a restart
    /// interrupted.
    pub fn open(flash: &Rc<Flash>) -> Self {
        let io = Region::new(flash, storage::JOURNAL);
        let mut journal = FeedJournal::default();
        let mut error = journal.open(io.clone(), JOURNAL);
        if error == JournalError::Empty {
            error = journal.create(io, JOURNAL, Direction::D0, true, true);
        }
        let failed = error != JournalError::Ok;
        let interrupted = if failed { None } else { journal.resolve() };
        Self {
            journal,
            failed,
            interrupted,
            current: None,
            unreserved: None,
            stalled: false,
            refusing: false,
            floor: Floor::default(),
            adapter: true,
            hopper: true,
            food: None,
            food_reads: FoodReads::new(),
            reads: 0,
            fault: None,
        }
    }

    /// Judge the supply at `now_ms`, as the owner does on each poll: the
    /// adapter, and the battery channel's pin millivolts. With no pack the
    /// floor stays unjudged, so the simulator's default (the adapter, no
    /// cells) records no floor crossing; on the pack alone that leaves
    /// feeds refused.
    pub fn sense(&mut self, now_ms: u32, supply: SimSupply) {
        self.adapter = supply.adapter;
        if let Some(pin_mv) = supply.pin_mv {
            self.floor
                .update(now_ms, Some(pin_mv), &wellfed_hal::power::FLOOR);
        }
    }

    /// On the pack alone and not judged above its floor.
    fn supply_low(&self) -> bool {
        !self.adapter && !self.floor.above()
    }

    /// End the next feed on `reason` once it has driven a portion's time:
    /// an outlet that blocks or a jam that holds. A feed of more than one
    /// portion ends with that one delivered; a feed of one ends `Failed`
    /// with none, since it ends before its last mark.
    pub fn fault(&mut self, reason: FeedReason) {
        self.fault = Some(reason);
    }

    /// Put food in front of the beam (`true`), or empty the hopper.
    pub fn fill(&mut self, food: bool) {
        self.hopper = food;
    }

    /// Hold a feed in flight where it is, or let it go on.
    pub fn stall(&mut self, stalled: bool) {
        self.stalled = stalled;
    }

    /// Refuse every feed `Busy` while readiness says ready, or stop.
    pub fn refuse(&mut self, refusing: bool) {
        self.refusing = refusing;
    }

    /// Step the owner at `now_ms`: the `Fed` event of a feed that ended,
    /// if one did, and how soon to step again while a feed runs.
    pub fn step(&mut self, now_ms: u32) -> (Option<EventBody>, Option<u32>) {
        if let Some(result) = self.interrupted.take() {
            let number = u32::try_from(result.id).unwrap_or(u32::MAX);
            return (
                Some(feed::interrupted(number, &result, Source::Feeder)),
                None,
            );
        }
        if self.current.is_none() {
            self.read_food(now_ms);
        }
        let Some(current) = self.current.as_mut() else {
            return (None, None);
        };
        if !current.reserved {
            let recovery = self.journal.view.recovery;
            let request = FeedRequest {
                id: current.feed,
                portions: current.portions,
                mode: JOURNAL.mode,
                direction: recovery.direction,
                credit_next_event: recovery.credit_next_event,
                phase_known: recovery.phase_known,
                homing: false,
            };
            if !self.journal.reserve(&request) {
                // Nothing ran: the device API answers the refusal.
                self.unreserved = Some((current.feed, Refusal::NotWritable));
                self.current = None;
                return (None, None);
            }
            current.reserved = true;
            current.stepped_ms = Some(now_ms);
            return (None, Some(0));
        }

        if !self.stalled
            && let Some(stepped) = current.stepped_ms
        {
            current.driven_ms = current
                .driven_ms
                .saturating_add(now_ms.wrapping_sub(stepped));
        }
        current.stepped_ms = Some(now_ms);
        let full = u32::from(current.portions) * PORTION_MS;
        let cancelled = current.cancelled.get();
        // A fault comes after a portion's drive and ends the feed there:
        // stopped, never resumed. A feed of one has not reached its mark.
        let faulted = self.fault.filter(|_| current.driven_ms >= PORTION_MS);
        if !cancelled && faulted.is_none() && current.driven_ms < full {
            let next = if self.fault.is_some() {
                PORTION_MS
            } else {
                full
            };
            return (None, Some(next.saturating_sub(current.driven_ms).max(1)));
        }

        let (outcome, reason, completed) = if let Some(reason) = faulted {
            self.fault = None;
            let delivered = u16::from(current.portions > 1);
            (OwnerOutcome::Failed, reason, delivered)
        } else if cancelled {
            let driven = current.driven_ms.min(full) / PORTION_MS;
            let driven = u16::try_from(driven).unwrap_or(current.portions);
            (OwnerOutcome::Cancelled, FeedReason::Cancelled, driven)
        } else {
            (OwnerOutcome::Done, FeedReason::Done, current.portions)
        };
        let result = OwnerResult {
            id: current.feed,
            requested: current.portions,
            completed,
            outcome,
            reason,
            motion_possible: true,
        };
        let by = current.by;
        self.current = None;
        self.food = Some(self.hopper);
        (Some(self.finish(&result, by)), None)
    }

    /// A food reading at rest, if one is due and the adapter is in.
    fn read_food(&mut self, now_ms: u32) {
        let view = FoodView {
            reading: false,
            reads: self.reads,
        };
        let held = self.failed || !self.adapter;
        if self.food_reads.step(now_ms, view, held) == FoodStep::Start {
            self.food_reads.started();
            self.food = Some(self.hopper);
            self.reads = self.reads.wrapping_add(1);
        }
    }

    /// Write `result` into the journal: the `Fed` event it makes, uncertain
    /// when the write fails.
    fn finish(&mut self, result: &OwnerResult, by: Source) -> EventBody {
        let phase = OwnerRecovery {
            high_water_id: result.id,
            pending_request: false,
            ..self.journal.view.recovery
        };
        let durability = if self.journal.finish(result, &phase) {
            Durability::Durable
        } else {
            Durability::Lost
        };
        fed(result, Some(self.hopper), durability, by)
    }
}

/// The `Fed` event for `result`, the food beam having read `hopper`. The
/// journal's ids fit a `u32` here: `submit` refuses one that does not.
fn fed(
    result: &OwnerResult,
    hopper: Option<bool>,
    durability: Durability,
    by: Source,
) -> EventBody {
    let feed = u32::try_from(result.id).unwrap_or(u32::MAX);
    feed::fed(feed, result, hopper, durability, by)
}

impl Feeds for SimFeeds {
    fn feeding(&self) -> bool {
        self.current.is_some()
    }

    fn busy(&self) -> bool {
        self.current.is_some()
    }

    fn blocked(&self) -> Option<Blocked> {
        if self.failed {
            Some(Blocked::Journal)
        } else if self.interrupted.is_some() {
            Some(Blocked::Resolving)
        } else if self.current.is_some() {
            Some(Blocked::Feeding)
        } else if self.supply_low() {
            Some(Blocked::Battery)
        } else {
            None
        }
    }

    fn below_floor(&self) -> Option<bool> {
        self.floor.below()
    }

    fn submit(&mut self, portions: u8, by: Source) -> Result<u32, Refusal> {
        if self.failed
            || self.interrupted.is_some()
            || self.current.is_some()
            || self.refusing
            || self.supply_low()
        {
            return Err(Refusal::Busy);
        }
        let asked = portions;
        let portions = u16::from(portions);
        if portions == 0 || portions > JOURNAL.max_portions {
            return Err(Refusal::Portions { asked });
        }
        let feed = self.journal.view.recovery.high_water_id.wrapping_add(1);
        let number = u32::try_from(feed).map_err(|_| Refusal::NotWritable)?;

        self.current = Some(Current {
            feed,
            portions,
            by,
            reserved: false,
            driven_ms: 0,
            stepped_ms: None,
            cancelled: Cell::new(false),
        });
        Ok(number)
    }

    fn reserved(&self, feed: u32) -> Option<Result<(), Refusal>> {
        let feed = u64::from(feed);
        match (&self.current, self.unreserved) {
            (Some(current), _) if current.feed == feed => current.reserved.then_some(Ok(())),
            (_, Some((ended, refusal))) if ended == feed => Some(Err(refusal)),
            // Reserved, and ended since.
            _ => Some(Ok(())),
        }
    }

    fn food(&self) -> Option<bool> {
        self.food
    }

    fn high_water(&self) -> Option<u32> {
        if self.failed {
            return None;
        }
        u32::try_from(self.journal.view.recovery.high_water_id).ok()
    }

    fn cancel(&self, feed: u32) -> bool {
        let Some(current) = &self.current else {
            return false;
        };
        let ours = current.feed == u64::from(feed);
        if ours {
            current.cancelled.set(true);
        }
        ours
    }
}

/// How many feeds the journal on `flash` has ever reserved: its high
/// water, which only a reservation moves. `None` before the journal
/// exists or when it does not open.
pub fn reserved(flash: &Rc<Flash>) -> Option<u64> {
    let mut journal = FeedJournal::default();
    let opened = journal.open(Region::new(flash, storage::JOURNAL), JOURNAL);
    (opened == JournalError::Ok).then_some(journal.view.recovery.high_water_id)
}
