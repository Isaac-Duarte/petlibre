//! Owner fixture: a booted owner over fake hardware, with scripted storage
//! callbacks and an external flash authority.

use core::ptr;

use super::*;
use wellfed_core::feed::controller::{FeedLimits, FeedRequest};
use wellfed_core::feed::progress::{BowlMode, Direction};
use wellfed_core::feed::sensor_inputs::SensorLimits;
use wellfed_core::feed::storage::{OwnerOutcome, OwnerRecovery, OwnerResult, OwnerStorage};
use wellfed_core::flash_coord::{FlashCoord, FlashParty, FlashReply};
use wellfed_core::store::feed_journal::{FeedJournal, JournalConfig, JournalError};
use wellfed_core::supply::FloorLimits;
use wellfed_hal::board::MOTOR_MASK;
use wellfed_hal::owner::{
    Owner, OwnerConfig, OwnerState, OwnerStatus, OwnerTask, Step, SubmitResult,
};
use wellfed_hal::sdk::{self, fake};
use wellfed_hal::supervision::events::EventsConfig;
use wellfed_testkit::journal_flash::{FakeJournalFlash, FlashIo};

pub const CONFIG: OwnerConfig = OwnerConfig {
    feed: FeedLimits {
        feedback_inhibit_ms: 5,
        feedback_timeout_ms: 30,
        direction_pause_ms: 3,
        reverse_ms: 3,
        max_duration_ms: 120,
        portion_ms: 120,
        max_observation_gap_ms: 4,
        // Past the longest feed: the fake's PA18 rests LOW, and the tests
        // that time these set their own.
        mark_gap_ms: 1000,
        mark_low_ms: 1000,
        current_limit_ma: 300,
        current_max_ma: 1000,
        surge_ms: 0,
        max_portions: 3,
        max_reverse_attempts: 0,
    },
    sensors: SensorLimits {
        optical_settle_ms: 2,
        food_low_delay_ms: 5,
        outlet_block_delay_ms: 5,
        power_settle_ms: 2,
        max_observation_gap_ms: 4,
        max_age_ms: 2,
        battery: FloorLimits {
            floor: 1650,
            clear: 1710,
            cutoff: 1550,
            hold_ms: 4,
        },
    },
    events: EventsConfig {
        ticks_per_second: 32_000,
        timer_period_ms: 2,
        max_feedback_age_ms: 2,
        feedback_capacity: 8,
    },
    max_progress_gap_ms: 50,
    idle_gap_ms: 50,
    bowl_mode: BowlMode::Single,
    sample_period_ms: 1,
    prepare_timeout_ms: 20,
    optics_max_on_ms: 140,
    motor_deadtime_ms: 2,
    motor_lease_ms: 12,
    adc_poll_limit: 8,
    wake_lock_id: 16,
    flash: ptr::null_mut(),
};

pub const PHASE_KNOWN: OwnerRecovery = OwnerRecovery {
    high_water_id: 0,
    direction: Direction::D0,
    credit_next_event: true,
    phase_known: true,
    pending_request: false,
};

pub struct Fixture {
    pub owner: Owner<FixtureStorage>,
    pub config: OwnerConfig,
    pub authority: FlashCoord,
    pub disk: OwnerRecovery,
    pub saved_result: OwnerResult,
    /// The last result storage holds, for the phase at boot: a completed
    /// feed unless a test says otherwise.
    pub last_result: Option<OwnerResult>,

    pub recover_calls: u32,
    pub reserve_calls: u32,
    pub finish_calls: u32,
    pub finish_refreshes: u32,
    pub resolve_calls: u32,

    pub recover_fail: bool,
    pub reserve_fail: bool,
    pub finish_fail: bool,
    pub resolve_fail: bool,
    pub cancel_in_reserve: bool,
    pub stop_in_finish: bool,

    pub reserve_delay_ticks: u32,
    pub recover_delay_ticks: u32,
    pub finish_delay_ticks: u32,
    pub reserve_mask_irqs: bool,

    pub flash: FakeJournalFlash,
    pub journal: FeedJournal<FlashIo>,
    pub use_journal: bool,
}

impl Fixture {
    pub fn new(disk: OwnerRecovery) -> Self {
        Self {
            owner: Owner::default(),
            config: CONFIG,
            authority: FlashCoord::default(),
            disk,
            saved_result: OwnerResult::default(),
            last_result: Some(OwnerResult {
                outcome: OwnerOutcome::Done,
                motion_possible: true,
                ..OwnerResult::default()
            }),

            recover_calls: 0,
            reserve_calls: 0,
            finish_calls: 0,
            finish_refreshes: 0,
            resolve_calls: 0,

            recover_fail: false,
            reserve_fail: false,
            finish_fail: false,
            resolve_fail: false,
            cancel_in_reserve: false,
            stop_in_finish: false,

            reserve_delay_ticks: 0,
            recover_delay_ticks: 0,
            finish_delay_ticks: 0,
            reserve_mask_irqs: false,

            flash: FakeJournalFlash::default(),
            journal: FeedJournal::default(),
            use_journal: false,
        }
    }

