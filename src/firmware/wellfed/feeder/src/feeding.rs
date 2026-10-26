//! The feeding owner, stepped from the loop task.
//!
//! The owner opens once the runtime has armed the flash hooks: its journal
//! writes go through them, and their lease authority is the one that blocks
//! the owner's outputs during a lease. Until then, and if it never opens,
//! the outputs stay as startup left them (motor STOP, emitters OFF) and the
//! runtime's watchdog rule is unchanged. At rest the owner holds the
//! board, samples the inputs, and beats.
//!
//! A trial build confirms only once the owner has proved it can feed
//! ([`Proof`], reported to the runtime each pass): open on its journal, any
//! interrupted feed resolved, and at rest with every check passing for
//! [`PROOF_MS`]. An owner that cannot open reports a failure, which resets a
//! trial build at once so KM0 counts it. After a reset storm with nothing
//! to fall back to (`wellfed_sdk::held`), the owner stays closed until the
//! runtime ends the hold, then opens as on any boot.
//!
//! A feed comes from the device API (`wellfed_feeder::feeder::Feeds`): the
//! loop submits it under the next journal id, answers once the owner has
//! reserved it, and when the owner reports its result, logs what the drive
//! measured, records a `Fed` event through the product's event recorder and
//! acknowledges it, which lets the owner take the next. One feed at a
//! time; a feed whose owner faulted without a durable result is recorded
//! as uncertain. So is a feed a reset interrupted: the owner resolves its
//! reservation when it opens, and the loop records that `Fed` event, by the
//! feeder, before the owner takes a feed.
//!
//! The timing limits come from the first pulse and the optics probe (a
//! 200 ms motor lease, 10 ms supervision timer, stock's 100 ms sensor
//! settling); the feed ones from the measured feeds (marks, portions,
//! current in raw scaled counts) and stock (the jam limit and its three
//! reversals). With no confirmed feedback edge a drive stops after
//! `feedback_timeout_ms`, and with no edge at all after `mark_gap_ms`; a
//! jam backs off at most `max_reverse_attempts` times, then ends the feed.
//! None of them restarts a feed.
//!
//! Feeds and Wi-Fi joins take turns (`wellfed_hal::interlock`): an
//! association holds every task for up to 0.72 s, longer than the beat
//! allows while outputs may move, and no task priority avoids it. So a feed
//! is refused `Busy` while a join runs or waits, and a join requested during
//! a feed waits until the owner is at rest with its outputs stopped.
//!
//! Readiness ([`wellfed_feeder::feeder::feeding::readiness`]) starts from
//! the owner's own reason not to feed (`Feeding::blocked`): not open yet
//! or the build not yet confirmed, the journal failed, a fault (held after
//! a reset storm included), an interrupted feed being resolved, a feed in
//! flight, or a join's turn. `submit` keeps its own checks behind it.
//!
//! Each feed is traced (`wellfed_feeder_runtime::traces`): after every
//! owner step the loop hands what the step saw to the trace, so the trace
//! never runs inside the owner's stop and motor decisions, and when the
//! feed's result is recorded the trace is sealed with it and the owner's
//! drive figures, for the settings writer to store once the outputs are
//! stopped.
//!
//! The rotor's phase is the owner's ([`RotorPhase`]): a feed from an
//! unknown phase homes on its first mark, uncredited, and the loop counts
//! those homing feeds and the outlet beam's breaks for the portions
//! statistics (`PLOWN1 marks`), so delivered food can be estimated apart
//! from the requested count ([calibration]).
//!
//! [calibration]: ../../../../../docs/firmware/calibration.md
//!
//! While a feed is driven the loop steps the owner before each pass and,
//! when the pass ran a handshake operation (a client opening a session),
//! again at once after it, so the owner's observations stay
//! inside its 100 ms gap; `PLOWN1 steps` reports, for each feed, the
//! longest gap between two steps and the handshake operations it absorbed.
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
use wellfed_core::feed::storage::{OwnerResult, RotorPhase};
use wellfed_core::store::feed_journal::{JournalConfig, JournalError};
use wellfed_feeder::feed::Durability;
use wellfed_proto::api::Refusal;
use wellfed_sdk::hal::interlock::{self, Held, Interlock};
use wellfed_sdk::hal::owner::{
    FeedTrace, OwnerConfig, OwnerFault, OwnerState, OwnerStatus, SubmitResult,
};
use wellfed_sdk::hal::sdk;
use wellfed_sdk::hal::supervision::config::{SYSTIMER_HZ, WATCHDOG};
use wellfed_sdk::hal::supervision::events::{EventsConfig, EventsFault, PinWatch};
use wellfed_sdk::hal::supervision::supervise::{self, LIVENESS_MS};

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
        // A jam backs off 1 s (about 13 degrees of rotor, a ninth of a
        // compartment), stops 0.5 s, and drives on, three times at most as
        // stock does; each costs 2.5 s of deadline and feedback window.
        // Stock's reversal length is not recovered: 1 s is provisional
        // until a real jam shows what frees it.
        direction_pause_ms: 500,
        reverse_ms: 1000,
        // A feed's average must stay within 12 s a portion, and every gap
        // within the feedback timeout; the cap covers the largest request.
        max_duration_ms: 12_000 * MAX_PORTIONS as u32,
        portion_ms: 12_000,
        max_observation_gap_ms: 100,
        // A turning rotor shows an edge at least every 8.5 s (a mark's fall
        // 7.8-8.4 s after the last rise, empty or with food, on the
        // adapter), and reads LOW for 1.2-1.4 s at a mark. On batteries the
        // motor may turn slower; a gap past 11 s there would end feeds
        // `FeedbackTimeout`, so the rotor's speed on D cells needs a check.
        mark_gap_ms: 11_000,
        mark_low_ms: 4000,
        // Scaled counts (stock's unit; milliamps need a reference): a drive
        // reads a mean of 59-64 and peaks of 164-168 empty, a mean of 82-87
        // with food, and 15-16 stopped. Stock reverses over 300. A start-up
        // surge read 333 on one poll at 17 ms and up to 447 at 10 ms
        // polling; the 150 ms after each change of drive (50 ms of it the
        // driver's deadtime) count toward no jam, and only a reading past
        // twice the highest surge stops at once.
        //
        // Risk: both come from empty and light-food drives on the adapter.
        // A full hopper's peaks, the peaks on batteries, and a real stall's
        // current are unmeasured. Peaks near 300 would reverse sound feeds
        // (and end them after three); a stall reading over 1000 would stop
        // every jam at once with no reversal, which is safe but never frees
        // one. Both want measuring before the limits are trusted.
        current_limit_ma: 300,
        current_max_ma: 1000,
        surge_ms: 150,
        max_portions: MAX_PORTIONS,
        max_reverse_attempts: 3,
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

