//! The feeding owner, stepped from the loop task.
//!
//! The owner opens once the runtime has armed the flash hooks: its journal
//! writes go through them, and their lease authority is the one that blocks
//! the owner's outputs during a lease. Until then, and if it never opens,
//! the outputs stay as startup left them (motor STOP, emitters OFF) and the
//! runtime's watchdog rule is unchanged. At rest the owner holds the
//! board, samples the inputs, and beats.
//!
//! A feed comes from the device API: the loop submits it under the next
//! journal id, answers once the owner has reserved it, and when the owner
//! reports its result, logs what the drive measured, records a `Fed` event
//! and acknowledges it, which lets the owner take the next. One feed at a
//! time; a feed whose owner faulted without a durable result is recorded
//! as uncertain. So is a feed a reset interrupted: the owner resolves its
//! reservation when it opens, and the loop records that `Fed` event, by the
//! feeder, before the owner takes a feed.
//!
//! The limits are provisional. The timing ones come from the first pulse
//! and the optics probe (a 200 ms motor lease, 10 ms supervision timer,
//! stock's 100 ms sensor settling); the feed ones (duration, reverse,
//! current) are placeholders until measured feeds replace them. With no
//! confirmed feedback edge a drive stops after `feedback_timeout_ms`.
//!
//! Feeds and Wi-Fi joins take turns (`wellfed_hal::interlock`): an
//! association holds every task for up to 0.72 s, longer than the beat
//! allows while outputs may move, and no task priority avoids it. So a feed
//! is refused `Busy` while a join runs or waits, and a join requested during
//! a feed waits until the owner is at rest with its outputs stopped.
//!
//! Storage writes take the same turn: a flash lease while the outputs may
//! move fails its stopped-outputs check and resets the board. A store write
//! that comes during a feed waits until the outputs are released (at most
//! `FEED_BOUND_MS`), and a feed that finds a write running waits for it in
//! the loop, at most `WRITE_WAIT_MS`. The journal writes under the loop's
//! own claim, at the owner's rest boundaries.

use wellfed_core::feed::controller::FeedLimits;
use wellfed_core::feed::progress::BowlMode;
use wellfed_core::feed::sensor_inputs::SensorLimits;
use wellfed_core::store::feed_journal::JournalConfig;
use wellfed_proto::api::Refusal;
use wellfed_sdk::hal::interlock::{Held, Interlock};
use wellfed_sdk::hal::owner::{OwnerConfig, OwnerState, OwnerStatus};
use wellfed_sdk::hal::sdk;
use wellfed_sdk::hal::supervision::config::{SYSTIMER_HZ, WATCHDOG};
use wellfed_sdk::hal::supervision::events::EventsConfig;
use wellfed_sdk::hal::supervision::supervise::LIVENESS_MS;

/// The vendor app's cap on one feed, from the stock inventory. The journal
/// pins it: a journal written with another reads as corrupt.
pub const MAX_PORTIONS: u16 = 48;

/// The PLAF103's single bowl.
pub const BOWL: BowlMode = BowlMode::Single;

