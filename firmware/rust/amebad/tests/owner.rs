mod common;

use core::ffi::c_void;
use core::ptr;

use common::*;
use petlibro_amebad::board::MOTOR_MASK;
use petlibro_amebad::events::EventsConfig;
use petlibro_amebad::owner::{
    Owner, OwnerConfig, OwnerFault, OwnerState, OwnerStatus, SubmitResult,
};
use petlibro_amebad::sdk::{self, fake};
use petlibro_amebad::watchdog::{WatchdogConfig, WatchdogFault};
use petlibro_core::feed_controller::{FeedLimits, FeedRequest};
use petlibro_core::feed_journal::{FeedJournal, JournalConfig, JournalError, RECORD_BYTES};
use petlibro_core::feed_progress::{BowlMode, Direction};
use petlibro_core::feed_storage::{OwnerOutcome, OwnerRecovery, OwnerResult, OwnerStorage};
use petlibro_core::flash_coord::{
    FlashCoord, FlashFault, FlashLimits, FlashParty, FlashPhase, FlashReply,
};
use petlibro_core::sensor_inputs::SensorLimits;
use petlibro_testkit::journal_flash::{FakeJournalFlash, UNIT};

const CONFIG: OwnerConfig = OwnerConfig {
    feed: FeedLimits {
        feedback_inhibit_ms: 5,
        feedback_timeout_ms: 30,
        direction_pause_ms: 3,
        reverse_ms: 3,
        max_duration_ms: 120,
        max_observation_gap_ms: 4,
        current_limit_ma: 300,
        max_portions: 3,
        max_reverse_attempts: 0,
    },
    sensors: SensorLimits {
        optical_settle_ms: 2,
        food_empty_delay_ms: 5,
        outlet_block_delay_ms: 5,
        power_settle_ms: 2,
        max_observation_gap_ms: 4,
        max_age_ms: 2,
        battery_min_scaled: 1650,
    },
    events: EventsConfig {
        ticks_per_second: 32_000,
        timer_period_ms: 2,
        max_feedback_age_ms: 2,
        feedback_capacity: 8,
    },
    watchdog: WatchdogConfig {
        systimer_hz: 32_000,
        clock_min_hz: 32_000,
        clock_max_hz: 32_000,
        min_refresh_ms: 1,
        max_progress_gap_ms: 50,
        max_expiry_ms: 126,
        count_id: 5,
        divisor: 63,
    },
    bowl_mode: BowlMode::SINGLE,
    sample_period_ms: 1,
    prepare_timeout_ms: 20,
    optics_max_on_ms: 140,
    motor_deadtime_ms: 2,
    motor_lease_ms: 12,
    adc_poll_limit: 8,
    wake_lock_id: 16,
    flash: ptr::null_mut(),
};

const PHASE_KNOWN: OwnerRecovery = OwnerRecovery {
    high_water_id: 0,
    direction: Direction::D0,
    credit_next_event: true,
    phase_known: true,
    pending_request: false,
};

struct Fixture {
    owner: Owner,
    config: OwnerConfig,
    authority: FlashCoord,
    disk: OwnerRecovery,
    saved_result: OwnerResult,
    recover_calls: u32,
    reserve_calls: u32,
    finish_calls: u32,
    finish_refreshes: u32,
    recover_fail: bool,
    reserve_fail: bool,
    finish_fail: bool,
    cancel_in_reserve: bool,
    stop_in_finish: bool,
    reserve_delay_ticks: u32,
    recover_delay_ticks: u32,
    finish_delay_ticks: u32,
    reserve_mask_irqs: bool,
    flash: FakeJournalFlash,
    journal: FeedJournal,
    use_journal: bool,
}