/// The shortest drive between two marks' rises: 9.1 s with the hopper
/// empty on the adapter, 9.8-9.9 s with food (bench, October 25,
/// docs/firmware/motor-pulse.md, "Food and a full turn").
pub const MARK_PERIOD_MIN_MS: u32 = 9100;

// A homing mark is uncredited, so its window must never hold a whole
// compartment of travel. The most forward drive that window can hold is
// the inhibit plus every reversal's return over the ground it gave up
// (the back-off and pause themselves move the rotor no further forward).
const _: () = assert!(
    CONFIG.feed.feedback_inhibit_ms
        + CONFIG.feed.max_reverse_attempts as u32 * CONFIG.feed.reverse_ms
        < MARK_PERIOD_MIN_MS
);

/// The longest one feed can keep the outputs from rest, and so the longest
/// a Wi-Fi join or a storage write waits for them: the owner keeps the emitters on for at most
/// `optics_max_on_ms` from the request, and its configuration check holds
/// the prepare timeout and the longest drive inside that, then it stops (or
/// faults, which stops too). The margin covers the stop and the pass that
/// sees it. About 9 min 40 s for the largest request.
pub const FEED_BOUND_MS: u32 = CONFIG.optics_max_on_ms + 2000;

/// Claims the outputs in `interlock` for one feed, before it is submitted,
/// on the clock `now_ms` that `delay` advances. A storage write running or
/// waiting, or a change `queued` in a mailbox, goes first (the loop waits
/// for it, at most `WRITE_WAIT_MS`). A join running or waiting, or a write
/// still running or a change still queued past that wait, refuses the feed
/// `Busy`, final for that write id (clients never retry a feed on their
/// own); `held` hears why, for the log.
pub fn claim_outputs(
    interlock: &Interlock,
    queued: impl FnMut() -> bool,
    now_ms: impl FnMut() -> u32,
    delay: impl FnMut(u32),
    held: impl FnOnce(Held),
) -> Result<(), Refusal> {
    interlock
        .claim_feed_after_writes(FEED_BOUND_MS, queued, now_ms, delay)
        .map_err(|why| {
            held(why);
            Refusal::Busy
        })
}

