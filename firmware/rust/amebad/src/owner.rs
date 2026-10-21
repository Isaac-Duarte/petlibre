//! EXPERIMENTAL FreeRTOS feeding owner: durable reservation, preparation,
//! supervised feeding, result recording and cooperative shutdown.
//!
//! Starting it creates a task that WILL open GPIO/ADC/IRQs when run. The
//! normal diagnostic does not call it. Requires a verified board/recovery,
//! exclusive peripherals/ADC and an unused PMU customer slot (not SDIO 17).

use core::ffi::c_void;
use core::ptr;

use petlibro_core::adc_scale::AdcCalibration;
use petlibro_core::feed_controller::{
    FeedController, FeedLimits, FeedReason, FeedRequest, FeedStartResult, FeedState, MotorCommand,
};
use petlibro_core::feed_progress::{BowlMode, Direction};
use petlibro_core::feed_storage::{OwnerOutcome, OwnerRecovery, OwnerResult, OwnerStorage};
use petlibro_core::feedback_events::CAPACITY;
use petlibro_core::flash_coord::{FlashCoord, FlashFault, FlashLimits, FlashPhase};
use petlibro_core::sensor_inputs::{SensorInputs, SensorLimits, SensorReading};

use crate::active::active_pointer;
use crate::board::{self, Board};
use crate::events::{self, Events, EventsConfig, EventsFault, EventsSnapshot};
use crate::sdk::{self, IrqGuard};
use crate::watchdog::{Watchdog, WatchdogConfig, WatchdogFault};

c_enum!(OwnerState {
    STARTING = 0,
    IDLE = 1,
    QUEUED = 2,
    PREPARING = 3,
    FEEDING = 4,
    REPORTING = 5,
    TERMINAL = 6,
    FAULT = 7,
    STOPPED = 8,
});
c_enum!(OwnerFault { OK = 0, SETUP = 1, RECOVERY = 2, STORAGE = 3, HARDWARE = 4, WATCHDOG = 5, FLASH = 6 });
c_enum!(SubmitResult {
    ENQUEUED = 0,
    REJECT_BUSY = 1,
    REJECT_ID = 2,
    REJECT_REQUEST = 3,
    REJECT_PHASE = 4,
    REJECT_STOPPED = 5,
});

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct OwnerConfig {
    pub feed: FeedLimits,
    pub sensors: SensorLimits,
    pub events: EventsConfig,
    pub watchdog: WatchdogConfig,
    pub bowl_mode: BowlMode,
    pub sample_period_ms: u32,
    pub prepare_timeout_ms: u32,
    pub optics_max_on_ms: u32,
    pub motor_deadtime_ms: u32,
    pub motor_lease_ms: u32,
    pub adc_poll_limit: u32,
    pub wake_lock_id: u32,
    /// Required external authority. Platform boot/transport establishes IDLE
    /// before start. Retain beyond owner STOPPED until global transport teardown.
    pub flash: *mut FlashCoord,
}