impl Fixture {
    fn new(disk: OwnerRecovery) -> Self {
        Self {
            owner: Owner::default(),
            config: CONFIG,
            authority: FlashCoord::default(),
            disk,
            saved_result: OwnerResult::default(),
            recover_calls: 0,
            reserve_calls: 0,
            finish_calls: 0,
            finish_refreshes: 0,
            recover_fail: false,
            reserve_fail: false,
            finish_fail: false,
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

    fn status(&self) -> OwnerStatus {
        self.owner.status()
    }

    fn storage(&mut self) -> OwnerStorage {
        OwnerStorage {
            context: ptr::from_mut(self).cast(),
            recover: Some(recover),
            reserve: Some(reserve),
            finish: Some(finish_record),
        }
    }

    fn bind_authority(&mut self) {
        self.authority = *flash_ready();
        self.config.flash = &raw mut self.authority;
    }

    fn try_start(&mut self, config: &OwnerConfig) -> bool {
        let storage = self.storage();
        // SAFETY: the boxed fixture holds the owner, authority and storage context in place.
        unsafe { self.owner.start(config, &storage) }
    }

    fn create(&mut self) {
        if self.config.flash.is_null() {
            self.bind_authority();
        }
        let config = self.config;
        assert!(self.try_start(&config));
    }

    fn advance(&mut self, ms: u32) {
        for _ in 0..ms {
            fake::advance(32, true);
            fake::run_task();
        }
    }

    fn stop(&mut self) {
        self.owner.stop();
        fake::run_task();
        let s = self.status();
        assert!(s.state == OwnerState::STOPPED && s.stopped && s.optics_off);
        assert!(!hw(|h| h.task_live) && hw(|h| h.wake_locks) == 0 && sdk::tim2_en() == 0);
    }

    fn start_feed(&mut self) {
        assert_eq!(self.owner.submit(1, 1), SubmitResult::ENQUEUED);
        assert!(self.status().state == OwnerState::QUEUED && !self.status().intent_reserved);
        fake::run_task();
        self.advance(5);
        assert!(self.status().state == OwnerState::FEEDING && self.status().intent_reserved);
        assert_eq!(motor_pins(), 1 << 13);
    }

    fn complete(&mut self) {
        self.advance(4);
        fake::advance(32, true);
        fake::edge(true);
        fake::run_task();
    }

    fn hold(&mut self) {
        assert_eq!(
            self.authority.request(FlashParty::PEER, 29, 1, 0),
            FlashReply::ACCEPTED
        );
    }

    fn release_hold(&mut self) {
        let t = self.authority.token;
        let inactive = self.owner.board.flash_quiesce();
        let c = &mut self.authority;
        assert_eq!(c.quiesced(&t, inactive, 1), FlashReply::ACCEPTED);
        assert_eq!(c.parked(&t, FlashParty::LOCAL, 2), FlashReply::ACCEPTED);
        assert_eq!(c.complete(&t, FlashParty::PEER, 3), FlashReply::ACCEPTED);
        assert_eq!(
            c.released(&t, FlashParty::LOCAL, true, 4),
            FlashReply::ACCEPTED
        );
    }
}

fn inactive_outputs() {
    assert_eq!(hw(|h| h.interrupts), 0);
    assert_eq!(motor_pins(), MOTOR_MASK);
    assert!(optics_off());
}

unsafe extern "C" fn recover(context: *mut c_void, value: *mut OwnerRecovery) -> bool {
    let f = context.cast::<Fixture>();
    inactive_outputs();
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

unsafe extern "C" fn reserve(context: *mut c_void, request: *const FeedRequest) -> bool {
    let f = context.cast::<Fixture>();
    inactive_outputs();
    unsafe {
        let request = &*request;
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

unsafe extern "C" fn finish_record(
    context: *mut c_void,
    result: *const OwnerResult,
    phase: *const OwnerRecovery,
) -> bool {
    let f = context.cast::<Fixture>();
    inactive_outputs();
    unsafe {
        let (result, phase) = (&*result, &*phase);
        (*f).finish_calls += 1;
        (*f).finish_refreshes = hw(|h| h.watchdog_refreshes);
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
        (*f).disk = *phase;
        true
    }
}

fn boot() -> Box<Fixture> {
    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(PHASE_KNOWN));
    f.create();
    assert_eq!(f.status().state, OwnerState::STARTING);
    fake::run_task();
    let s = f.status();
    assert!(s.state == OwnerState::IDLE && s.stopped && s.optics_off);
    assert_eq!(hw(|h| h.wake_locks), 1 << 16);
    f
}

#[test]
fn serialized_lifecycle_and_durable_success() {
    let mut f = boot();
    assert_eq!(f.owner.submit(0, 1), SubmitResult::REJECT_ID);
    assert_eq!(f.owner.submit(1, 0), SubmitResult::REJECT_REQUEST);
    f.start_feed();
    assert_eq!(f.owner.submit(2, 1), SubmitResult::REJECT_BUSY);
    assert!(!f.owner.cancel(99));
    f.complete();
    let s = f.status();
    assert!(s.state == OwnerState::TERMINAL && s.result_durable && s.stopped && s.optics_off);
    assert!(
        s.result.outcome == OwnerOutcome::DONE
            && s.result.completed == 1
            && s.result.motion_possible
    );
    assert!(f.reserve_calls == 1 && f.finish_calls == 1 && !f.disk.pending_request);
    assert!(f.owner.acknowledge(1));
    f.advance(1);
    assert_eq!(f.owner.submit(1, 1), SubmitResult::REJECT_ID);
    assert_eq!(f.owner.submit(2, 1), SubmitResult::ENQUEUED);
    assert!(f.owner.cancel(2));
    fake::run_task();
    assert!(
        f.status().result.outcome == OwnerOutcome::CANCELLED && !f.status().result.motion_possible
    );
    assert!(f.finish_calls == 2 && f.disk.high_water_id == 2);
    assert!(!f.owner.acknowledge(1));
    assert!(f.owner.acknowledge(2));
    f.advance(1);
    assert_eq!(f.status().state, OwnerState::IDLE);
    f.stop();
}

#[test]
fn reservation_failure_cannot_move_outputs_or_retry() {
    let mut f = boot();
    f.reserve_fail = true;
    assert_eq!(f.owner.submit(1, 1), SubmitResult::ENQUEUED);
    fake::run_task();
    assert!(f.status().state == OwnerState::FAULT && f.status().fault == OwnerFault::STORAGE);
    assert!(!f.status().result.motion_possible && !f.status().result_durable);
    assert!(f.reserve_calls == 1 && f.finish_calls == 0);
    f.advance(5);
    assert!(f.reserve_calls == 1 && f.status().stopped && f.status().optics_off);
    assert!(!f.owner.acknowledge(1));
    f.stop();
}

#[test]
fn slow_reservation_expires_and_cancellation_during_callback_wins() {
    let mut f = boot();
    f.reserve_delay_ticks = 25 * 32;
    assert_eq!(f.owner.submit(1, 1), SubmitResult::ENQUEUED);
    fake::run_task();
    assert!(f.status().state == OwnerState::TERMINAL && f.status().result_durable);
    assert!(
        f.status().result.outcome == OwnerOutcome::EXPIRED && !f.status().result.motion_possible
    );
    f.stop();
    let mut f = boot();
    f.cancel_in_reserve = true;
    assert_eq!(f.owner.submit(1, 1), SubmitResult::ENQUEUED);
    fake::run_task();
    assert!(f.status().result.outcome == OwnerOutcome::CANCELLED && f.status().result_durable);
    assert!(!f.status().result.motion_possible);
    f.stop();
}

#[test]
fn interrupted_recovery_blocks_every_new_request() {
    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(OwnerRecovery {
        high_water_id: 7,
        pending_request: true,
        ..OwnerRecovery::default()
    }));
    f.create();
    fake::run_task();
    assert!(f.status().state == OwnerState::FAULT && f.status().fault == OwnerFault::RECOVERY);
    assert_eq!(f.owner.submit(8, 1), SubmitResult::REJECT_STOPPED);
    assert!(
        f.reserve_calls == 0 && f.finish_calls == 0 && f.status().stopped && f.status().optics_off
    );
    f.stop();
}

#[test]
fn finish_failure_is_not_reported_as_durable_success() {
    let mut f = boot();
    f.start_feed();
    f.finish_fail = true;
    f.complete();
    assert!(f.status().state == OwnerState::FAULT && !f.status().result_durable);
    assert!(
        f.disk.pending_request
            && f.finish_calls == 1
            && f.status().stopped
            && f.status().optics_off
    );
    f.advance(4);
    assert_eq!(f.finish_calls, 1);
    f.stop();
}

fn cancel_hook(context: *mut c_void) {
    let f = context.cast::<Fixture>();
    assert!(unsafe { (*f).owner.cancel(1) });
}

#[test]
fn cancellation_during_acquisition_precedes_feedback() {
    let mut f = boot();
    f.start_feed();
    f.advance(4);
    fake::advance(32, true);
    fake::edge(true);
    let context: *mut c_void = ptr::from_mut(&mut *f).cast();
    hw(|h| {
        h.adc_hook = Some(cancel_hook);
        h.adc_hook_context = context;
    });
    fake::run_task();
    assert!(f.status().state == OwnerState::TERMINAL && f.status().result_durable);
    assert!(
        f.status().result.outcome == OwnerOutcome::CANCELLED && f.status().result.completed == 0
    );
    assert!(f.owner.acknowledge(1));
    f.advance(100); // Healthy owner continues servicing the watchdog.
    assert_eq!(f.status().state, OwnerState::IDLE); // Cancelled edges cannot poison a later epoch.
    f.stop();
}

#[test]
fn slow_adc_and_stalled_owner_stop_through_the_supervisor() {
    let mut f = boot();
    f.start_feed();
    hw(|h| h.adc_delay_ticks = 12 * 32);
    fake::run_task();
    assert!(f.status().state == OwnerState::FAULT && f.status().stopped && f.status().optics_off);
    assert!(f.status().result.outcome == OwnerOutcome::FAILED && f.status().result_durable);
    f.stop();
    let mut f = boot();
    assert_eq!(f.owner.submit(1, 1), SubmitResult::ENQUEUED);
    fake::run_task();
    f.advance(2); // IR on, still preparing.
    assert!(!f.status().optics_off);
    fake::advance(12 * 32, true);
    assert!(optics_off());
    fake::run_task();
    assert!(f.status().state == OwnerState::FAULT && f.status().stopped);
    f.stop();
}

#[test]
fn stop_waits_for_verified_outputs_and_retains_context() {
    let mut f = boot();
    f.start_feed();
    hw(|h| h.blocked_motor_write = true);
    f.owner.stop();
    fake::run_task();
    assert!(hw(|h| h.task_live) && sdk::tim2_en() != 0 && hw(|h| h.wake_locks) != 0);
    assert!(f.status().state == OwnerState::REPORTING && f.finish_calls == 0);
    hw(|h| h.blocked_motor_write = false);
    fake::advance(64, true);
    fake::run_task();
    assert!(f.status().state == OwnerState::STOPPED && f.status().stopped);
    assert!(!hw(|h| h.task_live) && hw(|h| h.wake_locks) == 0 && f.finish_calls == 1);
}

#[test]
fn ambiguous_double_phase_is_never_assumed_after_cancel() {
    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(PHASE_KNOWN));
    f.config.bowl_mode = BowlMode::DOUBLE;
    f.create();
    fake::run_task();
    f.start_feed();
    assert!(f.owner.cancel(1));
    fake::run_task();
    assert!(f.status().state == OwnerState::TERMINAL && !f.status().phase_known);
    assert!(f.owner.acknowledge(1));
    f.advance(1);
    assert_eq!(f.owner.submit(2, 1), SubmitResult::REJECT_PHASE);
    f.stop();
}

#[test]
fn failed_startup_and_invalid_configuration_do_not_claim_live_resources() {
    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(OwnerRecovery::default()));
    f.bind_authority();
    assert!(!f.try_start(&OwnerConfig {
        wake_lock_id: 17,
        ..f.config
    }));
    assert!(!f.try_start(&OwnerConfig {
        optics_max_on_ms: 120,
        ..f.config
    }));
    hw(|h| h.task_create_fail = true);
    let config = f.config;
    assert!(!f.try_start(&config));
    assert!(!hw(|h| h.task_live) && hw(|h| h.wake_locks) == 0 && hw(|h| h.output[0]) == 0);
    hw(|h| {
        h.task_create_fail = false;
        h.efuse_fail = true;
    });
    f.create();
    fake::run_task();
    assert!(f.status().state == OwnerState::FAULT && f.recover_calls == 0);
    f.stop();

    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(OwnerRecovery::default()));
    f.create();
    f.owner.stop(); // Cancel before the task ever opens hardware.
    fake::run_task();
    assert!(f.status().state == OwnerState::STOPPED && !hw(|h| h.task_live));
    assert!(f.recover_calls == 0 && hw(|h| h.output[0]) == 0 && hw(|h| h.wake_locks) == 0);

    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(OwnerRecovery::default()));
    hw(|h| h.blocked_motor_write = true);
    f.create();
    fake::run_task();
    assert!(f.status().state == OwnerState::FAULT && !f.status().stopped);
    f.owner.stop();
    fake::run_task();
    assert!(hw(|h| h.task_live) && hw(|h| h.wake_locks) != 0);
    hw(|h| h.blocked_motor_write = false);
    fake::run_task();
    assert!(f.status().state == OwnerState::STOPPED && f.status().stopped && f.status().optics_off);
    assert!(!hw(|h| h.task_live) && hw(|h| h.wake_locks) == 0);
}