/// Whether a client's change waits in a mailbox for the settings writer,
/// which takes its write turn only on its next poll: a claim lets it go
/// first, so the client hears its answer before the feed rather than after.
pub fn writes_queued() -> bool {
    use wellfed_feeder_runtime::{keys, network, traces};
    use wellfed_sdk::hal::settings;
    settings::pending().is_some() || network::saving() || keys::queued() || traces::queued()
}

/// The owner's figures for a feed's drive, as its trace's summary keeps
/// them: the same that `fed` logs.
pub fn drive_figures(trace: &FeedTrace) -> wellfed_feeder::trace::DriveFigures {
    wellfed_feeder::trace::DriveFigures {
        drive_ms: trace.drive_ms,
        samples: u32::from(trace.current_samples),
        current_peak: trace.current_max,
        current_mean: trace.current_mean(),
        rising: trace.rising,
        falling: trace.falling,
        first_edge_ms: trace.first_edge_ms,
        reversals: u16::from(trace.reversals),
    }
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

/// How long the owner rests Idle with every check passing before it has
/// proved it can feed: a hundred of its 20 ms samples, each reading the
/// sensors and the motor current and verifying motor STOP and the emitters
/// off.
pub const PROOF_MS: u32 = 2_000;

/// What the owner has shown this boot, for the runtime: a trial build
/// confirms only on [`supervise::Feeding::Proven`], and one whose owner cannot open
/// ([`supervise::Feeding::Failed`]) is reset at once and counted.
#[derive(Clone, Copy, Debug, Default)]
pub struct Proof {
    /// When the owner was first seen at rest with every check passing,
    /// since it last was not.
    resting_since: Option<u32>,
}

/// The owner as the loop sees it, for [`Proof::step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opened<'a> {
    /// Not opened yet: the hooks are unarmed, or a lease holds the flash.
    Not,
    /// The journal could not be opened or created: no owner this boot.
    JournalFailed,
    /// Opened; `resolving` while a feed a reset interrupted is still being
    /// recorded as uncertain.
    Owner {
        status: &'a OwnerStatus,
        resolving: bool,
    },
}

impl Proof {
    pub fn step(&mut self, opened: Opened<'_>, now_ms: u32) -> supervise::Feeding {
        let (status, resolving) = match opened {
            Opened::Not => return self.pending(),
            Opened::JournalFailed => return self.failed(),
            Opened::Owner { status, resolving } => (status, resolving),
        };
        // A fault latches until a restart: setup, the journal's recovery,
        // storage, an output or input check, or the owner's own beat.
        if status.state == OwnerState::Fault || status.fault != OwnerFault::Ok {
            return self.failed();
        }

        let resting = status.state == OwnerState::Idle
            && status.supervisor_fault == EventsFault::Ok
            && status.stopped
            && status.optics_off
            && !resolving;
        if !resting {
            return self.pending();
        }
        let since = *self.resting_since.get_or_insert(now_ms);
        if now_ms.wrapping_sub(since) >= PROOF_MS {
            supervise::Feeding::Proven
        } else {
            supervise::Feeding::Pending
        }
    }

    fn pending(&mut self) -> supervise::Feeding {
        self.resting_since = None;
        supervise::Feeding::Pending
    }

    fn failed(&mut self) -> supervise::Feeding {
        self.resting_since = None;
        supervise::Feeding::Failed
    }
}

/// How long the owner holds lost sensors, at rest, before the loop asks
/// for the one restart a power cycle allows.
pub const RECOVERY_HOLD_MS: u32 = 10 * 60_000;