pub const CONFIG: OwnerConfig = OwnerConfig {
    feed: FeedLimits {
        // A portion is one compartment: PA18 reads LOW for 1.2-1.4 s at each
        // mark and a portion is credited on its rise, about 9.1 s of drive
        // apart empty and 9.9 s with food (bench). Like stock, an edge within
        // 5 s of the drive's start or the last portion does not count.
        feedback_inhibit_ms: 5000,
        // Half again a portion with food; stock waits about 30 s.
        feedback_timeout_ms: 15_000,
        direction_pause_ms: 500,
        reverse_ms: 500,
        // A feed's average must stay within 12 s a portion, and every gap
        // within the feedback timeout; the cap covers the largest request.
        max_duration_ms: 12_000 * MAX_PORTIONS as u32,
        portion_ms: 12_000,
        max_observation_gap_ms: 100,
        current_limit_ma: 300,
        max_portions: MAX_PORTIONS,
        max_reverse_attempts: 0,
    },
    // Stock: 100 ticks of settling after enable, about 1 s of bad samples
    // before a bad state.
    sensors: SensorLimits {
        optical_settle_ms: 100,
        food_empty_delay_ms: 1000,
        outlet_block_delay_ms: 1000,
        power_settle_ms: 100,
        max_observation_gap_ms: 100,
        max_age_ms: 100,
        battery_min_scaled: 1650,
    },
    events: EventsConfig {
        ticks_per_second: SYSTIMER_HZ,
        timer_period_ms: 10,
        max_feedback_age_ms: 100,
        feedback_capacity: 8,
    },
    max_progress_gap_ms: WATCHDOG.max_progress_gap_ms,
    // At rest the loop may be held off by the Wi-Fi driver's handshake for
    // over a second (observed); the runtime's liveness limit for its tasks.
    idle_gap_ms: LIVENESS_MS,
    bowl_mode: BOWL,
    // The loop's own pass; a whole flash lease fits between two beats.
    sample_period_ms: 20,
    prepare_timeout_ms: 2000,
    optics_max_on_ms: 12_000 * MAX_PORTIONS as u32 + 2000,
    motor_deadtime_ms: 50,
    motor_lease_ms: 200,
    adc_poll_limit: 10_000,
    wake_lock_id: sdk::PMU_USER_BASE,
    flash: core::ptr::null_mut(),
};

/// The longest one feed can keep the outputs from rest, and so the longest
/// a Wi-Fi join or a storage write waits for them: the owner keeps the emitters on for at most
/// `optics_max_on_ms` from the request, and its configuration check holds
/// the prepare timeout and the longest drive inside that, then it stops (or
/// faults, which stops too). The margin covers the stop and the pass that
/// sees it. About 9 min 40 s for the largest request.
pub const FEED_BOUND_MS: u32 = CONFIG.optics_max_on_ms + 2000;

/// Claims the outputs in `interlock` for one feed, before it is submitted,
/// on the clock `now_ms` that `delay` advances. A storage write running or
/// waiting gets its turn first (the loop waits for it, at most
/// `WRITE_WAIT_MS`). A join running or waiting, or a write still running
/// past that wait, refuses the feed `Busy`, final for that write id (clients
/// never retry a feed on their own); `held` hears why, for the log.
pub fn claim_outputs(
    interlock: &Interlock,
    now_ms: impl FnMut() -> u32,
    delay: impl FnMut(u32),
    held: impl FnOnce(Held),
) -> Result<(), Refusal> {
    interlock
        .claim_feed_after_writes(FEED_BOUND_MS, now_ms, delay)
        .map_err(|why| {
            held(why);
            Refusal::Busy
        })
}

/// Whether the loop has a feed between its submission and its recorded
/// result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InFlight {
    Nothing,
    Feed,
}

/// Whether the loop may hand the outputs' turn back to Wi-Fi joins: no feed
/// in flight, the owner in a state that never drives (a queued request is
/// about to), and motor STOP and the emitters off read back. A failed stop
/// keeps the turn, and a waiting join then goes ahead only past the bound.
pub fn may_release(in_flight: InFlight, status: &OwnerStatus) -> bool {
    let resting = matches!(
        status.state,
        OwnerState::Starting
            | OwnerState::Idle
            | OwnerState::Terminal
            | OwnerState::Fault
            | OwnerState::Stopped
    );
    in_flight == InFlight::Nothing && resting && status.stopped && status.optics_off
}

pub const JOURNAL: JournalConfig = JournalConfig {
    binding: wellfed_sdk::storage::JOURNAL_BINDING,
    mode: BOWL,
    max_portions: MAX_PORTIONS,
};

#[cfg(target_os = "none")]
pub use target::{Feeding, Record, describe};