    pub fn status(&self) -> OwnerStatus {
        self.owner.status()
    }

    /// The owner task's state, between steps.
    pub fn task(&mut self) -> &mut OwnerTask<FixtureStorage> {
        // SAFETY: the owner is stepped only inside `step`, on this thread, so
        // no other borrow is live here.
        unsafe { &mut *self.owner.task_state() }
    }

    /// One owner step, as the stepping task's pass would run it; the step
    /// must leave interrupts enabled.
    pub fn step(&self) -> Step {
        let step = self.owner.step();
        assert_eq!(
            hw(|h| h.interrupts),
            0,
            "a step returns with interrupts enabled"
        );
        step
    }

    pub fn storage(&mut self) -> FixtureStorage {
        FixtureStorage(ptr::from_mut(self))
    }

    pub fn bind_authority(&mut self) {
        self.authority = *flash_ready();
        self.config.flash = &raw mut self.authority;
    }

    pub fn try_start(&mut self, config: &OwnerConfig) -> bool {
        let storage = self.storage();
        // SAFETY: the boxed fixture holds the owner, authority and storage context in place.
        unsafe { self.owner.open(config, storage) }
    }

    pub fn create(&mut self) {
        if self.config.flash.is_null() {
            self.bind_authority();
        }
        let config = self.config;
        assert!(self.try_start(&config));
    }

    pub fn advance(&mut self, ms: u32) {
        for _ in 0..ms {
            fake::advance(32, true);
            self.step();
        }
    }

    pub fn stop(&mut self) {
        self.owner.stop();
        self.step();
        let s = self.status();
        assert!(s.state == OwnerState::Stopped && s.stopped && s.optics_off);
        assert!(!self.owner.running() && hw(|h| h.wake_locks) == 0 && sdk::tim2_en() == 0);
    }

    pub fn start_feed(&mut self) {
        assert_eq!(self.owner.submit(1, 1), SubmitResult::Enqueued);
        assert!(self.status().state == OwnerState::Queued && !self.status().intent_reserved);
        self.step();
        self.advance(5);
        assert!(self.status().state == OwnerState::Feeding && self.status().intent_reserved);
        assert_eq!(motor_pins(), 1 << 13);
    }

    pub fn complete(&mut self) {
        self.advance(4);
        fake::advance(32, true);
        fake::edge(true);
        self.step();
    }

    pub fn hold(&mut self) {
        assert_eq!(
            self.authority.request(FlashParty::Peer, 29, 1, 0),
            FlashReply::Accepted
        );
    }

    pub fn release_hold(&mut self) {
        let t = self.authority.token;
        let inactive = self.owner.board().flash_quiesce();
        let c = &mut self.authority;
        assert_eq!(c.quiesced(&t, inactive, 1), FlashReply::Accepted);
        assert_eq!(c.parked(&t, FlashParty::Local, 2), FlashReply::Accepted);
        assert_eq!(c.complete(&t, FlashParty::Peer, 3), FlashReply::Accepted);
        assert_eq!(
            c.released(&t, FlashParty::Local, true, 4),
            FlashReply::Accepted
        );
    }
}

/// The storage callbacks below run only while outputs are inactive and
/// interrupts are enabled; each one asserts it before touching the disk.
pub fn inactive_outputs() {
    assert_eq!(hw(|h| h.interrupts), 0);
    assert_eq!(motor_pins(), MOTOR_MASK);
    assert!(optics_off());
}

/// The fixture's scripted storage, reached through the boxed fixture, which
/// keeps the owner and its storage in place while the owner runs.
#[derive(Debug)]
pub struct FixtureStorage(*mut Fixture);

impl OwnerStorage for FixtureStorage {
    fn recover(&mut self, recovery: &mut OwnerRecovery) -> bool {
        // SAFETY: as the type says.
        unsafe { recover(self.0, recovery) }
    }

    fn reserve(&mut self, request: &FeedRequest) -> bool {
        // SAFETY: as the type says.
        unsafe { reserve(self.0, request) }
    }

    fn finish(&mut self, result: &OwnerResult, phase: &OwnerRecovery) -> bool {
        // SAFETY: as the type says.
        unsafe { finish_record(self.0, result, phase) }
    }

    fn resolve(&mut self) -> Option<OwnerResult> {
        // SAFETY: as the type says.
        unsafe { resolve(self.0) }
    }

    fn last_result(&self) -> Option<OwnerResult> {
        // SAFETY: as the type says.
        unsafe {
            if (*self.0).use_journal {
                return (*self.0).journal.last_result();
            }
            (*self.0).last_result
        }
    }
}

