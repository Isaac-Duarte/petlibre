//! The owner task's lifecycle state machine: initialization, request
//! reservation, preparation, feeding, result recording and shutdown.

use wellfed_core::HALF_RANGE;
use wellfed_core::feed::controller::{
    FeedController, FeedReason, FeedRequest, FeedStartResult, FeedState, MotorCommand,
};
use wellfed_core::feed::observe::{Beams, Observation};
use wellfed_core::feed::progress::{BowlMode, Direction};
use wellfed_core::feed::sensor_inputs::{SensorInputs, SensorReading};
use wellfed_core::feed::storage::{OwnerOutcome, OwnerResult, OwnerStorage};
use wellfed_core::flash_coord::{FlashCoord, FlashFault, FlashPhase};

use crate::board::{self, Board, CaptureFault};
use crate::power;
use crate::sdk::{self, IrqGuard};
use crate::supervision::beat;
use crate::supervision::events::{Events, EventsSnapshot};

use super::{FeedTrace, OwnerFault, OwnerState, OwnerStatus, OwnerTask, Shared};

/// Why an acquisition gave the owner nothing to act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lost {
    /// An ADC conversion never completed.
    Sensors,
    /// The supervisor's clock or processing failed, or the board was
    /// already faulted by something else.
    Supervision,
}

impl<S: OwnerStorage> OwnerTask<S> {
    /// The shared half, inside `IrqGuard`; the borrow ends with `f`, which
    /// must not call back in.
    fn shared<R>(&self, f: impl FnOnce(&mut Shared) -> R) -> R {
        let _guard = IrqGuard::lock();
        // SAFETY: `start` pointed this at the owner's shared half, which lives
        // as long as the owner; inside the guard this is its only borrow.
        f(unsafe { &mut *self.shared })
    }

    /// The owner's board. A shared handle, so the borrow is not tied to the
    /// task half's: a step can drive the board while it updates the
    /// controller.
    fn board<'b>(&self) -> &'b Board {
        // SAFETY: `open` pointed this at the owner's board, which lives as
        // long as the owner and is reached only through `&`.
        unsafe { &*self.board }
    }

    /// The owner's supervisor, as for `board`.
    fn events<'b>(&self) -> &'b Events {
        // SAFETY: as for `board`.
        unsafe { &*self.events }
    }

    fn flash(&self) -> &FlashCoord {
        // SAFETY: `start` requires a live authority retained beyond STOPPED.
        unsafe { &*self.config.flash }
    }

    fn snapshot(&mut self, s: &mut EventsSnapshot) -> bool {
        *s = EventsSnapshot::default();
        if self.events().opened() {
            self.events().snapshot(s);
            return s.healthy;
        }
        let b = self.board().view();
        s.stopped = b.motor.pins.is_some()
            && b.motor.applied_known
            && b.motor.applied == MotorCommand::Stop;
        s.optics_off = b.optics_initialized && board::optics_off();
        false
    }

    fn publish(&mut self) {
        let mut s = EventsSnapshot::default();
        self.snapshot(&mut s);

        let mut value = OwnerStatus {
            state: self.state,
            fault: self.fault,
            supervisor_fault: s.fault,
            result: self.result,
            high_water_id: self.recovery.high_water_id,
            phase_known: self.recovery.phase_known,
            intent_reserved: self.reserved,
            result_durable: self.result_durable,
            stopped: s.stopped,
            optics_off: s.optics_off,
            trace: self.trace,
            food: self.food,
            ..OwnerStatus::default()
        };

        let clock = self.events().now_ms();
        let _guard = IrqGuard::lock();
        value.flash_phase = self.flash().phase;
        value.flash_fault = self.flash().fault;
        self.shared(|shared| {
            shared.published = value;
            shared.clock_ms = clock;
        });
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
        if self.events().opened() {
            // Best effort: the outputs are verified stopped below.
            let _ = self.events().motor(MotorCommand::Stop);
            self.events().optics(false, 0);
        } else {
            let gate = self.board().view().motor;
            if gate.pins.is_some() {
                self.board().trip(gate.observed_at);
                self.board().optics(false);
            }
        }

        let mut s = EventsSnapshot::default();
        self.snapshot(&mut s);
        if !s.stopped || !s.optics_off {
            return false;
        }
        if self.events().opened() {
            self.events().capture(false);
        }
        true
    }