/// Which boot of its power cycle this is: the KM4 boot count the flash
/// arming keeps in a backup register, which power loss clears.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerCycle {
    FirstBoot,
    /// Some reset came before this boot: a restart, an update, a watchdog.
    Restarted,
}

/// When a feeder whose owner holds lost sensors restarts by itself.
///
/// Lost sensors are held, not reset on (the owner's own rule), so a feeder
/// left alone would never feed again. A converter that stopped once may
/// convert again after a reset, so after `RECOVERY_HOLD_MS` of holding them
/// at rest with nothing in flight, the loop asks for one restart, only from
/// the first boot of a power cycle: if the sensors are lost again after it,
/// the owner holds them for good, until the owner restarts it or power is
/// cut. No backup register bit is free to count restarts, so a power cycle
/// that already had any reset gives up its recovery restart.
#[derive(Clone, Copy, Debug, Default)]
pub struct Recovery {
    /// Since when the owner has held lost sensors at rest.
    since: Option<u32>,
    asked: bool,
}

impl Recovery {
    /// One pass at `now_ms`; `held` names the boot while the owner holds
    /// lost sensors with nothing in flight and no install running. True
    /// once, when the restart is due.
    pub fn step(&mut self, now_ms: u32, held: Option<PowerCycle>) -> bool {
        let Some(cycle) = held else {
            self.since = None;
            return false;
        };
        let since = *self.since.get_or_insert(now_ms);
        let due = cycle == PowerCycle::FirstBoot
            && !self.asked
            && now_ms.wrapping_sub(since) >= RECOVERY_HOLD_MS;
        self.asked |= due;
        due
    }

    /// How long the owner has held them, for the log.
    pub fn held_ms(&self, now_ms: u32) -> u32 {
        self.since.map_or(0, |since| now_ms.wrapping_sub(since))
    }
}

pub const JOURNAL: JournalConfig = JournalConfig {
    binding: wellfed_sdk::storage::JOURNAL_BINDING,
    mode: BOWL,
    max_portions: MAX_PORTIONS,
};

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
    /// What the feed's marks and the outlet beam showed: how far into the
    /// drive a homing mark came, the beam's breaks, and the phase after.
    Marks {
        feed: u64,
        homed_ms: Option<u32>,
        outlet_breaks: u32,
        phase: RotorPhase,
    },
    /// While the feed was in flight: the longest gap between two owner
    /// steps, and the handshake operations the loop ran.
    Steps {
        feed: u64,
        max_gap_ms: u32,
        handshakes: u32,
    },
    /// The owner held lost sensors this long: the loop restarts the feeder
    /// once to recover them.
    Recover { held_ms: u32 },
}

/// The most pads a `pins` record lists, so the line fits a record.
const PINS_LISTED: usize = 10;

/// The outlet beam's breaks in a feed's pin watch: each break is two
/// changes of PA16, a half-seen one at either end counts whole. Sampled on
/// the 10 ms supervision tick, so a shorter passage can fall between two
/// samples: a lower bound.
pub fn outlet_breaks(pins: &PinWatch) -> u32 {
    let outlet = usize::try_from(wellfed_sdk::hal::plaf103::pins::OUTLET).unwrap_or(usize::MAX);
    let toggles = pins.toggles[0].get(outlet).copied().unwrap_or(0);
    u32::from(toggles).div_ceil(2)
}

/// The body of a `PLOWN1` line.
impl core::fmt::Display for Record {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
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
                write!(
                    f,
                    " samples={} reversals={}",
                    trace.current_samples, trace.reversals
                )
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
            Record::Marks {
                feed,
                homed_ms,
                outlet_breaks,
                phase,
            } => {
                write!(f, "marks id={feed} homed_ms=")?;
                match homed_ms {
                    Some(ms) => write!(f, "{ms}")?,
                    None => f.write_str("none")?,
                }
                let phase = match phase {
                    RotorPhase::AtMark => "at_mark",
                    RotorPhase::Unknown => "unknown",
                };
                write!(f, " outlet_breaks={outlet_breaks} phase={phase}")
            }
            Record::Steps {
                feed,
                max_gap_ms,
                handshakes,
            } => write!(
                f,
                "steps id={feed} max_gap_ms={max_gap_ms} handshakes={handshakes}"
            ),
            Record::Recover { held_ms } => write!(f, "recover restart=1 held_ms={held_ms}"),
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
}