#[test]
fn optical_cleanup_and_failed_completion_cannot_publish_success() {
    let mut f = boot();
    f.start_feed();
    hw(|h| h.blocked_optics_off = true);
    f.owner.stop();
    fake::run_task();
    assert!(
        f.status().state == OwnerState::REPORTING && f.status().stopped && !f.status().optics_off
    );
    assert!(f.finish_calls == 0 && hw(|h| h.task_live) && hw(|h| h.wake_locks) != 0);
    hw(|h| h.blocked_optics_off = false);
    fake::advance(64, true);
    fake::run_task();
    assert!(
        f.status().state == OwnerState::STOPPED && f.status().optics_off && f.finish_calls == 1
    );
    let mut f = boot();
    f.start_feed();
    hw(|h| h.blocked_motor_write = true);
    f.complete();
    assert!(
        f.status().state == OwnerState::REPORTING
            && !f.status().result_durable
            && f.finish_calls == 0
    );
    hw(|h| h.blocked_motor_write = false);
    fake::advance(64, true);
    fake::run_task();
    assert!(f.status().state == OwnerState::FAULT && f.status().result_durable);
    assert!(f.status().result.completed == 1 && f.status().result.outcome == OwnerOutcome::FAILED);
    f.stop();
}

#[test]
fn stop_during_result_write_and_singleton_ownership() {
    let mut f = boot();
    let mut second = Box::<Owner>::default();
    let storage = f.storage();
    assert!(!unsafe { second.start(&f.config, &storage) });
    f.start_feed();
    f.stop_in_finish = true;
    f.complete();
    assert!(f.status().state == OwnerState::STOPPED && f.status().result_durable);
    assert!(!hw(|h| h.task_live) && hw(|h| h.wake_locks) == 0);
}

