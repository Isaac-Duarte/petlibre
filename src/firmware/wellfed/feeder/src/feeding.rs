//! The feeding owner, stepped from the loop task.
//!
//! The owner opens once the runtime has armed the flash hooks: its journal
//! writes go through them, and their lease authority is the one that blocks
//! the owner's outputs during a lease. Until then, and if it never opens,
//! the outputs stay as startup left them (motor STOP, emitters OFF) and the
//! runtime's watchdog rule is unchanged. Nothing submits a feed yet, so the
//! owner sits idle: it holds the board, samples the inputs, and beats.
//!
//! The limits are provisional. The timing ones come from the first pulse
//! and the optics probe (a 200 ms motor lease, 10 ms supervision timer,
//! stock's 100 ms sensor settling); the feed ones (duration, reverse,
//! current) are placeholders that a measured feed replaces before any
//! request path exists.

use wellfed_core::feed::controller::FeedLimits;
use wellfed_core::feed::progress::BowlMode;
use wellfed_core::feed::sensor_inputs::SensorLimits;
use wellfed_core::store::feed_journal::JournalConfig;
use wellfed_sdk::hal::owner::OwnerConfig;
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
        feedback_inhibit_ms: 200,
        feedback_timeout_ms: 3000,
        direction_pause_ms: 500,
        reverse_ms: 500,
        max_duration_ms: 10_000,
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
    optics_max_on_ms: 12_000,
    motor_deadtime_ms: 50,
    motor_lease_ms: 200,
    adc_poll_limit: 10_000,
    wake_lock_id: sdk::PMU_USER_BASE,
    flash: core::ptr::null_mut(),
};

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
    use wellfed_core::store::feed_journal::{FeedJournal, JournalError};
    use wellfed_sdk::hal::owner::{Owner, OwnerConfig, OwnerFault, OwnerState, OwnerStatus, Step};
    use wellfed_sdk::hal::supervision::events::EventsFault;
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
    }

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
        }
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
            delay
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