#[cfg(target_os = "none")]
pub use target::Feeding;

#[cfg(target_os = "none")]
mod target {
    use wellfed_core::feed::progress::Direction;
    use wellfed_core::feed::storage::{OwnerResult, RotorPhase};
    use wellfed_core::store::feed_journal::{FeedJournal, JournalError};
    use wellfed_feeder::feed::{self, Durability};
    use wellfed_feeder::feeder::Marks;
    use wellfed_feeder_runtime::traces;
    use wellfed_proto::api::{Blocked, EventBody, Phase, Refusal, Source, TraceSummary};
    use wellfed_sdk::hal::interlock::{self, INTERLOCK, Turn};
    use wellfed_sdk::hal::owner::{
        Owner, OwnerConfig, OwnerFault, OwnerState, OwnerStatus, Step, SubmitResult,
    };
    use wellfed_sdk::{JournalSlots, Slot};

    use super::{CONFIG, JOURNAL, Opened, PowerCycle, Proof, Record, Recovery, drive_figures};

    type Journal = FeedJournal<JournalSlots>;

    /// The owner. Reached only through `&`: the supervision interrupts reach
    /// its board and supervisor while the loop runs, so no `&mut` may cover
    /// it once it is open. Filled once, before it opens.
    static OWNER: Slot<Owner<Journal>> = Slot::new();

    /// The feed in flight, from its submission until its result is recorded.
    #[derive(Clone, Copy, Debug)]
    struct Current {
        feed: u64,
        by: Source,
        /// The owner has reserved it in the journal.
        reserved: bool,
        /// The longest gap between two owner steps since it was queued.
        max_gap_ms: u32,
        /// Handshake operations the loop ran meanwhile.
        handshakes: u32,
    }

    /// The loop's half of the feeding owner: when to open it, and what it
    /// last logged.
    #[derive(Debug, Default)]
    pub struct Feeding {
        /// Opened, or the journal failed: nothing more to try this boot.
        settled: bool,
        /// The journal could not be opened or created: no feed this boot.
        journal_failed: bool,
        /// When the owner was last stepped.
        stepped_ms: Option<u32>,
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
        /// What the owner has shown, for a trial's confirmation.
        proof: Proof,
        /// When to restart to recover lost sensors.
        recovery: Recovery,
        /// Feeds this boot that began with a homing mark.
        homed: u32,
        /// Outlet-beam breaks this boot's feeds saw.
        outlet_breaks: u32,
    }

    impl Feeding {
        /// One pass; the owner's delay while it runs. A feed's end goes to
        /// the log (`record`), and its `Fed` event and its trace's summary
        /// to `outcome`.
        pub fn step(
            &mut self,
            mut record: impl FnMut(Record),
            mut outcome: impl FnMut(EventBody, Option<&TraceSummary>),
        ) -> Option<u32> {
            let now = wellfed_sdk::hal::sdk::uptime_ms();
            // While a reset storm holds the feeder the owner stays closed,
            // the outputs as startup left them, stopped. It opens once the
            // hold ends.
            if wellfed_sdk::held() {
                return None;
            }
            if !self.settled {
                if !wellfed_sdk::storage_armed() {
                    wellfed_sdk::report_feeding(self.proof.step(Opened::Not, now));
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
                        self.journal_failed = matches!(opened, Record::Journal { .. });
                        record(opened);
                    }
                    None => {}
                }
            }
            if self.journal_failed || !self.settled {
                let opened = if self.journal_failed {
                    Opened::JournalFailed
                } else {
                    Opened::Not
                };
                wellfed_sdk::report_feeding(self.proof.step(opened, now));
            }

            let owner = OWNER.enter()?;
            if let (Some(current), Some(last)) = (self.current.as_mut(), self.stepped_ms) {
                current.max_gap_ms = current.max_gap_ms.max(now.wrapping_sub(last));
            }
            self.stepped_ms = Some(now);
            let delay = match owner.step() {
                Step::Wait(ms) => Some(ms),
                Step::Stopped | Step::Refused => None,
            };
            // After the step, so the trace never delays a stop or a
            // motor decision.
            if let Some(seen) = owner.take_observation() {
                traces::observe(&seen);
            }

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