fn boot_with_journal(dual: bool) -> Box<Fixture> {
    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(OwnerRecovery::default()));
    f.use_journal = true;
    if dual {
        f.config.bowl_mode = BowlMode::DOUBLE;
    }
    let io = f.flash.io();
    let c = JournalConfig {
        binding: 42,
        mode: f.config.bowl_mode,
        max_portions: CONFIG.feed.max_portions,
    };
    // SAFETY: the boxed fixture keeps the fake flash in place.
    assert_eq!(
        unsafe { f.journal.create(io, c, Direction::D0, true, true) },
        JournalError::OK
    );
    f.create();
    fake::run_task();
    assert_eq!(f.status().state, OwnerState::IDLE);
    f
}

fn restart_with_journal(f: &mut Fixture) {
    assert!(!hw(|h| h.task_live));
    fake::reset_hardware();
    f.flash.budget = -1;
    let (io, config) = (f.journal.io, f.journal.config);
    // SAFETY: as for `boot_with_journal`.
    assert_eq!(unsafe { f.journal.open(io, config) }, JournalError::OK);
    f.create();
    fake::run_task();
}

#[test]
fn journal_success_and_reboot_preserve_ids() {
    let mut f = boot_with_journal(false);
    f.start_feed();
    assert!(f.journal.view.recovery.pending_request && f.journal.view.recovery.high_water_id == 1);
    f.complete();
    assert!(f.status().state == OwnerState::TERMINAL && f.status().result_durable);
    assert!(f.journal.view.has_result && f.journal.view.result.outcome == OwnerOutcome::DONE);
    assert_eq!(f.journal.view.result.completed, 1);
    f.stop();
    restart_with_journal(&mut f);
    assert!(f.status().state == OwnerState::IDLE && f.status().high_water_id == 1);
    assert_eq!(f.owner.submit(1, 1), SubmitResult::REJECT_ID);
    assert_eq!(f.owner.submit(2, 1), SubmitResult::ENQUEUED);
    assert!(f.owner.cancel(2));
    fake::run_task();
    assert!(f.status().result_durable && f.journal.view.result.id == 2);
    assert!(
        f.journal.view.result.outcome == OwnerOutcome::CANCELLED
            && !f.status().result.motion_possible
    );
    f.stop();

    let mut f = boot_with_journal(true);
    f.start_feed();
    f.complete();
    assert!(f.status().state == OwnerState::TERMINAL && f.status().result_durable);
    let r = f.journal.view.recovery;
    assert!(r.direction == Direction::D0 && r.credit_next_event && r.phase_known);
    assert!(f.owner.acknowledge(1));
    f.advance(1);
    assert_eq!(f.owner.submit(2, 1), SubmitResult::ENQUEUED);
    f.advance(6);
    assert_eq!(f.status().state, OwnerState::FEEDING);
    assert!(f.owner.cancel(2));
    fake::run_task();
    assert!(f.status().result_durable && !f.journal.view.recovery.phase_known);
    f.stop();
    restart_with_journal(&mut f);
    assert!(f.status().state == OwnerState::IDLE && !f.status().phase_known);
    assert_eq!(f.owner.submit(3, 1), SubmitResult::REJECT_PHASE);
    f.stop();
}