    fn reporting(&mut self, outcome: OwnerOutcome, reason: FeedReason) {
        if let Some(at) = self.drive_at.take() {
            let now = self.events().now_ms().unwrap_or(at);
            let edges = self.events().edges();
            self.trace.drive_ms = now.wrapping_sub(at);
            self.trace.rising = edges.rising;
            self.trace.falling = edges.falling;
            self.trace.pins = edges.pins;
            // Capture opens just before the drive, so an edge can predate it.
            let since = |edge: u32| {
                let since = edge.wrapping_sub(at);
                if since < HALF_RANGE { since } else { 0 }
            };
            self.trace.first_edge_ms = edges.first_at.map(since);
            for (logged, edge) in self.trace.edge_ms.iter_mut().zip(edges.log) {
                *logged = edge.map(|(edge, high)| (since(edge), high));
            }
        }
        self.result.outcome = outcome;
        self.result.reason = reason;
        let ours = self.controller.last_request_id == self.request.id;
        self.result.completed = if ours {
            self.controller.progress.completed
        } else {
            0
        };
        self.trace.reversals = if ours {
            self.controller.reverse_attempts
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

    /// The gap that applies now: the active one while outputs may move.
    fn gap_ms(&self) -> u32 {
        if self.active_phase() || self.state == OwnerState::Reporting {
            self.config.max_progress_gap_ms
        } else {
            self.config.idle_gap_ms
        }
    }

    /// Whether the owner's own last beat (or its open) is at least the
    /// current gap old, the same edge as `Beat::fresh`.
    fn overdue(&self) -> bool {
        let ticks = sdk::systimer_ticks().wrapping_sub(self.beat_raw);
        let tps = u64::from(self.config.events.ticks_per_second);
        u64::from(ticks) * 1000 >= u64::from(self.gap_ms()) * tps
    }

    /// A step that blocked past the gap faults the owner: by then the
    /// runtime may already be withholding the watchdog, and nothing the
    /// owner was supervising may continue.
    fn check_progress(&mut self) -> bool {
        if !self.overdue() {
            return true;
        }
        self.beat_lost = true;
        self.fail(OwnerFault::Progress);
        self.abort();
        self.quiet();
        false
    }

    /// Publish a beat, unless a flash lease blocks the outputs: the lease
    /// counts against the gap (`OwnerConfig::lease_fits`).
    /// Once the gap has passed, the owner never beats again, as a watchdog
    /// latches a missed deadline: the runtime then resets the platform.
    /// False once the beat is lost.
    fn beat(&mut self, supervised: bool) -> bool {
        if self.overdue() {
            self.beat_lost = true;
        }
        if self.beat_lost {
            return false;
        }
        if !self.flash().outputs_blocked() {
            beat::beat(sdk::uptime_ms(), self.gap_ms(), supervised);
            self.beat_raw = sdk::systimer_ticks();
        }
        true
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
                || self.board().view().optics_enabled
                || self.board().view().motor.desired != MotorCommand::Stop);

        if blocked && self.board_attempted && !self.board().flash_quiesce() {
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
        let result = self.result;
        if self
            .storage
            .as_mut()
            .is_some_and(|storage| storage.finish(&result, &next))
        {
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

        // The beat `open` started covers a blocked board initialization or
        // recovery: the step after it finds the owner overdue.
        self.board_attempted = true;
        let c = self.config;
        let board = self.board();
        // SAFETY: the owner, its board and the authority stay at fixed addresses.
        let opened = board.open(c.motor_deadtime_ms, c.motor_lease_ms, c.adc_poll_limit, 0)
            && unsafe { board.attach_flash(c.flash) }
            && unsafe { self.events().open(board, &c.events) };
        if !opened {
            self.fail(OwnerFault::Setup);
            self.held = true;
            return false;
        }

        if !self.recover() {
            return self.recovery_failed();
        }

        let controller = FeedController::new(&c.feed, self.recovery.high_water_id);
        let sensors = SensorInputs::new(&c.sensors, &board.view().calibration);
        let (Some(controller), Some(sensors)) = (controller, sensors) else {
            self.fail(OwnerFault::Setup);
            self.held = true;
            return false;
        };
        self.controller = controller;
        self.sensors = sensors;

        // A reservation a reset interrupted is never resumed or retried:
        // what it dispensed is unknown, so it ends uncertain, under its ID.
        let resolved = if self.recovery.pending_request {
            let Some(result) = self.resolve() else {
                return self.recovery_failed();
            };
            Some(result)
        } else {
            None
        };
        if c.bowl_mode == BowlMode::Single {
            self.recovery.phase_known = true;
        }

        let high_water = self.recovery.high_water_id;
        self.shared(|shared| shared.last_submitted_id = high_water);
        self.state = match resolved {
            None => OwnerState::Idle,
            // Reported like any durable result: Idle once acknowledged.
            Some(result) => {
                self.result = result;
                self.result_durable = true;
                OwnerState::Terminal
            }
        };
        true
    }

    /// Read the journal's recovery state: false when it is unreadable or
    /// holds a phase this bowl cannot have.
    fn recover(&mut self) -> bool {
        let readable = self
            .storage
            .as_mut()
            .is_some_and(|storage| storage.recover(&mut self.recovery));
        readable
            && (self.config.bowl_mode != BowlMode::Single
                || self.recovery.direction == Direction::D0)
    }

    /// Record the pending reservation as uncertain, then read the state
    /// after it: the result, or `None` when it could not be recorded. The
    /// reservation then stays pending, and its ID consumed, for the next
    /// boot.
    fn resolve(&mut self) -> Option<OwnerResult> {
        let result = self.storage.as_mut()?.resolve()?;
        let resolved = self.recover()
            && !self.recovery.pending_request
            && self.recovery.high_water_id == result.id;
        resolved.then_some(result)
    }

    /// Latch a recovery fault: no request is ever taken this boot.
    fn recovery_failed(&mut self) -> bool {
        self.fail(OwnerFault::Recovery);
        self.recovery.phase_known = false;
        self.held = true;
        false
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
            self.blocked_by = FeedReason::None;
            self.trace = FeedTrace::default();
            self.drive_at = None;
            self.observation = None;
            self.result = OwnerResult {
                id: self.request.id,
                requested: self.request.portions,
                ..OwnerResult::default()
            };
            // A fresh beat at rest before the active gap applies, so a loop
            // held off just before the request does not fault the feed.
            let _ = self.beat(false);
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
        } else if !self
            .storage
            .as_mut()
            .is_some_and(|storage| storage.reserve(&self.request))
        {
            self.fail(OwnerFault::Storage);
            self.state = OwnerState::Fault;
        } else {
            self.reserved = true;
            self.recovery.high_water_id = self.request.id;
            self.recovery.pending_request = true;
        }
    }

    fn acquire(&mut self) -> Result<(), Lost> {
        let Some(started) = self.events().now_ms() else {
            return Err(Lost::Supervision);
        };
        let uptime = sdk::uptime_ms();
        let mut raw = SensorReading::default();
        let captured = self.board().capture(&mut raw);
        let valid = captured.is_ok();
        self.sensors.update(started, valid.then_some(&raw));
        if let Some(present) = self.sensors.food_present() {
            self.food = Some(present);
        }
        if self.drive_at.is_some() && self.sensors.current_valid {
            let current = self.sensors.current_scaled;
            let trace = &mut self.trace;
            trace.current_max = trace.current_max.max(current);
            trace.current_sum = trace.current_sum.saturating_add(current);
            trace.current_samples = trace.current_samples.saturating_add(1);
        }

        // The owner holds the ADC, so its captures stand in for the
        // runtime's power monitor.
        let pin_mv = valid
            .then(|| self.board().view().calibration.scale_16(&raw.battery))
            .flatten();
        if let Some(pin_mv) = pin_mv {
            power::publish(pin_mv, raw.dc_present, uptime);
        }

        let active = self.active_phase();
        let lost = match captured {
            Ok(()) => None,
            Err(CaptureFault::Adc) => Some(Lost::Sensors),
            Err(CaptureFault::Board) => Some(Lost::Supervision),
        };

        // Cancellation has priority over feedback captured during acquisition.
        let observed = if active && self.cancelled() {
            self.controller.cancel(self.request.id);
            self.reporting(OwnerOutcome::Cancelled, FeedReason::Cancelled);
            lost.map_or(Ok(()), Err)
        } else if let Some(lost) = lost {
            Err(lost)
        } else if self.events().observe(&mut self.controller, &self.sensors) {
            Ok(())
        } else {
            Err(Lost::Supervision)
        };

        if active {
            let beams = (valid && raw.gpio_valid && raw.optics_enabled).then_some(Beams {
                outlet_clear: raw.outlet_high,
                food: !raw.food_high,
            });
            let edges = self.events().edge_tally();
            self.observation = Some(Observation {
                at_ms: started,
                drive_at_ms: self.drive_at,
                current: self
                    .sensors
                    .current_valid
                    .then_some(self.sensors.current_scaled),
                battery_mv: pin_mv.and_then(power::pack_mv),
                beams,
                rising: edges.rising,
                falling: edges.falling,
                last_edge: edges.last,
                motor: self.controller.motor,
            });
        }
        observed
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

        // An input that kept it from starting (a blocked outlet, a sensor,
        // power) fails it with that reason, before any motion; otherwise it
        // only ran out of time. The journal holds an expired feed to
        // `TotalTimeout`, so a reason of its own needs the failed outcome.
        if s.now_ms.wrapping_sub(self.queued_at) >= self.config.prepare_timeout_ms {
            match self.blocked_by {
                FeedReason::None => self.reporting(OwnerOutcome::Expired, FeedReason::TotalTimeout),
                reason => self.reporting(OwnerOutcome::Failed, reason),
            }
            return;
        }

        // Held off by a lease or the supervisor, not by an input: an input
        // seen before no longer says why it has not started.
        if !self.flash_ready() || !s.timer_ready || !s.owner_ready {
            self.blocked_by = FeedReason::None;
            return;
        }

        if !self.optics_started {
            if !self.events().optics(true, self.config.optics_max_on_ms) {
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
                self.events()
                    .start(&mut self.controller, &self.sensors, &self.request, latest);
            self.result.motion_possible = self.controller.last_request_id == self.request.id;
            if result == FeedStartResult::Accepted {
                self.state = OwnerState::Feeding;
                self.drive_at = self.events().now_ms();
            } else if self.result.motion_possible {
                self.fail(OwnerFault::Hardware);
                let reason = self.controller.reason;
                self.reporting(OwnerOutcome::Failed, reason);
            } else if result == FeedStartResult::RejectInput {
                let sample = self.sensors.sample(s.now_ms, false, false);
                self.blocked_by = self.controller.input_refusal(&sample);
            }
        }
    }

    fn shutdown(&mut self) -> bool {
        if self.board_attempted && !self.quiet() {
            return false;
        }
        if self.events().opened() && !self.events().close() {
            return false;
        }
        self.board().close();

        if self.wake_owned {
            sdk::wakelock_release(self.config.wake_lock_id);
            self.wake_owned = false;
        }
        self.state = OwnerState::Stopped;
        true
    }

    /// Terminal stopped fault after a determined failure (initialization,
    /// or lost sensors): no request is ever taken, and the owner beats only
    /// after `quiet` verified STOP and infrared OFF in this same iteration.
    /// The gap still applies, so a hang before or during this state resets.
    fn hold_fault(&mut self) {
        let mut s = EventsSnapshot::default();
        self.snapshot(&mut s);
        // Gate state alone is not enough here: also read back the pins.
        if !s.stopped || !s.optics_off || !self.board().inactive() {
            return;
        }
        // A lost beat stays lost; the latched fault already holds STOP.
        let _ = self.beat(false);
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
                (*this).check_progress();
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
                (*this).check_progress(); // A reservation can block.
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
                let result = (*this).acquire();
                acquired = result.is_ok();
                if let Err(lost) = result {
                    let (fault, reason) = match lost {
                        Lost::Sensors => (OwnerFault::Sensor, FeedReason::Sensor),
                        Lost::Supervision => (OwnerFault::Hardware, FeedReason::Output),
                    };
                    (*this).fail(fault);
                    // Lost sensors are held, not reset on: a reset would
                    // only meet the same converter again, every few
                    // seconds. Held, the feeder stays reachable and says why
                    // it cannot feed until a restart; outputs are
                    // re-verified stopped on every beat. (A trial build's
                    // faulted owner fails its proof of feeding, and the
                    // runtime resets that one at once.) A lost supervisor
                    // still resets.
                    (*this).held |= lost == Lost::Sensors;
                    if (*this).reserved
                        && ((*this).active_phase() || (*this).state == OwnerState::Reporting)
                    {
                        // The supervisor may already have tripped it with
                        // its own reason, which it keeps. A request that
                        // ended otherwise (cancelled in this acquisition)
                        // fails with the loss: the journal holds a failed
                        // result to a fault's reason.
                        let reason = if (*this).controller.trip(reason) {
                            reason
                        } else {
                            match (*this).controller.reason {
                                FeedReason::None | FeedReason::Done | FeedReason::Cancelled => {
                                    reason
                                }
                                tripped => tripped,
                            }
                        };
                        (*this).reporting(OwnerOutcome::Failed, reason);
                    } else {
                        (*this).state = OwnerState::Fault;
                    }
                }
            }

            if (*this).fault == OwnerFault::Ok {
                (*this).check_progress(); // Before optical/motor enable.
            }
            if (*this).fault == OwnerFault::Ok {
                (*this).flash_ready();
            }
            if (*this).state == OwnerState::Preparing {
                (*this).prepare();
            }

            if (*this).state == OwnerState::Feeding {
                if !(*this).events().motor((*this).controller.motor) {
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
                    // Supervised: the runtime trusts this supervisor, not a
                    // stopped readback, until the beat goes stale.
                    let supervised = (*this).active_phase();
                    if !(*this).beat(supervised) {
                        (*this).check_progress();
                    }
                }
            }

            if (*this).state == OwnerState::Fault {
                let quiet = (*this).quiet();
                if quiet && (*this).held {
                    (*this).hold_fault();
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