            self.follow(&owner, &status, &mut record, &mut outcome);
            self.release(&status, &mut record);
            if self.settled && !self.journal_failed {
                let opened = Opened::Owner {
                    status: &status,
                    resolving: self.interrupted.is_some(),
                };
                wellfed_sdk::report_feeding(self.proof.step(opened, now));
            }
            self.recover(&status, now, &mut record);
            delay
        }

        /// The one restart a power cycle allows to recover lost sensors
        /// (`Recovery`), through the deliberate restart every reboot takes,
        /// which no reset storm counts. A held boot never gets here: its
        /// owner stays closed.
        fn recover(&mut self, status: &OwnerStatus, now: u32, record: &mut impl FnMut(Record)) {
            use wellfed_sdk::hal::flash_lock::provider::BOOT_COUNT_REGISTER;
            use wellfed_sdk::hal::sdk;

            let held = status.state == OwnerState::Fault
                && status.fault == OwnerFault::Sensor
                && status.stopped
                && status.optics_off
                && self.current.is_none()
                && !wellfed_sdk::hal::ota::mailbox::installing();
            let cycle = if sdk::backup_read(BOOT_COUNT_REGISTER) <= 1 {
                PowerCycle::FirstBoot
            } else {
                PowerCycle::Restarted
            };
            let held_ms = self.recovery.held_ms(now);
            if self.recovery.step(now, held.then_some(cycle)) {
                record(Record::Recover { held_ms });
                wellfed_sdk::request_reset();
            }
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
            outcome: &mut impl FnMut(EventBody, Option<&TraceSummary>),
        ) {
            let Some(current) = self.current.as_mut() else {
                self.report_interrupted(owner, status, record, outcome);
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
            record(Record::Steps {
                feed: result.id,
                max_gap_ms: current.max_gap_ms,
                handshakes: current.handshakes,
            });
            let outlet_breaks = super::outlet_breaks(&status.trace.pins);
            if status.trace.homed_ms.is_some() {
                self.homed = self.homed.saturating_add(1);
            }
            self.outlet_breaks = self.outlet_breaks.saturating_add(outlet_breaks);
            record(Record::Marks {
                feed: result.id,
                homed_ms: status.trace.homed_ms,
                outlet_breaks,
                phase: status.phase,
            });
            if status.trace.pins.start.is_some() {
                record(Record::Pins {
                    feed: result.id,
                    pins: status.trace.pins,
                });
            }
            // The feed id fits the event: `submit` refused any that does not.
            let feed = u32::try_from(current.feed).unwrap_or(u32::MAX);
            let body = feed::fed(feed, &result, durability, current.by);
            let summary = traces::seal(&body, &drive_figures(&status.trace));
            outcome(body, summary.as_ref());
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
            outcome: &mut impl FnMut(EventBody, Option<&TraceSummary>),
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
            // Its trace, if it had one, was lost with the RAM it was in.
            let body = feed::fed(number, &status.result, Durability::Durable, Source::Feeder);
            outcome(body, None);
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
            // An install holds the flash; the owner refuses it as well.
            if self.current.is_some() || wellfed_sdk::hal::ota::mailbox::installing() {
                return Err(Refusal::Busy);
            }
            let clock = wellfed_sdk::hal::sdk::uptime_ms;
            let delay = wellfed_sdk::hal::sdk::delay_ms;
            super::claim_outputs(&INTERLOCK, super::writes_queued, clock, delay, |held| {
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
                    traces::begin(number);
                    self.current = Some(Current {
                        feed,
                        by,
                        reserved: false,
                        max_gap_ms: 0,
                        handshakes: 0,
                    });
                    // Gaps count from the first step after the feed was
                    // queued, not from one before it.
                    self.stepped_ms = None;
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
                // Not the feed in flight nor the last one unreserved: it was
                // reserved and has ended. A feed ends unreserved only when
                // the owner faults before reserving it, and a faulted owner
                // takes no feed until a restart (which empties the server's
                // memory of ids), so no later feed can have replaced it in
                // `unreserved`.
                _ => Some(Ok(())),
            }
        }

        /// Stop the feed `feed` if it is still running.
        pub fn cancel(&self, feed: u32) -> bool {
            let feed = u64::from(feed);
            self.current.is_some_and(|current| current.feed == feed)
                && OWNER.enter().is_some_and(|owner| owner.cancel(feed))
        }

        /// The feed whose motor the owner is driving, for the
        /// fault-injection variants.
        #[cfg(any(feature = "feed-stall", feature = "stop-fail"))]
        pub fn driving(&self) -> Option<crate::faults::Driving> {
            let status = OWNER.enter()?.status();
            (status.state == OwnerState::Feeding).then_some(crate::faults::Driving {
                feed: status.result.id,
                portions: status.result.requested,
            })
        }

        /// The feed the owner has, from its queueing until its result, for
        /// the jam and sensor-fault variants.
        #[cfg(any(feature = "feed-jam", feature = "sensor-fault"))]
        pub fn phase(&self) -> Option<crate::feed_faults::Phase> {
            let status = OWNER.enter()?.status();
            matches!(
                status.state,
                OwnerState::Queued | OwnerState::Preparing | OwnerState::Feeding
            )
            .then_some(crate::feed_faults::Phase {
                feed: status.result.id,
                portions: status.result.requested,
                driving: status.state == OwnerState::Feeding,
            })
        }

        /// The pass ran a handshake operation during the feed in flight.
        pub fn handshook(&mut self) {
            if let Some(current) = self.current.as_mut() {
                current.handshakes = current.handshakes.saturating_add(1);
            }
        }

        /// The owner's own reason not to start a feed now.
        pub fn blocked(&self) -> Option<Blocked> {
            if self.journal_failed {
                return Some(Blocked::Journal);
            }
            // While a reset storm holds the feeder, until the hold ends.
            if wellfed_sdk::held() {
                return Some(Blocked::Fault);
            }
            // A trial build feeds once confirmed: the confirmation must
            // never wait for a feed towards the trial's deadline.
            if wellfed_sdk::confirming() {
                return Some(Blocked::Starting);
            }
            let owner = OWNER.enter();
            let Some(status) = owner.as_ref().filter(|_| self.settled).map(|o| o.status()) else {
                return Some(Blocked::Starting);
            };
            match status.state {
                // A stop asked for (a restart on its way) serves no feed
                // either, until the next boot.
                OwnerState::Fault | OwnerState::Stopped => return Some(Blocked::Fault),
                OwnerState::Starting => return Some(Blocked::Starting),
                OwnerState::Idle
                | OwnerState::Queued
                | OwnerState::Preparing
                | OwnerState::Feeding
                | OwnerState::Reporting
                | OwnerState::Terminal => {}
            }
            if self.interrupted.is_some() {
                return Some(Blocked::Resolving);
            }
            if self.current.is_some() || status.state != OwnerState::Idle {
                return Some(Blocked::Feeding);
            }
            match INTERLOCK.turn() {
                Turn::Joining | Turn::FeedingJoinWaits => Some(Blocked::Joining),
                Turn::Free | Turn::Feeding => None,
            }
        }

        /// The food beam's latest reading, taken while a feed's emitters
        /// were on.
        pub fn food(&self) -> Option<bool> {
            OWNER.enter()?.status().food
        }

        /// Where the rotor rests, and this boot's homing feeds and outlet
        /// breaks, once the owner has opened.
        pub fn marks(&self) -> Option<Marks> {
            let owner = OWNER
                .enter()
                .filter(|_| self.settled && !self.journal_failed)?;
            let phase = match owner.status().phase {
                RotorPhase::AtMark => Phase::AtMark,
                RotorPhase::Unknown => Phase::Unknown,
            };
            Some(Marks {
                phase,
                homed: self.homed,
                outlet_breaks: self.outlet_breaks,
            })
        }

        /// A feed is queued or runs: no install starts.
        pub fn busy(&self) -> bool {
            self.current.is_some() || self.feeding()
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
            // The `owner-fail` variant: a build whose owner cannot open, so
            // its trial must not confirm. The journal is left untouched.
            if cfg!(feature = "owner-fail") {
                return Some(Record::Journal {
                    error: JournalError::Corrupt,
                    created: false,
                });
            }
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