#[test]
fn journal_failed_result_and_reservation_block_reboot() {
    let mut f = boot_with_journal(false);
    f.start_feed();
    f.flash.budget = (UNIT + 40) as i64; // Erase old snapshot, then tear body.
    f.complete();
    assert!(f.status().state == OwnerState::FAULT && f.status().fault == OwnerFault::STORAGE);
    assert!(!f.status().result_durable && f.status().stopped && f.status().optics_off);
    let writes = f.flash.mutations;
    f.advance(2);
    assert!(f.flash.mutations == writes && f.finish_calls == 1);
    f.stop();
    restart_with_journal(&mut f);
    assert!(f.status().state == OwnerState::FAULT && f.status().fault == OwnerFault::RECOVERY);
    assert!(f.status().high_water_id == 1 && !f.status().phase_known);
    assert_ne!(f.owner.submit(2, 1), SubmitResult::ENQUEUED);
    assert_eq!(f.flash.mutations, writes);
    f.stop();

    let mut f = boot_with_journal(false);
    // The final commit reaches media, but its callback reports failure.
    f.flash.budget = (UNIT + RECORD_BYTES) as i64;
    assert_eq!(f.owner.submit(1, 1), SubmitResult::ENQUEUED);
    fake::run_task();
    assert!(f.status().state == OwnerState::FAULT && !f.status().result.motion_possible);
    assert!(f.status().stopped && f.status().optics_off && f.finish_calls == 0);
    f.stop();
    restart_with_journal(&mut f);
    assert!(f.status().state == OwnerState::FAULT && f.status().fault == OwnerFault::RECOVERY);
    assert_eq!(f.status().high_water_id, 1);
    f.stop();
}

