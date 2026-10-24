//! The owner task's lifecycle state machine: initialization, request
//! reservation, preparation, feeding, result recording and shutdown.

use wellfed_core::feed::controller::{
    FeedController, FeedReason, FeedRequest, FeedStartResult, FeedState, MotorCommand,
};
use wellfed_core::feed::progress::{BowlMode, Direction};
use wellfed_core::feed::sensor_inputs::{SensorInputs, SensorReading};
use wellfed_core::feed::storage::{OwnerOutcome, OwnerResult};
use wellfed_core::flash_coord::{FlashCoord, FlashFault, FlashPhase};

use crate::board;
use crate::sdk::{self, IrqGuard};
use crate::supervision::events::EventsSnapshot;
use crate::supervision::watchdog::WatchdogFault;

use super::{OwnerFault, OwnerState, OwnerStatus, OwnerTask, Shared};

impl OwnerTask {
    /// The shared half, inside `IrqGuard`; the borrow ends with `f`, which
    /// must not call back in.
    fn shared<R>(&self, f: impl FnOnce(&mut Shared) -> R) -> R {
        let _guard = IrqGuard::lock();
        // SAFETY: `start` pointed this at the owner's shared half, which lives
        // as long as the owner; inside the guard this is its only borrow.
        f(unsafe { &mut *self.shared })
    }

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
            && self.board.motor.applied == MotorCommand::Stop;
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
        self.shared(|shared| shared.published = value);
    }

    fn fail(&mut self, reason: OwnerFault) {
        if self.fault == OwnerFault::Ok {
            self.fault = reason;
        }
    }

    fn stop_requested(&self) -> bool {
        self.shared(|shared| shared.mailbox.stop)
    }

    fn cancelled(&self) -> bool {
        let id = self.request.id;
        self.shared(|shared| {
            let m = &shared.mailbox;
            m.stop || (m.cancel && m.cancel_id == id)
        })
    }

    fn quiet(&mut self) -> bool {
        if self.events.opened {
            // Best effort: the outputs are verified stopped below.
            let _ = self.events.motor(MotorCommand::Stop);
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
            && outcome != OwnerOutcome::Done
            && self.config.bowl_mode == BowlMode::Double
        {
            self.recovery.phase_known = false;
        }
        self.state = OwnerState::Reporting;
    }

    /// Fail an active reservation as an output failure, or fault the owner.
    fn abort(&mut self) {
        if self.reserved
            && (self.state == OwnerState::Preparing || self.state == OwnerState::Feeding)
        {
            self.controller.trip(FeedReason::Output);
            self.reporting(OwnerOutcome::Failed, FeedReason::Output);
        } else if self.state != OwnerState::Reporting {
            self.state = OwnerState::Fault;
        }
    }

    fn check_watchdog(&mut self) -> bool {
        if self.watchdog.check() {
            return true;
        }
        self.fail(OwnerFault::Watchdog);
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
            || c.fault != FlashFault::Ok
            || c.phase == FlashPhase::Boot
            || c.phase == FlashPhase::Closing
            || c.phase == FlashPhase::Closed;
        let interrupted = blocked
            && (self.state == OwnerState::Feeding
                || (self.state == OwnerState::Preparing && self.optics_started)
                || self.board.optics_enabled
                || self.board.motor.desired != MotorCommand::Stop);

        if blocked && self.board_attempted && !self.board.flash_quiesce() {
            // SAFETY: as for `flash`; serialized by the guard.
            unsafe { (*self.config.flash).trip(FlashFault::Output) };
            broken = true;
        }
        if broken || interrupted {
            self.fail(OwnerFault::Flash);
            self.abort();
        }
        !blocked
    }

    fn finish_result(&mut self) {
        if !self.quiet() {
            self.fail(OwnerFault::Hardware);
            return; // Keep the supervisor and caller-owned context alive for retries.
        }

        let mut s = EventsSnapshot::default();
        if !self.snapshot(&mut s) {
            self.fail(OwnerFault::Hardware);
        }
        if self.fault != OwnerFault::Ok && self.result.outcome == OwnerOutcome::Done {
            self.reporting(OwnerOutcome::Failed, FeedReason::Output);
        }
        if !self.reserved || self.finish_attempted {
            self.state = OwnerState::Fault;
            return;
        }

        let mut next = self.recovery;
        next.pending_request = false;
        if self.result.outcome == OwnerOutcome::Done {
            next.direction = self.controller.progress.direction;
            next.credit_next_event = self.controller.progress.credit_next_event;
        }

        self.finish_attempted = true;
        self.publish(); // REPORTING is visible before a potentially slow journal write.
        // SAFETY: `start` validated the storage callbacks for their context.
        if unsafe { self.storage.finish(&self.result, &next) } {
            self.recovery = next;
            self.result_durable = true;
            self.state = if self.fault == OwnerFault::Ok {
                OwnerState::Terminal
            } else {
                OwnerState::Fault
            };
        } else {
            self.fail(OwnerFault::Storage);
            self.state = OwnerState::Fault;
        }
    }

    fn initialize(&mut self) -> bool {
        self.initialized = true;
        sdk::wakelock_acquire(self.config.wake_lock_id);
        self.wake_owned = true;

        // Covers blocked board initialization/recovery, before any later refresh.
        let watchdog_config = self.config.watchdog;
        if !self.watchdog.open(&watchdog_config) {
            self.fail(OwnerFault::Setup);
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
            self.fail(OwnerFault::Setup);
            self.init_failed = true;
            return false;
        }

        // SAFETY: as for `finish_result`.
        let recovered = unsafe { self.storage.recover_into(&mut self.recovery) };
        let r = self.recovery;
        if !recovered
            || (c.bowl_mode == BowlMode::Single && r.direction != Direction::D0)
            || r.pending_request
        {
            self.fail(OwnerFault::Recovery);
            self.recovery.phase_known = false;
            self.init_failed = true;
            return false;
        }

        let controller = FeedController::new(&c.feed, self.recovery.high_water_id);
        let sensors = SensorInputs::new(&c.sensors, &self.board.calibration);
        let (Some(controller), Some(sensors)) = (controller, sensors) else {
            self.fail(OwnerFault::Setup);
            self.init_failed = true;
            return false;
        };
        self.controller = controller;
        self.sensors = sensors;
        if c.bowl_mode == BowlMode::Single {
            self.recovery.phase_known = true;
        }

        let high_water = self.recovery.high_water_id;
        self.shared(|shared| shared.last_submitted_id = high_water);
        self.state = OwnerState::Idle;
        true
    }

    fn take_request(&mut self) {
        {
            let _guard = IrqGuard::lock();
            if self.state != OwnerState::Idle {
                return;
            }
            let Some(mailbox) = self.shared(|shared| {
                let mailbox = shared.mailbox;
                shared.mailbox.has_request = false;
                mailbox.has_request.then_some(mailbox)
            }) else {
                return;
            };

            self.request = FeedRequest {
                id: mailbox.id,
                portions: mailbox.portions,
                mode: self.config.bowl_mode,
                direction: self.recovery.direction,
                credit_next_event: self.recovery.credit_next_event,
                phase_known: self.recovery.phase_known,
            };
            self.queued_at = mailbox.queued_at;

            self.reserved = false;
            self.finish_attempted = false;
            self.result_durable = false;
            self.optics_started = false;
            self.result = OwnerResult {
                id: self.request.id,
                requested: self.request.portions,
                ..OwnerResult::default()
            };
            self.state = OwnerState::Preparing;
            let (state, result) = (self.state, self.result);
            self.shared(|shared| {
                shared.published.state = state;
                shared.published.result = result;
                shared.published.result_durable = false;
                shared.published.intent_reserved = false;
            });
        }

        self.publish();

        // Reserve even a queued cancellation/expiry: it becomes a durable outcome.
        if !self.quiet() {
            self.fail(OwnerFault::Hardware);
            self.state = OwnerState::Fault;
        // SAFETY: as for `finish_result`.
        } else if !unsafe { self.storage.reserve(&self.request) } {
            self.fail(OwnerFault::Storage);
            self.state = OwnerState::Fault;
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
        if (self.state == OwnerState::Feeding || self.state == OwnerState::Preparing)
            && self.cancelled()
        {
            self.controller.cancel(self.request.id);
            self.reporting(OwnerOutcome::Cancelled, FeedReason::Cancelled);
            return valid;
        }
        valid && self.events.observe(&mut self.controller, &self.sensors)
    }

    fn prepare(&mut self) {
        if self.cancelled() {
            self.reporting(OwnerOutcome::Cancelled, FeedReason::Cancelled);
            return;
        }

        let mut s = EventsSnapshot::default();
        if !self.snapshot(&mut s) {
            self.fail(OwnerFault::Hardware);
            self.reporting(OwnerOutcome::Failed, FeedReason::Output);
            return;
        }

        if s.now_ms.wrapping_sub(self.queued_at) >= self.config.prepare_timeout_ms {
            self.reporting(OwnerOutcome::Expired, FeedReason::TotalTimeout);
            return;
        }

        if !self.flash_ready() || !s.timer_ready || !s.owner_ready {
            return;
        }

        if !self.optics_started {
            if !self.events.optics(true, self.config.optics_max_on_ms) {
                self.fail(OwnerFault::Hardware);
                self.reporting(OwnerOutcome::Failed, FeedReason::Output);
            } else {
                self.optics_started = true;
            }
            return; // Fresh observations must now establish optical settling.
        }

        let _guard = IrqGuard::lock();
        if self.cancelled() {
            self.reporting(OwnerOutcome::Cancelled, FeedReason::Cancelled);
        } else if self.flash_ready() {
            let latest = self.queued_at.wrapping_add(self.config.prepare_timeout_ms);
            let result =
                self.events
                    .start(&mut self.controller, &self.sensors, &self.request, latest);
            self.result.motion_possible = self.controller.last_request_id == self.request.id;
            if result == FeedStartResult::Accepted {
                self.state = OwnerState::Feeding;
            } else if self.result.motion_possible {
                self.fail(OwnerFault::Hardware);
                let reason = self.controller.reason;
                self.reporting(OwnerOutcome::Failed, reason);
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
            self.fail(OwnerFault::Watchdog);
            self.state = OwnerState::Fault;
            return false;
        }

        if self.wake_owned {
            sdk::wakelock_release(self.config.wake_lock_id);
            self.wake_owned = false;
        }
        self.state = OwnerState::Stopped;
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
            || self.watchdog.fault != WatchdogFault::Ok
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
        self.state == OwnerState::Preparing || self.state == OwnerState::Feeding
    }

    /// One owner iteration; true once the owner has stopped.
    ///
    /// Takes a raw pointer so that no single `&mut Owner` spans the
    /// interrupt-enabled windows in which other tasks post to the mailbox and
    /// ISRs update the board; each step borrows the owner afresh.
    ///
    /// # Safety
    /// `this` is the started, fixed-address owner, polled only by its task.
    pub(super) unsafe fn poll(this: *mut Self) -> bool {
        // SAFETY: the function contract, for the whole body: each step
        // borrows the owner afresh through `this`.
        unsafe {
            if !(*this).initialized && (*this).stop_requested() {
                (*this).state = OwnerState::Stopped;
                (*this).publish(); // No hardware or wake lock has been claimed.
                return true;
            }

            if !(*this).initialized && !(*this).initialize() {
                (*this).state = OwnerState::Fault;
            }

            if (*this).fault == OwnerFault::Ok {
                (*this).check_watchdog();
            }
            if (*this).fault == OwnerFault::Ok {
                (*this).flash_ready();
            }

            let ack = (*this).shared(|shared| core::mem::take(&mut shared.mailbox.acknowledge));
            if ack && (*this).state == OwnerState::Terminal && (*this).result_durable {
                (*this).controller.reset();
                (*this).state = OwnerState::Idle;
            }

            (*this).take_request();
            if (*this).fault == OwnerFault::Ok {
                (*this).check_watchdog(); // A reservation can block.
            }
            if (*this).fault == OwnerFault::Ok {
                (*this).flash_ready();
            }

            if (*this).active_phase() && (*this).cancelled() {
                (*this).controller.cancel((*this).request.id);
                (*this).reporting(OwnerOutcome::Cancelled, FeedReason::Cancelled);
            }

            let mut acquired = false;
            if !(*this).flash().outputs_blocked()
                && ((*this).state == OwnerState::Idle || (*this).active_phase())
            {
                acquired = (*this).acquire();
                if !acquired {
                    (*this).fail(OwnerFault::Hardware);
                    if (*this).reserved
                        && ((*this).active_phase() || (*this).state == OwnerState::Reporting)
                    {
                        (*this).controller.trip(FeedReason::Output);
                        let reason = (*this).controller.reason;
                        (*this).reporting(OwnerOutcome::Failed, reason);
                    } else {
                        (*this).state = OwnerState::Fault;
                    }
                }
            }

            if (*this).fault == OwnerFault::Ok {
                (*this).check_watchdog(); // Before optical/motor enable.
            }
            if (*this).fault == OwnerFault::Ok {
                (*this).flash_ready();
            }
            if (*this).state == OwnerState::Preparing {
                (*this).prepare();
            }

            if (*this).state == OwnerState::Feeding {
                if !(*this).events.motor((*this).controller.motor) {
                    (*this).fail(OwnerFault::Hardware);
                    (*this).controller.trip(FeedReason::Output);
                }
                let reason = (*this).controller.reason;
                match (*this).controller.state {
                    FeedState::Complete => (*this).reporting(OwnerOutcome::Done, reason),
                    FeedState::Cancelled => (*this).reporting(OwnerOutcome::Cancelled, reason),
                    FeedState::Fault => (*this).reporting(OwnerOutcome::Failed, reason),
                    _ => {}
                }
            }

            if (*this).state == OwnerState::Reporting {
                (*this).finish_result();
            }

            if (*this).fault == OwnerFault::Ok
                && !(*this).flash().outputs_blocked()
                && ((*this).state == OwnerState::Idle
                    || (*this).active_phase()
                    || (*this).state == OwnerState::Terminal)
            {
                let mut s = EventsSnapshot::default();
                if (*this).snapshot(&mut s)
                    && s.timer_ready
                    && (((*this).state == OwnerState::Terminal && s.stopped && s.optics_off)
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

            if (*this).state == OwnerState::Fault {
                let quiet = (*this).quiet();
                if quiet && (*this).init_failed {
                    (*this).hold_init_fault();
                }
            }

            let ended = (*this).stop_requested()
                && (*this).state != OwnerState::Reporting
                && !(*this).active_phase()
                && (*this).shutdown();
            (*this).publish();
            ended
        }
    }
}