#[cfg(target_os = "none")]
mod target {
    use wellfed_core::feed::progress::Direction;
    use wellfed_core::feed::storage::OwnerResult;
    use wellfed_core::store::feed_journal::{FeedJournal, JournalError};
    use wellfed_feeder::feed::{self, Durability};
    use wellfed_proto::api::{Refusal, Source};
    use wellfed_sdk::hal::interlock::{self, Held, INTERLOCK};
    use wellfed_sdk::hal::owner::{
        FeedTrace, Owner, OwnerConfig, OwnerFault, OwnerState, OwnerStatus, Step, SubmitResult,
    };
    use wellfed_sdk::hal::supervision::events::{EventsFault, PinWatch};
    use wellfed_sdk::{JournalSlots, Slot};

    use super::{CONFIG, JOURNAL};

    type Journal = FeedJournal<JournalSlots>;

    /// The owner. Reached only through `&`: the supervision interrupts reach
    /// its board and supervisor while the loop runs, so no `&mut` may cover
    /// it once it is open. Filled once, before it opens.
    static OWNER: Slot<Owner<Journal>> = Slot::new();

    /// What the loop logs, for the bench.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Record {
        /// The journal could not be opened or created; the owner never opens
        /// this boot.
        Journal { error: JournalError, created: bool },
        /// The owner refused to open (a lease in progress, or the authority
        /// not yet ready); retried each pass, logged once.
        Refused,
        /// Opened, on a journal created this boot or found.
        Opened { created: bool, high_water: u64 },
        /// The owner's published state changed.
        Status {
            state: OwnerState,
            fault: OwnerFault,
            supervisor: EventsFault,
            stopped: bool,
            optics_off: bool,
        },
        /// A feed was handed to the owner, or refused before it was.
        Submitted {
            feed: u64,
            portions: u8,
            result: SubmitResult,
        },
        /// A feed ended: its result and what its drive measured.
        Fed {
            result: OwnerResult,
            durability: Durability,
            trace: FeedTrace,
        },
        /// Which pads changed during that feed, to find the feedback input.
        Pins { feed: u64, pins: PinWatch },
        /// When each feedback edge came, from the drive's start.
        Edges { feed: u64, trace: FeedTrace },
        /// A feed was refused before the owner saw it: a Wi-Fi join runs or
        /// waits, or a storage write ran past the claim's wait.
        Held { portions: u8, held: Held },
        /// Joins waited for the outputs, or went ahead past a feed's bound.
        Joins(interlock::Status),
    }

    /// The most pads a `pins` record lists, so the line fits a record.
    const PINS_LISTED: usize = 10;

    pub fn describe(record: Record, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match record {
            Record::Journal { error, created } => write!(
                f,
                "open journal={} created={}",
                error.code(),
                u8::from(created)
            ),
            Record::Refused => f.write_str("open refused=1"),
            Record::Opened {
                created,
                high_water,
            } => write!(
                f,
                "open created={} high_water={high_water}",
                u8::from(created)
            ),
            Record::Status {
                state,
                fault,
                supervisor,
                stopped,
                optics_off,
            } => write!(
                f,
                "state={} fault={} supervisor={} stopped={} optics_off={}",
                state.code(),
                fault.code(),
                supervisor.code(),
                u8::from(stopped),
                u8::from(optics_off)
            ),
            Record::Submitted {
                feed,
                portions,
                result,
            } => write!(
                f,
                "feed id={feed} portions={portions} submit={}",
                result.code()
            ),
            Record::Fed {
                result,
                durability,
                trace,
            } => {
                write!(
                    f,
                    "fed id={} requested={} completed={} outcome={} reason={} durable={} drive_ms={} rising={} falling={} first_edge_ms=",
                    result.id,
                    result.requested,
                    result.completed,
                    result.outcome.code(),
                    result.reason.code(),
                    u8::from(durability == Durability::Durable),
                    trace.drive_ms,
                    trace.rising,
                    trace.falling,
                )?;
                match trace.first_edge_ms {
                    Some(ms) => write!(f, "{ms}")?,
                    None => f.write_str("none")?,
                }
                write!(f, " current_max={} current_mean=", trace.current_max)?;
                match trace.current_mean() {
                    Some(mean) => write!(f, "{mean}")?,
                    None => f.write_str("none")?,
                }
                write!(f, " samples={}", trace.current_samples)
            }
            Record::Edges { feed, trace } => {
                write!(f, "edges id={feed} at=")?;
                let mut logged = trace.edge_ms.iter().flatten().peekable();
                if logged.peek().is_none() {
                    return f.write_str("none");
                }
                for (index, &(ms, high)) in logged.enumerate() {
                    if index != 0 {
                        f.write_str(",")?;
                    }
                    write!(f, "{ms}{}", if high { 'H' } else { 'L' })?;
                }
                Ok(())
            }
            Record::Held { portions, held } => {
                write!(f, "feed portions={portions} held={}", held.code())
            }
            Record::Joins(status) => write!(
                f,
                "interlock refused={} deferred={} forced={} waited_max_ms={} long_joins={} \
                 writes_deferred={} writes_forced={} write_waited_max_ms={} feeds_delayed={}",
                status.refused,
                status.deferred,
                status.forced,
                status.waited_max_ms,
                status.long_joins,
                status.writes_deferred,
                status.writes_forced,
                status.write_waited_max_ms,
                status.feeds_delayed
            ),
            Record::Pins { feed, pins } => {
                let [start_a, start_b] = pins.start.unwrap_or_default();
                let [changed_a, changed_b] = pins.changed;
                write!(
                    f,
                    "pins id={feed} start={start_a:08x},{start_b:08x} changed={changed_a:08x},{changed_b:08x} toggles="
                )?;
                let toggles = pins
                    .toggles
                    .iter()
                    .zip(['A', 'B'])
                    .flat_map(|(port, name)| {
                        (0..32).filter_map(move |pin| {
                            let count = port[pin];
                            (count != 0).then_some((name, pin, count))
                        })
                    });
                let mut listed = 0;
                for (name, pin, count) in toggles {
                    if listed == PINS_LISTED {
                        return f.write_str(",+");
                    }
                    if listed != 0 {
                        f.write_str(",")?;
                    }
                    write!(f, "{name}{pin}:{count}")?;
                    listed += 1;
                }
                if listed == 0 {
                    f.write_str("none")?;
                }
                Ok(())
            }
        }
    }

    /// The feed in flight, from its submission until its result is recorded.
    #[derive(Clone, Copy, Debug)]
    struct Current {
        feed: u64,
        by: Source,
        /// The owner has reserved it in the journal.
        reserved: bool,
    }

    /// The loop's half of the feeding owner: when to open it, and what it
    /// last logged.
    #[derive(Debug, Default)]
    pub struct Feeding {
        /// Opened, or the journal failed: nothing more to try this boot.
        settled: bool,
        /// The journal was created this boot (by an attempt the owner may
        /// have refused).
        created: bool,
        refused: bool,
        logged: Option<OwnerStatus>,
        current: Option<Current>,
        /// Why the last feed ended before its reservation, for the API's
        /// poll: its id and the refusal.
        unreserved: Option<(u64, Refusal)>,
        /// The interlock's counters as last logged.
        joins: interlock::Status,
        /// The reservation a reset interrupted, which the owner resolves as
        /// it opens: its id, until its `Fed` event is recorded.
        interrupted: Option<u64>,
    }

    impl Feeding {
        /// One pass; the owner's delay while it runs.
        pub fn step(&mut self, mut record: impl FnMut(Record)) -> Option<u32> {
            if !self.settled {
                if !wellfed_sdk::storage_armed() {
                    return None;
                }
                match self.open() {
                    Some(Record::Refused) if self.refused => {}
                    Some(Record::Refused) => {
                        self.refused = true;
                        record(Record::Refused);
                    }
                    Some(opened) => {
                        self.settled = true;
                        record(opened);
                    }
                    None => {}
                }
            }

            let owner = OWNER.enter()?;
            let delay = match owner.step() {
                Step::Wait(ms) => Some(ms),
                Step::Stopped | Step::Refused => None,
            };

            let status = owner.status();
            let key = |s: &OwnerStatus| {
                (
                    s.state,
                    s.fault,
                    s.supervisor_fault,
                    s.stopped,
                    s.optics_off,
                )
            };
            if self.logged.as_ref().map(key) != Some(key(&status)) {
                self.logged = Some(status);
                record(Record::Status {
                    state: status.state,
                    fault: status.fault,
                    supervisor: status.supervisor_fault,
                    stopped: status.stopped,
                    optics_off: status.optics_off,
                });
            }

            self.follow(&owner, &status, &mut record);
            self.release(&status, &mut record);
            delay
        }

        /// Hands the outputs' turn back by `may_release`, and watches for a
        /// join that holds its turn too long.
        fn release(&mut self, status: &OwnerStatus, record: &mut impl FnMut(Record)) {
            let in_flight = if self.current.is_some() {
                super::InFlight::Feed
            } else {
                super::InFlight::Nothing
            };
            if super::may_release(in_flight, status) {
                INTERLOCK.release_feed();
            }
            INTERLOCK.watch_join(wellfed_sdk::hal::sdk::uptime_ms());

            // Logged whenever a counter moves: a refused feed, a join that
            // starts waiting, waits longer than any before, is forced, or
            // holds its turn past the long-join limit.
            let joins = INTERLOCK.status();
            if joins != self.joins {
                self.joins = joins;
                record(Record::Joins(joins));
            }
        }

        /// The feed in flight against the owner's status: note its
        /// reservation, and record its end.
        fn follow(
            &mut self,
            owner: &Owner<Journal>,
            status: &OwnerStatus,
            record: &mut impl FnMut(Record),
        ) {
            let Some(current) = self.current.as_mut() else {
                self.report_interrupted(owner, status, record);
                return;
            };
            let ours = status.result.id == current.feed;
            if ours && status.intent_reserved {
                current.reserved = true;
            }

            let durability = match status.state {
                OwnerState::Terminal if ours && status.result_durable => Durability::Durable,
                OwnerState::Fault if ours && status.result_durable => Durability::Durable,
                OwnerState::Fault if current.reserved => Durability::Lost,
                OwnerState::Fault => {
                    self.unreserved = Some((current.feed, Refusal::Fault));
                    self.current = None;
                    return;
                }
                _ => return,
            };

            let current = *current;
            self.current = None;
            let result = if ours {
                status.result
            } else {
                OwnerResult {
                    id: current.feed,
                    ..OwnerResult::default()
                }
            };
            record(Record::Fed {
                result,
                durability,
                trace: status.trace,
            });
            record(Record::Edges {
                feed: result.id,
                trace: status.trace,
            });
            if status.trace.pins.start.is_some() {
                record(Record::Pins {
                    feed: result.id,
                    pins: status.trace.pins,
                });
            }
            // The feed id fits the event: `submit` refused any that does not.
            let feed = u32::try_from(current.feed).unwrap_or(u32::MAX);
            crate::api::record_event(feed::fed(feed, &result, durability, current.by));
            if status.state == OwnerState::Terminal {
                owner.acknowledge(current.feed);
            }
        }

        /// The interrupted feed's uncertain result, once the owner holds it
        /// durably: recorded once, then acknowledged. An owner that faulted
        /// after the resolution still holds it; its event is recorded all
        /// the same, with nothing to acknowledge.
        fn report_interrupted(
            &mut self,
            owner: &Owner<Journal>,
            status: &OwnerStatus,
            record: &mut impl FnMut(Record),
        ) {
            let Some(feed) = self.interrupted else {
                return;
            };
            if !status.result_durable || status.result.id != feed {
                return;
            }
            match status.state {
                OwnerState::Terminal if owner.acknowledge(feed) => {}
                OwnerState::Fault => {}
                _ => return,
            }

            self.interrupted = None;
            record(Record::Fed {
                result: status.result,
                durability: Durability::Durable,
                trace: status.trace,
            });
            // The journal holds the id under the high water `submit` checks.
            let number = u32::try_from(feed).unwrap_or(u32::MAX);
            let body = feed::fed(number, &status.result, Durability::Durable, Source::Feeder);
            crate::api::record_event(body);
        }

        /// Hand one feed to the owner on `by`'s behalf: the number its
        /// journal will hold it under.
        pub fn submit(
            &mut self,
            portions: u8,
            by: Source,
            mut record: impl FnMut(Record),
        ) -> Result<u32, Refusal> {
            let owner = OWNER.enter().ok_or(Refusal::Busy)?;
            if self.current.is_some() {
                return Err(Refusal::Busy);
            }
            let clock = wellfed_sdk::hal::sdk::uptime_ms;
            let delay = wellfed_sdk::hal::sdk::delay_ms;
            super::claim_outputs(&INTERLOCK, clock, delay, |held| {
                record(Record::Held { portions, held });
            })?;
            let status = owner.status();
            let feed = status.high_water_id.wrapping_add(1);
            let number = u32::try_from(feed).map_err(|_| Refusal::NotWritable)?;

            let result = owner.submit(feed, u16::from(portions));
            record(Record::Submitted {
                feed,
                portions,
                result,
            });
            match result {
                SubmitResult::Enqueued => {
                    self.current = Some(Current {
                        feed,
                        by,
                        reserved: false,
                    });
                    self.unreserved = None;
                    Ok(number)
                }
                SubmitResult::RejectRequest => Err(Refusal::Invalid),
                SubmitResult::RejectStopped if status.state == OwnerState::Fault => {
                    Err(Refusal::Fault)
                }
                // Still opening, busy, or a phase only a double bowl needs.
                SubmitResult::RejectStopped
                | SubmitResult::RejectBusy
                | SubmitResult::RejectId
                | SubmitResult::RejectPhase => Err(Refusal::Busy),
            }
        }

        /// Whether the feed `feed` is reserved yet: `None` while it waits.
        pub fn reserved(&self, feed: u32) -> Option<Result<(), Refusal>> {
            let feed = u64::from(feed);
            match (self.current, self.unreserved) {
                (Some(current), _) if current.feed == feed => current.reserved.then_some(Ok(())),
                (_, Some((ended, refusal))) if ended == feed => Some(Err(refusal)),
                // Ended after its reservation, and already recorded.
                _ => Some(Ok(())),
            }
        }

        /// Stop the feed `feed` if it is still running.
        pub fn cancel(&self, feed: u32) -> bool {
            let feed = u64::from(feed);
            self.current.is_some_and(|current| current.feed == feed)
                && OWNER.enter().is_some_and(|owner| owner.cancel(feed))
        }

        /// A feed is being prepared or driven: the loop defers handshakes.
        pub fn feeding(&self) -> bool {
            OWNER.enter().is_some_and(|owner| {
                matches!(
                    owner.status().state,
                    OwnerState::Preparing | OwnerState::Feeding
                )
            })
        }

        /// Opens the journal (creating it on erased sectors only), then the
        /// owner on it.
        fn open(&mut self) -> Option<Record> {
            let io = JournalSlots::new();
            let mut journal = Journal::default();
            let mut error = journal.open(io, JOURNAL);
            if error == JournalError::Empty {
                // First boot of the feeding owner: both sectors erased.
                self.created = true;
                error = journal.create(io, JOURNAL, Direction::D0, true, true);
            }
            let created = self.created;
            if error != JournalError::Ok {
                return Some(Record::Journal { error, created });
            }
            let high_water = journal.view.recovery.high_water_id;
            let interrupted = journal.view.recovery.pending_request.then_some(high_water);

            // Filled here, before the owner opens, so no interrupt reaches it yet.
            let owner = OWNER.enter_or_init(Owner::default)?;
            let config = OwnerConfig {
                flash: wellfed_sdk::flash_authority(),
                ..CONFIG
            };
            // SAFETY: the authority is the runtime's static, live for the
            // boot; the owner is a static and never moves.
            let opened = unsafe { owner.open(&config, journal) };
            Some(if opened {
                self.interrupted = interrupted;
                Record::Opened {
                    created,
                    high_water,
                }
            } else {
                Record::Refused
            })
        }
    }
}