#[test]
fn watchdog_requires_owner_progress_and_checks_before_motion() {
    let mut f = boot();
    f.advance(3);
    let refreshes = hw(|h| h.watchdog_refreshes);
    assert!(refreshes > 0 && f.status().watchdog_armed && f.status().watchdog_claimed);
    fake::advance(51 * 32, true); // Timer IRQ executes, owner does not.
    assert_eq!(hw(|h| h.watchdog_refreshes), refreshes);
    fake::run_task();
    assert!(f.status().state == OwnerState::FAULT && f.status().fault == OwnerFault::WATCHDOG);
    assert!(f.status().watchdog_fault == WatchdogFault::PROGRESS && f.status().stopped);
    assert_eq!(hw(|h| h.watchdog_refreshes), refreshes);
    f.stop();
    assert!(!f.status().watchdog_armed && !f.status().watchdog_claimed);

    let mut f = boot();
    f.reserve_delay_ticks = 51 * 32;
    assert_eq!(f.owner.submit(1, 1), SubmitResult::ENQUEUED);
    fake::run_task();
    assert!(f.status().state == OwnerState::FAULT && f.status().fault == OwnerFault::WATCHDOG);
    assert!(
        f.status().result_durable && !f.status().result.motion_possible && f.status().optics_off
    );
    assert!(f.status().result.outcome == OwnerOutcome::FAILED && hw(|h| h.watchdog_refreshes) == 0);
    f.stop();

    let mut f = boot();
    f.owner.watchdog_progress = u64::MAX;
    f.advance(2);
    assert!(f.status().fault == OwnerFault::WATCHDOG && f.status().stopped);
    f.stop();

    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(PHASE_KNOWN));
    f.recover_delay_ticks = 51 * 32;
    f.create();
    fake::run_task();
    assert!(hw(|h| h.watchdog_init_calls) == 1 && hw(|h| h.watchdog_refreshes) == 0);
    assert!(
        f.status().fault == OwnerFault::WATCHDOG && f.status().stopped && f.status().optics_off
    );
    f.stop();
}