unsafe fn recover(f: *mut Fixture, value: &mut OwnerRecovery) -> bool {
    inactive_outputs();
    // SAFETY: the fixture keeps the owner and its storage in place while it runs.
    unsafe {
        (*f).recover_calls += 1;
        if (*f).recover_delay_ticks != 0 {
            fake::advance((*f).recover_delay_ticks, true);
        }

        if (*f).use_journal {
            let Some(view) = (*f).journal.read() else {
                return false;
            };
            *value = view.recovery;
            (*f).disk = view.recovery;
            return true;
        }

        *value = (*f).disk;
        !(*f).recover_fail
    }
}

unsafe fn reserve(f: *mut Fixture, request: &FeedRequest) -> bool {
    inactive_outputs();
    // SAFETY: the fixture keeps the owner and its storage in place while it runs.
    unsafe {
        (*f).reserve_calls += 1;
        assert!(!(*f).disk.pending_request && request.id > (*f).disk.high_water_id);

        if (*f).reserve_delay_ticks != 0 {
            let masked = (*f).reserve_mask_irqs;
            if masked {
                hw(|h| h.interrupts = 1);
            }
            fake::advance((*f).reserve_delay_ticks, !masked);
            hw(|h| h.interrupts = 0);
        }

        if (*f).cancel_in_reserve {
            assert!((*f).owner.cancel(request.id));
        }
        if (*f).reserve_fail {
            return false;
        }
        if (*f).use_journal && !(*f).journal.reserve(request) {
            return false;
        }
        (*f).disk.high_water_id = request.id;
        (*f).disk.pending_request = true;
        true
    }
}

unsafe fn finish_record(f: *mut Fixture, result: &OwnerResult, phase: &OwnerRecovery) -> bool {
    inactive_outputs();
    // SAFETY: the fixture keeps the owner and its storage in place while it runs.
    unsafe {
        (*f).finish_calls += 1;
        (*f).finish_refreshes = beats();
        if (*f).finish_delay_ticks != 0 {
            fake::advance((*f).finish_delay_ticks, true);
        }

        assert!((*f).disk.pending_request && result.id == (*f).disk.high_water_id);
        assert!(!phase.pending_request && phase.high_water_id == result.id);
        if (*f).stop_in_finish {
            (*f).owner.stop();
        }

        if (*f).finish_fail {
            return false;
        }
        if (*f).use_journal && !(*f).journal.finish(result, phase) {
            return false;
        }
        (*f).saved_result = *result;
        (*f).last_result = Some(*result);
        (*f).disk = *phase;
        true
    }
}

unsafe fn resolve(f: *mut Fixture) -> Option<OwnerResult> {
    inactive_outputs();
    // SAFETY: the fixture keeps the owner and its storage in place while it runs.
    unsafe {
        (*f).resolve_calls += 1;
        assert!(
            (*f).disk.pending_request,
            "only a pending reservation resolves"
        );
        if (*f).resolve_fail {
            return None;
        }

        let result = if (*f).use_journal {
            (*f).journal.resolve()?
        } else {
            OwnerResult {
                id: (*f).disk.high_water_id,
                requested: 1,
                outcome: OwnerOutcome::Uncertain,
                motion_possible: true,
                ..OwnerResult::default()
            }
        };
        (*f).disk.pending_request = false;
        (*f).saved_result = result;
        (*f).last_result = Some(result);
        Some(result)
    }
}

pub fn boot() -> Box<Fixture> {
    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(PHASE_KNOWN));
    f.create();
    assert_eq!(f.status().state, OwnerState::Starting);

    f.step();
    let s = f.status();
    assert!(s.state == OwnerState::Idle && s.stopped && s.optics_off);
    assert_eq!(hw(|h| h.wake_locks), 1 << 16);
    f
}

pub fn boot_with_journal(dual: bool) -> Box<Fixture> {
    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(OwnerRecovery::default()));
    f.use_journal = true;
    if dual {
        f.config.bowl_mode = BowlMode::Double;
    }

    // SAFETY: the boxed fixture keeps the fake flash in place.
    let io = unsafe { f.flash.io() };
    let c = JournalConfig {
        binding: 42,
        mode: f.config.bowl_mode,
        max_portions: CONFIG.feed.max_portions,
    };
    assert_eq!(
        f.journal.create(io, c, Direction::D0, true, true),
        JournalError::Ok
    );

    f.create();
    f.step();
    assert_eq!(f.status().state, OwnerState::Idle);
    f
}

pub fn restart_with_journal(f: &mut Fixture) {
    assert!(!f.owner.running());
    fake::reset_hardware();
    f.flash.budget = -1;
    // SAFETY: as for `boot_with_journal`.
    let io = unsafe { f.flash.io() };
    let config = f.journal.config;
    assert_eq!(f.journal.open(io, config), JournalError::Ok);
    f.create();
    f.step();
}