impl Default for OwnerConfig {
    fn default() -> Self {
        Self {
            feed: FeedLimits::default(),
            sensors: SensorLimits::default(),
            events: EventsConfig::default(),
            watchdog: WatchdogConfig::default(),
            bowl_mode: BowlMode::default(),
            sample_period_ms: 0,
            prepare_timeout_ms: 0,
            optics_max_on_ms: 0,
            motor_deadtime_ms: 0,
            motor_lease_ms: 0,
            adc_poll_limit: 0,
            wake_lock_id: 0,
            flash: ptr::null_mut(),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OwnerStatus {
    pub state: OwnerState,
    pub fault: OwnerFault,
    pub supervisor_fault: EventsFault,
    pub watchdog_fault: WatchdogFault,
    pub flash_phase: FlashPhase,
    pub flash_fault: FlashFault,
    pub result: OwnerResult,
    pub high_water_id: u64,
    pub phase_known: bool,
    pub intent_reserved: bool,
    pub result_durable: bool,
    pub stopped: bool,
    pub optics_off: bool,
    pub watchdog_claimed: bool,
    pub watchdog_armed: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OwnerMailbox {
    pub id: u64,
    pub portions: u16,
    pub queued_at: u32,
    pub has_request: bool,
    pub cancel: bool,
    pub cancel_id: u64,
    pub acknowledge: bool,
    pub stop: bool,
}

/// Caller-owned fixed-address storage; members private to the implementation.
/// Use status/command methods instead of reading/mutating fields concurrently.
#[repr(C)]
#[derive(Debug, Default)]
pub struct Owner {
    pub config: OwnerConfig,
    pub storage: OwnerStorage,
    pub board: Board,
    pub events: Events,
    pub watchdog: Watchdog,
    pub sensors: SensorInputs,
    pub controller: FeedController,
    pub recovery: OwnerRecovery,
    pub request: FeedRequest,
    pub result: OwnerResult,
    pub published: OwnerStatus,
    pub mailbox: OwnerMailbox,
    pub state: OwnerState,
    pub fault: OwnerFault,
    pub last_submitted_id: u64,
    pub watchdog_progress: u64,
    pub queued_at: u32,
    pub initialized: bool,
    pub board_attempted: bool,
    pub wake_owned: bool,
    pub reserved: bool,
    pub finish_attempted: bool,
    pub optics_started: bool,
    pub result_durable: bool,
    /// Initialization failed after the watchdog was armed (SETUP or RECOVERY).
    /// The fault stays latched; the watchdog is refreshed only while each
    /// iteration re-verifies STOP and infrared OFF, so a determined failure
    /// does not reset-loop while a hang or output loss still resets.
    pub init_failed: bool,
}

active_pointer!(Owner);

fn interval(value: u32) -> bool {
    value > 0 && value < 0x8000_0000
}

impl OwnerConfig {
    pub fn valid(&self) -> bool {
        let c = self;
        if !c.watchdog.valid()
            || c.watchdog.systimer_hz != c.events.ticks_per_second
            || c.watchdog.max_progress_gap_ms <= c.sample_period_ms
            || !interval(c.sample_period_ms)
            || !interval(c.prepare_timeout_ms)
            || !interval(c.optics_max_on_ms)
            || c.motor_deadtime_ms == 0
            || !interval(c.motor_lease_ms)
            || c.motor_lease_ms <= c.motor_deadtime_ms
            || c.sample_period_ms >= c.motor_lease_ms / 3
            || c.sample_period_ms > c.sensors.max_age_ms
            || c.sample_period_ms > c.sensors.max_observation_gap_ms
            || c.sample_period_ms > c.feed.max_observation_gap_ms
            || c.sample_period_ms > c.events.max_feedback_age_ms
            || c.events.max_feedback_age_ms >= c.feed.feedback_inhibit_ms
            || c.sensors.optical_settle_ms >= c.prepare_timeout_ms
            || c.sensors.power_settle_ms >= c.prepare_timeout_ms
            || u64::from(c.prepare_timeout_ms) + u64::from(c.feed.max_duration_ms)
                > u64::from(c.optics_max_on_ms)
            || c.events.ticks_per_second < 1000
            || c.events.ticks_per_second > 1_000_000
            || c.events.timer_period_ms == 0
            || c.events.timer_period_ms >= c.motor_lease_ms / 3
            || c.events.feedback_capacity == 0
            || usize::from(c.events.feedback_capacity) > CAPACITY
            || c.adc_poll_limit == 0
            || c.adc_poll_limit > 1_000_000
            || c.wake_lock_id < sdk::PMU_USER_BASE
            || c.wake_lock_id > sdk::PMU_MAX
            || c.wake_lock_id == 17
            || !c.bowl_mode.valid()
        {
            return false;
        }
        // Only validate structure/ranges here; real efuse calibration is read by open.
        let calibration = AdcCalibration {
            offset: 2480,
            gain: 12050,
            ..AdcCalibration::default()
        };
        events::period_ticks(&c.events).is_some()
            && FeedController::new(&c.feed, 0).is_some()
            && SensorInputs::new(&c.sensors, &calibration).is_some()
    }

    /// Whether a whole flash lease fits inside watchdog supervision. During a
    /// lease the owner cannot checkpoint, so the gap between two checkpoints
    /// spans up to one sample period before the lease, the entire lease
    /// budget, one sample period after it, and two timer periods of
    /// scheduling/timer granularity before the first post-lease checkpoint
    /// (it also needs a fresh timer interrupt). That sum must stay below
    /// `max_progress_gap_ms`. The hardware counter, whose shortest expiry
    /// `WatchdogConfig::valid` already requires to exceed twice the progress
    /// gap, then also covers an IRQ-masked lease plus the rate-limited refresh
    /// offset (`min_refresh_ms`, below the gap).
    pub fn lease_fits(&self, limits: &FlashLimits) -> bool {
        let needed = u64::from(limits.total_ms)
            + 2 * u64::from(self.sample_period_ms)
            + 2 * u64::from(self.events.timer_period_ms);
        limits.total_ms != 0 && needed < u64::from(self.watchdog.max_progress_gap_ms)
    }
}

impl Owner {
    fn flash(&self) -> &FlashCoord {
        // SAFETY: `start` requires a live authority retained beyond STOPPED.
        unsafe { &*self.config.flash }
    }

    fn snapshot(&mut self, s: &mut EventsSnapshot) -> bool {
        *s = EventsSnapshot::default();
        if self.events.opened {
            self.events.snapshot(s);
            return s.healthy;
        }
        s.stopped = self.board.motor.write_pair.is_some()
            && self.board.motor.applied_known
            && self.board.motor.applied == MotorCommand::STOP;
        s.optics_off = self.board.optics_initialized && board::optics_off();
        false
    }

    fn publish(&mut self) {
        let mut s = EventsSnapshot::default();
        self.snapshot(&mut s);
        let mut value = OwnerStatus {
            state: self.state,
            fault: self.fault,
            supervisor_fault: s.fault,
            watchdog_fault: self.watchdog.fault,
            watchdog_claimed: self.watchdog.claimed,
            watchdog_armed: self.watchdog.armed,
            result: self.result,
            high_water_id: self.recovery.high_water_id,
            phase_known: self.recovery.phase_known,
            intent_reserved: self.reserved,
            result_durable: self.result_durable,
            stopped: s.stopped,
            optics_off: s.optics_off,
            ..OwnerStatus::default()
        };
        let _guard = IrqGuard::lock();
        value.flash_phase = self.flash().phase;
        value.flash_fault = self.flash().fault;
        self.published = value;
    }

    fn fail(&mut self, reason: OwnerFault) {
        if self.fault == OwnerFault::OK {
            self.fault = reason;
        }
    }

    fn stop_requested(&self) -> bool {
        let _guard = IrqGuard::lock();
        self.mailbox.stop
    }

    fn cancelled(&self) -> bool {
        let _guard = IrqGuard::lock();
        self.mailbox.stop || (self.mailbox.cancel && self.mailbox.cancel_id == self.request.id)
    }

    fn quiet(&mut self) -> bool {
        if self.events.opened {
            self.events.motor(MotorCommand::STOP);
            self.events.optics(false, 0);
        } else if self.board.motor.write_pair.is_some() {
            let observed = self.board.motor.observed_at;
            self.board.trip(observed);
            self.board.optics(false);
        }
        let mut s = EventsSnapshot::default();
        self.snapshot(&mut s);
        if !s.stopped || !s.optics_off {
            return false;
        }
        if self.events.opened {
            self.events.capture(false);
        }
        true
    }

    fn reporting(&mut self, outcome: OwnerOutcome, reason: FeedReason) {
        self.result.outcome = outcome;
        self.result.reason = reason;
        self.result.completed = if self.controller.last_request_id == self.request.id {
            self.controller.progress.completed
        } else {
            0
        };
        if self.result.motion_possible
            && outcome != OwnerOutcome::DONE
            && self.config.bowl_mode == BowlMode::DOUBLE
        {
            self.recovery.phase_known = false;
        }
        self.state = OwnerState::REPORTING;
    }

    /// Fail an active reservation as an output failure, or fault the owner.
    fn abort(&mut self) {
        if self.reserved
            && (self.state == OwnerState::PREPARING || self.state == OwnerState::FEEDING)
        {
            self.controller.trip(FeedReason::OUTPUT);
            self.reporting(OwnerOutcome::FAILED, FeedReason::OUTPUT);
        } else if self.state != OwnerState::REPORTING {
            self.state = OwnerState::FAULT;
        }
    }

    fn check_watchdog(&mut self) -> bool {
        if self.watchdog.check() {
            return true;
        }
        self.fail(OwnerFault::WATCHDOG);
        self.abort();
        self.quiet();
        false
    }

    /// Authority time is owned by the global transport, not the event-clock
    /// epoch. This interlock neither advances its clock nor acknowledges RAM parking.
    fn flash_ready(&mut self) -> bool {
        let _guard = IrqGuard::lock();
        let c = self.flash();
        let blocked = c.outputs_blocked();
        let mut broken = !c.initialized
            || c.fault != FlashFault::OK
            || c.phase == FlashPhase::BOOT
            || c.phase == FlashPhase::CLOSING
            || c.phase == FlashPhase::CLOSED;
        let interrupted = blocked
            && (self.state == OwnerState::FEEDING
                || (self.state == OwnerState::PREPARING && self.optics_started)
                || self.board.optics_enabled
                || self.board.motor.desired != MotorCommand::STOP);
        if blocked && self.board_attempted && !self.board.flash_quiesce() {
            // SAFETY: as for `flash`; serialized by the guard.
            unsafe { (*self.config.flash).trip(FlashFault::OUTPUT) };
            broken = true;
        }
        if broken || interrupted {
            self.fail(OwnerFault::FLASH);
            self.abort();
        }
        !blocked
    }

    fn finish_result(&mut self) {
        if !self.quiet() {
            self.fail(OwnerFault::HARDWARE);
            return; // Keep the supervisor and caller-owned context alive for retries.
        }
        let mut s = EventsSnapshot::default();
        if !self.snapshot(&mut s) {
            self.fail(OwnerFault::HARDWARE);
        }
        if self.fault != OwnerFault::OK && self.result.outcome == OwnerOutcome::DONE {
            self.reporting(OwnerOutcome::FAILED, FeedReason::OUTPUT);
        }
        if !self.reserved || self.finish_attempted {
            self.state = OwnerState::FAULT;
            return;
        }
        let mut next = self.recovery;
        next.pending_request = false;
        if self.result.outcome == OwnerOutcome::DONE {
            next.direction = self.controller.progress.direction;
            next.credit_next_event = self.controller.progress.credit_next_event;
        }
        self.finish_attempted = true;
        self.publish(); // REPORTING is visible before a potentially slow journal write.
        // SAFETY: `start` validated the storage callbacks for their context.
        if unsafe { self.storage.finish(&self.result, &next) } {
            self.recovery = next;
            self.result_durable = true;
            self.state = if self.fault == OwnerFault::OK {
                OwnerState::TERMINAL
            } else {
                OwnerState::FAULT
            };
        } else {
            self.fail(OwnerFault::STORAGE);
            self.state = OwnerState::FAULT;
        }
    }

    fn initialize(&mut self) -> bool {
        self.initialized = true;
        sdk::wakelock_acquire(self.config.wake_lock_id);
        self.wake_owned = true;
        // Covers blocked board initialization/recovery, before any later refresh.
        let watchdog_config = self.config.watchdog;
        if !self.watchdog.open(&watchdog_config) {
            self.fail(OwnerFault::SETUP);
            return false;
        }
        self.board_attempted = true;
        let c = self.config;
        let board = &raw mut self.board;
        // SAFETY: the owner, its board and the authority stay at fixed addresses.
        let opened = self
            .board
            .open(c.motor_deadtime_ms, c.motor_lease_ms, c.adc_poll_limit, 0)
            && unsafe { self.board.attach_flash(c.flash) }
            && unsafe { self.events.open(board, &c.events) };
        if !opened {
            self.fail(OwnerFault::SETUP);
            self.init_failed = true;
            return false;
        }
        // SAFETY: as for `finish_result`.
        let recovered = unsafe { self.storage.recover_into(&mut self.recovery) };
        let r = self.recovery;
        if !recovered
            || !r.direction.valid()
            || (c.bowl_mode == BowlMode::SINGLE && r.direction != Direction::D0)
            || r.pending_request
        {
            self.fail(OwnerFault::RECOVERY);
            self.recovery.phase_known = false;
            self.init_failed = true;
            return false;
        }
        let controller = FeedController::new(&c.feed, self.recovery.high_water_id);
        let sensors = SensorInputs::new(&c.sensors, &self.board.calibration);
        let (Some(controller), Some(sensors)) = (controller, sensors) else {
            self.fail(OwnerFault::SETUP);
            self.init_failed = true;
            return false;
        };
        self.controller = controller;
        self.sensors = sensors;
        if c.bowl_mode == BowlMode::SINGLE {
            self.recovery.phase_known = true;
        }
        self.last_submitted_id = self.recovery.high_water_id;
        self.state = OwnerState::IDLE;
        true
    }

    fn take_request(&mut self) {
        {
            let _guard = IrqGuard::lock();
            if self.state != OwnerState::IDLE || !self.mailbox.has_request {
                return;
            }
            self.request = FeedRequest {
                id: self.mailbox.id,
                portions: self.mailbox.portions,
                mode: self.config.bowl_mode,
                direction: self.recovery.direction,
                credit_next_event: self.recovery.credit_next_event,
                phase_known: self.recovery.phase_known,
            };
            self.queued_at = self.mailbox.queued_at;
            self.mailbox.has_request = false;
            self.reserved = false;
            self.finish_attempted = false;
            self.result_durable = false;
            self.optics_started = false;
            self.result = OwnerResult {
                id: self.request.id,
                requested: self.request.portions,
                ..OwnerResult::default()
            };
            self.state = OwnerState::PREPARING;
            self.published.state = self.state;
            self.published.result = self.result;
            self.published.result_durable = false;
            self.published.intent_reserved = false;
        }
        self.publish();
        // Reserve even a queued cancellation/expiry: it becomes a durable outcome.
        if !self.quiet() {
            self.fail(OwnerFault::HARDWARE);
            self.state = OwnerState::FAULT;
        // SAFETY: as for `finish_result`.
        } else if !unsafe { self.storage.reserve(&self.request) } {
            self.fail(OwnerFault::STORAGE);
            self.state = OwnerState::FAULT;
        } else {
            self.reserved = true;
            self.recovery.high_water_id = self.request.id;
            self.recovery.pending_request = true;
        }
    }

    fn acquire(&mut self) -> bool {
        let Some(started) = self.events.now_ms() else {
            return false;
        };
        let mut raw = SensorReading::default();
        let valid = self.board.capture(&mut raw);
        self.sensors.update(started, valid.then_some(&raw));
        // Cancellation has priority over feedback captured during acquisition.
        if (self.state == OwnerState::FEEDING || self.state == OwnerState::PREPARING)
            && self.cancelled()
        {
            self.controller.cancel(self.request.id);
            self.reporting(OwnerOutcome::CANCELLED, FeedReason::CANCELLED);
            return valid;
        }
        valid && self.events.observe(&mut self.controller, &self.sensors)
    }

    fn prepare(&mut self) {
        if self.cancelled() {
            self.reporting(OwnerOutcome::CANCELLED, FeedReason::CANCELLED);
            return;
        }
        let mut s = EventsSnapshot::default();
        if !self.snapshot(&mut s) {
            self.fail(OwnerFault::HARDWARE);
            self.reporting(OwnerOutcome::FAILED, FeedReason::OUTPUT);
            return;
        }
        if s.now_ms.wrapping_sub(self.queued_at) >= self.config.prepare_timeout_ms {
            self.reporting(OwnerOutcome::EXPIRED, FeedReason::TOTAL_TIMEOUT);
            return;
        }
        if !self.flash_ready() || !s.timer_ready || !s.owner_ready {
            return;
        }
        if !self.optics_started {
            if !self.events.optics(true, self.config.optics_max_on_ms) {
                self.fail(OwnerFault::HARDWARE);
                self.reporting(OwnerOutcome::FAILED, FeedReason::OUTPUT);
            } else {
                self.optics_started = true;
            }
            return; // Fresh observations must now establish optical settling.
        }
        let _guard = IrqGuard::lock();
        if self.cancelled() {
            self.reporting(OwnerOutcome::CANCELLED, FeedReason::CANCELLED);
        } else if self.flash_ready() {
            let latest = self.queued_at.wrapping_add(self.config.prepare_timeout_ms);
            let result =
                self.events
                    .start(&mut self.controller, &self.sensors, &self.request, latest);
            self.result.motion_possible = self.controller.last_request_id == self.request.id;
            if result == FeedStartResult::ACCEPTED {
                self.state = OwnerState::FEEDING;
            } else if self.result.motion_possible {
                self.fail(OwnerFault::HARDWARE);
                let reason = self.controller.reason;
                self.reporting(OwnerOutcome::FAILED, reason);
            }
        }
    }

    fn shutdown(&mut self) -> bool {
        if self.board_attempted && !self.quiet() {
            return false;
        }
        if self.events.opened && !self.events.close() {
            return false;
        }
        if self.board.opened {
            self.board.close();
        }
        if !self.watchdog.close(Some(&self.board)) {
            self.fail(OwnerFault::WATCHDOG);
            self.state = OwnerState::FAULT;
            return false;
        }
        if self.wake_owned {
            sdk::wakelock_release(self.config.wake_lock_id);
            self.wake_owned = false;
        }
        self.state = OwnerState::STOPPED;
        true
    }

    /// Terminal stopped fault after a determined initialization failure: no
    /// request is ever taken, and the watchdog is refreshed only after `quiet`
    /// verified STOP and infrared OFF in this same iteration. The progress
    /// deadline still applies, so a hang before or during this state resets.
    fn hold_init_fault(&mut self) {
        let mut s = EventsSnapshot::default();
        self.snapshot(&mut s);
        // Gate state alone is not enough here: also read back the pins.
        if !s.stopped
            || !s.optics_off
            || !self.board.inactive()
            || self.watchdog.fault != WatchdogFault::OK
        {
            return;
        }
        let progress = if self.watchdog_progress == u64::MAX {
            0
        } else {
            self.watchdog_progress += 1;
            self.watchdog_progress
        };
        // SAFETY: as for `flash`; a blocked authority refreshes nothing.
        unsafe {
            self.watchdog
                .checkpoint_if_ready(progress, self.config.flash)
        };
    }

    fn active_phase(&self) -> bool {
        self.state == OwnerState::PREPARING || self.state == OwnerState::FEEDING
    }

    /// One owner iteration; true once the owner has stopped.
    ///
    /// Takes a raw pointer so that no single `&mut Owner` spans the
    /// interrupt-enabled windows in which other tasks post to the mailbox and
    /// ISRs update the board; each step borrows the owner afresh.
    ///
    /// # Safety
    /// `this` is the started, fixed-address owner, polled only by its task.
    unsafe fn poll(this: *mut Self) -> bool {
        // SAFETY (whole body): see the function contract.
        unsafe {
            if !(*this).initialized && (*this).stop_requested() {
                (*this).state = OwnerState::STOPPED;
                (*this).publish(); // No hardware or wake lock has been claimed.
                return true;
            }
            if !(*this).initialized && !(*this).initialize() {
                (*this).state = OwnerState::FAULT;
            }
            if (*this).fault == OwnerFault::OK {
                (*this).check_watchdog();
            }
            if (*this).fault == OwnerFault::OK {
                (*this).flash_ready();
            }
            let ack = {
                let _guard = IrqGuard::lock();
                core::mem::take(&mut (*this).mailbox.acknowledge)
            };
            if ack && (*this).state == OwnerState::TERMINAL && (*this).result_durable {
                (*this).controller.reset();
                (*this).state = OwnerState::IDLE;
            }
            (*this).take_request();
            if (*this).fault == OwnerFault::OK {
                (*this).check_watchdog(); // A reservation can block.
            }
            if (*this).fault == OwnerFault::OK {
                (*this).flash_ready();
            }
            if (*this).active_phase() && (*this).cancelled() {
                (*this).controller.cancel((*this).request.id);
                (*this).reporting(OwnerOutcome::CANCELLED, FeedReason::CANCELLED);
            }
            let mut acquired = false;
            if !(*this).flash().outputs_blocked()
                && ((*this).state == OwnerState::IDLE || (*this).active_phase())
            {
                acquired = (*this).acquire();
                if !acquired {
                    (*this).fail(OwnerFault::HARDWARE);
                    if (*this).reserved
                        && ((*this).active_phase() || (*this).state == OwnerState::REPORTING)
                    {
                        (*this).controller.trip(FeedReason::OUTPUT);
                        let reason = (*this).controller.reason;
                        (*this).reporting(OwnerOutcome::FAILED, reason);
                    } else {
                        (*this).state = OwnerState::FAULT;
                    }
                }
            }
            if (*this).fault == OwnerFault::OK {
                (*this).check_watchdog(); // Before optical/motor enable.
            }
            if (*this).fault == OwnerFault::OK {
                (*this).flash_ready();
            }
            if (*this).state == OwnerState::PREPARING {
                (*this).prepare();
            }
            if (*this).state == OwnerState::FEEDING {
                if !(*this).events.motor((*this).controller.motor) {
                    (*this).fail(OwnerFault::HARDWARE);
                    (*this).controller.trip(FeedReason::OUTPUT);
                }
                let reason = (*this).controller.reason;
                match (*this).controller.state {
                    FeedState::COMPLETE => (*this).reporting(OwnerOutcome::DONE, reason),
                    FeedState::CANCELLED => (*this).reporting(OwnerOutcome::CANCELLED, reason),
                    FeedState::FAULT => (*this).reporting(OwnerOutcome::FAILED, reason),
                    _ => {}
                }
            }
            if (*this).state == OwnerState::REPORTING {
                (*this).finish_result();
            }
            if (*this).fault == OwnerFault::OK
                && !(*this).flash().outputs_blocked()
                && ((*this).state == OwnerState::IDLE
                    || (*this).active_phase()
                    || (*this).state == OwnerState::TERMINAL)
            {
                let mut s = EventsSnapshot::default();
                if (*this).snapshot(&mut s)
                    && s.timer_ready
                    && (((*this).state == OwnerState::TERMINAL && s.stopped && s.optics_off)
                        || (acquired && s.owner_ready))
                {
                    let progress = if (*this).watchdog_progress == u64::MAX {
                        0
                    } else {
                        (*this).watchdog_progress += 1;
                        (*this).watchdog_progress
                    };
                    let ok = (*this)
                        .watchdog
                        .checkpoint_if_ready(progress, (*this).config.flash);
                    if !ok {
                        (*this).check_watchdog();
                    }
                }
            }
            if (*this).state == OwnerState::FAULT {
                let quiet = (*this).quiet();
                if quiet && (*this).init_failed {
                    (*this).hold_init_fault();
                }
            }
            let ended = (*this).stop_requested()
                && (*this).state != OwnerState::REPORTING
                && !(*this).active_phase()
                && (*this).shutdown();
            (*this).publish();
            ended
        }
    }

    /// Starts the experimental owner task. Retain this object until status
    /// STOPPED; stop is cooperative, never forced.
    ///
    /// # Safety
    /// `config.flash` must be a live authority, and `storage` callbacks sound
    /// for their context, for as long as the owner runs and beyond STOPPED
    /// until global transport teardown. `self` must not move until STOPPED.
    pub unsafe fn start(&mut self, config: &OwnerConfig, storage: &OwnerStorage) -> bool {
        if !config.valid() || !storage.complete() {
            return false;
        }
        {
            let _guard = IrqGuard::lock();
            // SAFETY: the contract requires a live authority; null fails here.
            let fits =
                unsafe { config.flash.as_ref() }.is_some_and(|f| config.lease_fits(&f.limits));
            if !active().is_null() || board::outputs_blocked(config.flash) || !fits {
                return false;
            }
            *self = Self {
                config: *config,
                storage: *storage,
                state: OwnerState::STARTING,
                published: OwnerStatus {
                    state: OwnerState::STARTING,
                    ..OwnerStatus::default()
                },
                ..Self::default()
            };
            set_active(self);
        }
        if !sdk::task_create(
            c"feed-owner",
            2048,
            3,
            owner_step,
            ptr::from_mut(self).cast(),
        ) {
            let _guard = IrqGuard::lock();
            set_active(ptr::null_mut());
            self.state = OwnerState::STOPPED;
            self.published.state = OwnerState::STOPPED;
            self.published.fault = OwnerFault::SETUP;
            return false;
        }
        true
    }

    fn running(&self) -> bool {
        active().cast_const() == ptr::from_ref(self)
    }

    /// ENQUEUED is transport acceptance, not durable acceptance or dispensing.
    pub fn submit(&mut self, id: u64, portions: u16) -> SubmitResult {
        let _guard = IrqGuard::lock();
        if !self.running() || self.mailbox.stop || self.published.state == OwnerState::FAULT {
            return SubmitResult::REJECT_STOPPED;
        }
        if self.published.state != OwnerState::IDLE || self.mailbox.has_request {
            return SubmitResult::REJECT_BUSY;
        }
        if id == 0 || id <= self.last_submitted_id {
            return SubmitResult::REJECT_ID;
        }
        if portions == 0 || portions > self.config.feed.max_portions {
            return SubmitResult::REJECT_REQUEST;
        }
        if self.config.bowl_mode == BowlMode::DOUBLE && !self.published.phase_known {
            return SubmitResult::REJECT_PHASE;
        }
        let Some(value) = self.events.now_ms() else {
            return SubmitResult::REJECT_STOPPED;
        };
        self.mailbox.id = id;
        self.mailbox.portions = portions;
        self.mailbox.queued_at = value;
        self.mailbox.has_request = true;
        self.mailbox.cancel = false;
        self.last_submitted_id = id;
        SubmitResult::ENQUEUED
    }

    pub fn cancel(&mut self, id: u64) -> bool {
        if id == 0 {
            return false;
        }
        let _guard = IrqGuard::lock();
        let result = self.running()
            && ((self.mailbox.has_request && self.mailbox.id == id)
                || ((self.published.state == OwnerState::PREPARING
                    || self.published.state == OwnerState::FEEDING)
                    && self.published.result.id == id));
        if result {
            self.mailbox.cancel = true;
            self.mailbox.cancel_id = id;
        }
        result
    }

    pub fn acknowledge(&mut self, id: u64) -> bool {
        if id == 0 {
            return false;
        }
        let _guard = IrqGuard::lock();
        let result = self.running()
            && self.published.state == OwnerState::TERMINAL
            && self.published.result.id == id
            && self.published.result_durable;
        if result {
            self.mailbox.acknowledge = true;
        }
        result
    }

    pub fn stop(&mut self) {
        let _guard = IrqGuard::lock();
        if self.running() {
            self.mailbox.stop = true;
        }
    }

    pub fn status(&self) -> OwnerStatus {
        let _guard = IrqGuard::lock();
        let mut status = self.published;
        if self.mailbox.has_request && status.state == OwnerState::IDLE {
            status.state = OwnerState::QUEUED;
            status.result = OwnerResult {
                id: self.mailbox.id,
                requested: self.mailbox.portions,
                ..OwnerResult::default()
            };
            status.intent_reserved = false;
            status.result_durable = false;
        }
        status
    }
}

unsafe extern "C" fn owner_step(argument: *mut c_void) -> u32 {
    // SAFETY: `start` registered this fixed-address owner as the task context.
    let owner = argument.cast::<Owner>();
    if !unsafe { Owner::poll(owner) } {
        return unsafe { (*owner).config.sample_period_ms };
    }
    let _guard = IrqGuard::lock();
    set_active(ptr::null_mut());
    0
}