#[test]
fn watchdog_masks_stalls_and_late_result_never_reenable() {
    let mut f = boot();
    f.reserve_delay_ticks = 130 * 32;
    f.reserve_mask_irqs = true;
    assert_eq!(f.owner.submit(1, 1), SubmitResult::ENQUEUED);
    fake::run_task();
    assert!(hw(|h| h.watchdog_reset_requests) == 1 && hw(|h| h.watchdog_refreshes) == 0);
    assert!(f.status().state == OwnerState::FAULT && !f.status().result.motion_possible);
    assert!(f.status().stopped && f.status().optics_off);
    f.stop(); // Model records the reset request; a real reset would end execution.

    let mut f = boot();
    f.start_feed();
    f.finish_delay_ticks = 51 * 32;
    f.complete();
    assert!(f.status().state == OwnerState::FAULT && f.status().fault == OwnerFault::WATCHDOG);
    assert!(f.status().stopped && f.status().optics_off && f.status().result_durable);
    assert_eq!(f.status().result.outcome, OwnerOutcome::DONE); // Durable physical result remains valid.
    assert_eq!(hw(|h| h.watchdog_refreshes), f.finish_refreshes);
    assert_ne!(f.owner.submit(2, 1), SubmitResult::ENQUEUED);
    f.stop();
}

#[test]
fn watchdog_changes_stop_motion_and_failed_close_retains_context() {
    let mut f = boot();
    f.start_feed();
    let refreshes = hw(|h| h.watchdog_refreshes);
    hw(|h| h.hs_control &= !sdk::HS_WDTRST_OPT);
    fake::run_task();
    assert!(f.status().state == OwnerState::FAULT && f.status().fault == OwnerFault::WATCHDOG);
    assert!(f.status().watchdog_fault == WatchdogFault::REGISTER && f.status().stopped);
    assert!(
        f.status().result.outcome == OwnerOutcome::FAILED
            && hw(|h| h.watchdog_refreshes) == refreshes
    );
    f.stop();

    let mut f = boot();
    hw(|h| h.watchdog_disable_blocked = true);
    f.owner.stop();
    fake::run_task();
    let s = f.status();
    assert!(s.state == OwnerState::FAULT && s.watchdog_claimed && s.watchdog_armed);
    assert!(s.stopped && s.optics_off && hw(|h| h.task_live) && hw(|h| h.wake_locks) != 0);
    hw(|h| h.watchdog_disable_blocked = false);
    fake::run_task();
    assert!(f.status().state == OwnerState::STOPPED && !f.status().watchdog_claimed);
    assert!(!hw(|h| h.task_live) && hw(|h| h.wake_locks) == 0);
}

#[test]
fn flash_hold_waits_before_outputs_and_release_allows_new_work() {
    let mut f = boot();
    f.advance(2);
    let refreshes = hw(|h| h.watchdog_refreshes);
    f.hold();
    assert_eq!(f.owner.submit(1, 1), SubmitResult::ENQUEUED);
    fake::run_task();
    f.advance(3);
    assert!(f.status().state == OwnerState::PREPARING && f.reserve_calls == 1);
    assert!(f.status().fault == OwnerFault::OK && !f.owner.optics_started);
    assert!(
        f.status().flash_phase == FlashPhase::QUIESCING
            && hw(|h| h.watchdog_refreshes) == refreshes
    );
    inactive_outputs();
    f.release_hold();
    f.advance(5);
    assert!(f.status().state == OwnerState::FEEDING && f.reserve_calls == 1);
    assert!(hw(|h| h.watchdog_refreshes) > refreshes);
    f.stop();
    assert_eq!(f.authority.phase, FlashPhase::IDLE); // Global authority outlives the owner.
}

#[test]
fn flash_interruption_latches_failure_even_after_release() {
    for phase in 0..3 {
        let mut f = boot();
        if phase != 0 {
            f.start_feed();
        } else {
            f.advance(2);
            assert_eq!(f.owner.submit(1, 1), SubmitResult::ENQUEUED);
            fake::run_task();
            assert!(f.owner.optics_started && f.status().state == OwnerState::PREPARING);
        }
        let refreshes = hw(|h| h.watchdog_refreshes);
        f.hold();
        hw(|h| h.blocked_motor_write = phase == 2);
        fake::advance(64, true); // Independent ISR must remove active outputs.
        if phase != 2 {
            inactive_outputs();
        }
        fake::run_task();
        if phase == 2 {
            assert!(
                f.status().fault == OwnerFault::FLASH && f.status().state == OwnerState::REPORTING
            );
            assert!(f.authority.fault == FlashFault::OUTPUT && !f.status().stopped);
            assert!(
                f.finish_calls == 0
                    && hw(|h| h.task_live)
                    && sdk::tim2_en() != 0
                    && hw(|h| h.wake_locks) != 0
            );
            hw(|h| h.blocked_motor_write = false);
            f.advance(1);
            inactive_outputs();
        }
        assert!(f.status().fault == OwnerFault::FLASH && f.status().state == OwnerState::FAULT);
        assert!(f.status().result.outcome == OwnerOutcome::FAILED && f.status().result_durable);
        assert!(f.finish_calls == 1 && hw(|h| h.watchdog_refreshes) == refreshes);
        if phase != 2 {
            f.release_hold();
        }
        f.advance(1);
        assert_eq!(f.owner.submit(2, 1), SubmitResult::REJECT_STOPPED);
        assert!(!f.owner.acknowledge(1));
        assert_eq!(hw(|h| h.watchdog_refreshes), refreshes);
        f.stop();
    }
}

#[test]
fn flash_hold_expires_requests_and_protocol_fault_stops_owner() {
    let mut f = boot();
    f.hold();
    assert_eq!(f.owner.submit(1, 1), SubmitResult::ENQUEUED);
    fake::run_task();
    f.advance(20);
    assert!(
        f.status().state == OwnerState::TERMINAL
            && f.status().result.outcome == OwnerOutcome::EXPIRED
    );
    assert!(!f.status().result.motion_possible && hw(|h| h.watchdog_refreshes) == 0);
    assert!(f.status().result_durable);
    f.authority.trip(FlashFault::TRANSPORT);
    f.advance(1);
    assert!(
        f.status().fault == OwnerFault::FLASH && f.status().flash_fault == FlashFault::TRANSPORT
    );
    assert!(f.status().state == OwnerState::FAULT && hw(|h| h.watchdog_refreshes) == 0);
    inactive_outputs();
    f.stop();

    let mut f = boot();
    f.advance(2);
    let refreshes = hw(|h| h.watchdog_refreshes);
    f.hold();
    f.advance(50); // A stalled flash protocol cannot keep the owner watchdog alive.
    assert!(f.status().fault == OwnerFault::WATCHDOG && hw(|h| h.watchdog_refreshes) == refreshes);
    f.stop();
}

#[test]
fn start_requires_an_external_ready_authority() {
    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(OwnerRecovery::default()));
    let config = f.config;
    assert!(!f.try_start(&config));
    f.bind_authority();
    let limits = FlashLimits {
        quiesce_ms: 100,
        park_ms: 100,
        active_ms: 100,
        release_ms: 100,
        total_ms: 300,
    };
    f.authority = FlashCoord::new(&limits, 17, 0).unwrap();
    let config = f.config;
    assert!(!f.try_start(&config));
    assert_eq!(f.authority.ready(17, 29, true, 0), FlashReply::ACCEPTED);
    f.authority.trip(FlashFault::PEER_LOST);
    assert!(!f.try_start(&config));
    assert!(!hw(|h| h.task_live) && hw(|h| h.watchdog_init_calls) == 0 && hw(|h| h.output[0]) == 0);
}
