//! KM4 TIM2 supervision tick and GPIOA feedback capture.
//!
//! EXPERIMENTAL hardware path; not called by the diagnostic. Requires an
//! opened, stopped board and exclusive ownership of KM4 TIM2 and the GPIOA IRQ
//! vector, plus stable SYSTIMER/GTIMER frequencies supplied by the caller.
//! Every public method and both ISRs run under the interrupt mask.

use core::ffi::c_void;
use core::ptr;

use petlibre_core::c_enum;
use petlibre_core::clock::tick::TickClock;
use petlibre_core::feed::controller::{
    FeedController, FeedReason, FeedRequest, FeedSample, FeedStartResult, MotorCommand,
};
use petlibre_core::feed::feedback::{CAPACITY, FeedbackDrive, FeedbackEvents, FeedbackFault};
use petlibre_core::feed::motor_gate::MotorGateFault;
use petlibre_core::feed::sensor_inputs::SensorInputs;
use petlibre_core::interval_valid;

use crate::board::{self, Board};
use crate::sdk::{self, Irq, IrqGuard};
use crate::supervision::active::active_pointer;

const FEEDBACK_PIN: u32 = sdk::board::FEEDBACK;
const FEEDBACK_MASK: u32 = sdk::board::bit(FEEDBACK_PIN);
const PRIORITY: u32 = 4;

c_enum!(EventsFault {
    OK = 0,
    CLOCK = 1,
    FEEDBACK = 2,
    OUTPUT = 3,
    TIMER = 4,
    RESOURCE = 5,
    SETUP = 6,
    OWNER = 7,
    OPTICS = 8,
});

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventsConfig {
    pub ticks_per_second: u32,
    pub timer_period_ms: u32,
    pub max_feedback_age_ms: u32,
    pub feedback_capacity: u8,
}

/// Fixed-address object; all public methods serialize with both ISR paths.
#[repr(C)]
#[derive(Debug)]
pub struct Events {
    pub board: *mut Board,
    pub feedback: FeedbackEvents,
    pub clock: TickClock,

    pub timer_period_ms: u32,
    pub timer_raw_at: u32,
    pub timer_at: u32,
    pub timer_arr: u32,
    pub owner_at: u32,
    pub optics_at: u32,
    pub optics_max_ms: u32,

    pub fault: EventsFault,
    pub opened: bool,
    pub capture_enabled: bool,
    pub timer_seen: bool,
    pub owner_seen: bool,
    pub optics_armed: bool,
    /// The previous timer interrupt saw no SYSTIMER advance. One such reading
    /// is a pending interrupt coalesced with the next update after a masked
    /// window; a second consecutive one is a frozen clock.
    pub timer_raw_repeat: bool,
    /// Accepted timer interrupts (SYSTIMER advanced); wraps. Diagnostic only.
    pub timer_count: u32,
}

impl Default for Events {
    fn default() -> Self {
        Self {
            board: ptr::null_mut(),
            feedback: FeedbackEvents::default(),
            clock: TickClock::default(),
            timer_period_ms: 0,
            timer_raw_at: 0,
            timer_at: 0,
            timer_arr: 0,
            owner_at: 0,
            optics_at: 0,
            optics_max_ms: 0,
            fault: EventsFault::OK,
            opened: false,
            capture_enabled: false,
            timer_seen: false,
            owner_seen: false,
            optics_armed: false,
            timer_raw_repeat: false,
            timer_count: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventsSnapshot {
    pub now_ms: u32,
    pub fault: EventsFault,
    pub healthy: bool,
    pub timer_ready: bool,
    pub owner_ready: bool,
    pub stopped: bool,
    pub optics_enabled: bool,
    pub optics_off: bool,
    pub capture_enabled: bool,
}

active_pointer!(Events);

/// Period in timer ticks, rounded up; `None` when it does not fit the counter.
pub(crate) fn period_ticks(config: &EventsConfig) -> Option<u32> {
    let ticks =
        (u64::from(config.ticks_per_second) * u64::from(config.timer_period_ms)).div_ceil(1000);
    u32::try_from(ticks).ok().filter(|&t| t != 0)
}

impl Events {
    fn board(&mut self) -> &mut Board {
        // SAFETY: `open` requires the caller to retain the board until close.
        unsafe { &mut *self.board }
    }

    // All helpers below run with KM4 interrupts disabled. No RTOS calls in ISRs.

    fn fail(&mut self, fault: EventsFault) -> bool {
        if self.fault == EventsFault::OK {
            self.fault = fault;
        }
        self.capture_enabled = false;
        sdk::gpio_int_config(FEEDBACK_PIN, false);
        let millis = self.clock.millis;
        let board = self.board();
        board.trip(millis);
        board.optics(false);
        false
    }

    fn now(&mut self) -> Option<u32> {
        let value = self.clock.read(sdk::systimer_ticks());
        if value.is_none() {
            self.fail(EventsFault::CLOCK);
        }
        value
    }

    fn timer_ready(&self, value: u32) -> bool {
        self.timer_seen
            && value.wrapping_sub(self.timer_at)
                <= self.timer_period_ms.wrapping_mul(2).wrapping_add(1)
            && sdk::tim2_en() & sdk::TIM_CR_CNT_STS != 0
            && sdk::tim2_dier() & sdk::TIM_IT_UPDATE != 0
            && sdk::tim2_arr() == self.timer_arr
    }

    fn owner_ready(&self, value: u32) -> bool {
        // SAFETY: as for `board`.
        let lease = unsafe { (*self.board).motor.lease_ms };
        self.owner_seen && value.wrapping_sub(self.owner_at) < lease
    }

    fn stopped(&self) -> bool {
        // SAFETY: as for `board`.
        let gate = unsafe { &(*self.board).motor };
        gate.applied_known
            && gate.applied == MotorCommand::STOP
            && gate.desired == MotorCommand::STOP
    }

    /// Requires an opened, stopped board. Refuses enabled GPIOA interrupts or
    /// a running TIM2 before claiming them.
    ///
    /// # Safety
    /// `board` is null or a live board; the caller retains it and this object
    /// at fixed addresses until close succeeds.
    pub unsafe fn open(&mut self, board: *mut Board, config: &EventsConfig) -> bool {
        // SAFETY: caller supplies null or a live board.
        let Some(b) = (unsafe { board.as_ref() }) else {
            return false;
        };

        if !b.opened
            || b.faulted
            || b.optics_enabled
            || b.motor.fault != MotorGateFault::OK
            || !b.motor.applied_known
            || b.motor.applied != MotorCommand::STOP
            || b.motor.desired != MotorCommand::STOP
            || config.ticks_per_second < 1000
            || config.ticks_per_second > 1_000_000
            || config.timer_period_ms == 0
            || config.timer_period_ms >= b.motor.lease_ms / 3
            || !interval_valid(config.max_feedback_age_ms)
            || config.feedback_capacity == 0
            || usize::from(config.feedback_capacity) > CAPACITY
        {
            return false;
        }

        let Some(ticks) = period_ticks(config) else {
            return false;
        };

        let observed_at = b.motor.observed_at;
        let _guard = IrqGuard::lock();
        if !active().is_null()
            || sdk::gpioa_int_en() != 0
            || sdk::tim2_en() & (sdk::TIM_CR_CNT_START | sdk::TIM_CR_CNT_RUN | sdk::TIM_CR_CNT_STS)
                != 0
        {
            return false;
        }

        let raw = sdk::systimer_ticks();
        *self = Self {
            board,
            timer_period_ms: config.timer_period_ms,
            timer_raw_at: raw,
            timer_arr: ticks - 1,
            ..Self::default()
        };
        self.clock = TickClock::new(config.ticks_per_second, raw, observed_at).unwrap_or_default();
        self.feedback = FeedbackEvents::new(
            config.feedback_capacity,
            config.max_feedback_age_ms,
            self.clock.millis,
        )
        .unwrap_or_default();

        sdk::gtimer_clock_enable();
        // GPIO is in the LP domain; its working clock is a board-open precondition.
        sdk::gpio_int_config(FEEDBACK_PIN, false);
        sdk::gpio_int_mode(FEEDBACK_PIN, true);
        sdk::gpio_int_config(FEEDBACK_PIN, false);
        sdk::gpioa_eoi(FEEDBACK_MASK);
        sdk::tim2_init(self.timer_arr);

        let timer_registered = sdk::irq_register(Irq::Timer2, timer_interrupt, PRIORITY);
        let gpio_registered =
            timer_registered && sdk::irq_register(Irq::GpioA, feedback_interrupt, PRIORITY);
        if !gpio_registered {
            if timer_registered {
                sdk::irq_unregister(Irq::Timer2);
            }
            sdk::tim2_int_config(false);
            sdk::tim2_cmd(false);
            sdk::gpio_int_mode(FEEDBACK_PIN, false);
            self.fault = EventsFault::SETUP;
            return false;
        }

        set_active(self);
        self.opened = true;
        sdk::tim2_int_clear();
        sdk::irq_enable(Irq::Timer2, PRIORITY);
        sdk::irq_enable(Irq::GpioA, PRIORITY);
        sdk::tim2_int_config(true);
        sdk::tim2_cmd(true);
        true
    }

    /// Read at acquisition START; compose health using another read later.
    pub fn now_ms(&mut self) -> Option<u32> {
        if !self.opened {
            return None;
        }
        let _guard = IrqGuard::lock();
        self.now()
    }

    /// Fill `snapshot` even when the clock read fails; returns the read result.
    pub fn snapshot(&mut self, snapshot: &mut EventsSnapshot) -> bool {
        if !self.opened {
            return false;
        }
        let _guard = IrqGuard::lock();
        let read = self.now();
        let value = read.unwrap_or(self.clock.millis);

        // SAFETY: as for `board`.
        let b = unsafe { &*self.board };
        *snapshot = EventsSnapshot {
            now_ms: value,
            fault: self.fault,
            healthy: read.is_some()
                && self.fault == EventsFault::OK
                && !b.faulted
                && b.motor.fault == MotorGateFault::OK,
            timer_ready: self.timer_ready(value),
            owner_ready: self.owner_ready(value),
            stopped: self.stopped(),
            optics_enabled: b.optics_enabled,
            optics_off: board::optics_off(),
            capture_enabled: self.capture_enabled,
        };

        read.is_some()
    }

    /// Arm a non-renewable maximum on-time. Repeated enables do not extend it.
    /// Only successful owner-side sample processing renews the liveness lease.
    pub fn optics(&mut self, enabled: bool, max_on_ms: u32) -> bool {
        self.optics_select(enabled, enabled, max_on_ms)
    }

    /// As `optics`, with PA15 to `a` and PB3 to `b`. While on, a repeated
    /// enable neither extends the budget nor changes the selection: it
    /// returns whether the emitters already match it.
    pub fn optics_select(&mut self, a: bool, b: bool, max_on_ms: u32) -> bool {
        let enabled = a || b;
        if !self.opened || (enabled && !interval_valid(max_on_ms)) {
            return false;
        }

        let _guard = IrqGuard::lock();
        let Some(value) = self.now() else {
            return false;
        };

        if enabled
            && (self.fault != EventsFault::OK
                || !self.timer_ready(value)
                || !self.owner_ready(value))
        {
            return false;
        }

        if enabled && self.board().optics_enabled {
            return board::optics_are(a, b);
        }
        if !self.board().optics_select(a, b) {
            return self.fail(EventsFault::OUTPUT);
        }

        self.optics_armed = enabled;
        self.optics_at = value;
        self.optics_max_ms = max_on_ms;
        true
    }

    /// Enable only while verified stopped and after observing a timer
    /// interrupt. Disabling capture while driving trips STOP.
    pub fn capture(&mut self, enabled: bool) -> bool {
        if !self.opened {
            return false;
        }
        let _guard = IrqGuard::lock();
        let stopped = self.stopped();
        let read = self.now();

        if !enabled {
            sdk::gpio_int_config(FEEDBACK_PIN, false);
            self.capture_enabled = false;
            if !stopped {
                return self.fail(EventsFault::FEEDBACK);
            }
            if let Some(value) = read
                && self.fault == EventsFault::OK
                && self.feedback.fault == FeedbackFault::OK
            {
                // Discard completed/cancelled-request edges only while stopped.
                // Never erase a latched fault to prepare a later request.
                self.reset_feedback(value);
            }
            return read.is_some();
        }

        let Some(value) = read else { return false };
        if !stopped || self.fault != EventsFault::OK || !self.timer_ready(value) {
            return false;
        }

        self.reset_feedback(value);
        sdk::gpioa_eoi(FEEDBACK_MASK);
        sdk::gpio_int_config(FEEDBACK_PIN, true);
        self.capture_enabled = sdk::gpioa_int_en() & FEEDBACK_MASK != 0
            && sdk::gpioa_int_mask() & FEEDBACK_MASK == 0
            && sdk::gpioa_int_bothedge() & FEEDBACK_MASK != 0;
        self.capture_enabled || self.fail(EventsFault::SETUP)
    }

    fn reset_feedback(&mut self, value: u32) {
        if let Some(fresh) =
            FeedbackEvents::new(self.feedback.capacity, self.feedback.max_age_ms, value)
        {
            self.feedback = fresh;
        }
    }

    fn process_with(
        &mut self,
        controller: &mut FeedController,
        sample: Option<&FeedSample>,
        inputs: Option<&SensorInputs>,
    ) -> bool {
        if !self.opened {
            return false;
        }
        let _guard = IrqGuard::lock();
        let read = self.now().filter(|_| self.fault == EventsFault::OK);
        let mut result = read.is_some();
        if let Some(value) = read
            && self.board().optics_enabled
            && !self.owner_ready(value)
        {
            result = self.fail(EventsFault::OWNER);
        }

        let Some(value) = read.filter(|_| result) else {
            controller.trip(if self.fault == EventsFault::FEEDBACK {
                FeedReason::FEEDBACK_STREAM
            } else {
                FeedReason::OUTPUT
            });
            return false;
        };

        let composed = inputs.map(|inputs| inputs.sample(value, false, false));
        let b = self.board();
        let drive = FeedbackDrive {
            healthy: !b.faulted && b.motor.fault == MotorGateFault::OK,
            applied_known: b.motor.applied_known,
            applied: b.motor.applied,
            applied_at: b.motor.applied_at,
        };

        let ok = match composed.as_ref().or(sample) {
            Some(sample) => self.feedback.process(controller, value, sample, drive),
            None => {
                // A missing sample fails the stream exactly like the C ABI.
                self.feedback.fail(FeedbackFault::ARGUMENT);
                controller.trip(FeedReason::FEEDBACK_STREAM);
                false
            }
        };

        if !ok {
            self.fail(if self.feedback.fault == FeedbackFault::OK {
                EventsFault::OUTPUT
            } else {
                EventsFault::FEEDBACK
            });
        } else {
            self.owner_at = value;
            self.owner_seen = true;
        }

        ok
    }

    /// Owner-side processing under one short lock; never acquires ADC here.
    pub fn process(
        &mut self,
        controller: &mut FeedController,
        sample: Option<&FeedSample>,
    ) -> bool {
        self.process_with(controller, sample, None)
    }

    /// Compose freshness at the exact processing clock read, inside serialization.
    pub fn observe(&mut self, controller: &mut FeedController, inputs: &SensorInputs) -> bool {
        self.process_with(controller, None, Some(inputs))
    }

    /// Reserve the request durably BEFORE calling. Freshness, capture and
    /// initial output application are serialized. A rejected input never
    /// drives; an output failure can leave an accepted controller faulted.
    pub fn start(
        &mut self,
        controller: &mut FeedController,
        inputs: &SensorInputs,
        request: &FeedRequest,
        latest_start_ms: u32,
    ) -> FeedStartResult {
        if !self.opened {
            return FeedStartResult::REJECT_REQUEST;
        }
        let _guard = IrqGuard::lock();
        let Some(value) = self.now() else {
            return FeedStartResult::REJECT_INPUT;
        };

        let remaining = latest_start_ms.wrapping_sub(value);
        if self.fault != EventsFault::OK
            || !self.timer_ready(value)
            || !self.owner_ready(value)
            || !interval_valid(remaining)
            || self.feedback.max_age_ms >= controller.limits.feedback_inhibit_ms
        {
            return FeedStartResult::REJECT_INPUT;
        }

        let sample = inputs.sample(value, false, false);
        let result = controller.start(request, value, &sample);
        if result == FeedStartResult::ACCEPTED {
            let motor = controller.motor;
            let ready = self.capture(true) && self.board().motor(motor, value);
            if !ready {
                self.fail(EventsFault::OUTPUT);
                controller.trip(FeedReason::OUTPUT);
                return FeedStartResult::REJECT_INPUT;
            }
        }
        result
    }

    /// Drive requires a recently observed timer and enabled healthy capture.
    #[must_use]
    pub fn motor(&mut self, command: MotorCommand) -> bool {
        if !self.opened {
            return false;
        }
        let _guard = IrqGuard::lock();
        let Some(value) = self.now() else {
            return false;
        };

        if command != MotorCommand::STOP {
            if self.fault != EventsFault::OK || !self.capture_enabled || !self.timer_ready(value) {
                return self.fail(EventsFault::TIMER);
            }
            if !self.owner_ready(value) {
                return self.fail(EventsFault::OWNER);
            }
        }

        self.board().motor(command, value) || self.fail(EventsFault::OUTPUT)
    }

    /// If STOP or infrared OFF cannot be verified, returns false and retains
    /// the retry timer; supervision is never removed from an active output.
    pub fn close(&mut self) -> bool {
        if !self.opened {
            return false;
        }
        let _guard = IrqGuard::lock();
        let millis = self.clock.millis;
        self.board().motor(MotorCommand::STOP, millis);
        sdk::gpio_int_config(FEEDBACK_PIN, false);
        self.capture_enabled = false;
        let optics_off = self.board().optics(false) && board::optics_off();
        self.optics_armed = false;

        if !self.stopped_applied() || !optics_off {
            self.fail(EventsFault::OUTPUT);
            return false;
        }

        sdk::irq_disable(Irq::GpioA);
        sdk::irq_disable(Irq::Timer2);
        sdk::tim2_int_config(false);
        sdk::tim2_cmd(false);
        sdk::gpio_int_mode(FEEDBACK_PIN, false);
        sdk::irq_unregister(Irq::GpioA);
        sdk::irq_unregister(Irq::Timer2);
        set_active(ptr::null_mut());
        self.opened = false;
        true
    }

    fn stopped_applied(&self) -> bool {
        // SAFETY: as for `board`.
        let gate = unsafe { &(*self.board).motor };
        gate.applied_known && gate.applied == MotorCommand::STOP
    }
}

unsafe extern "C" fn timer_interrupt(_: *mut c_void) -> u32 {
    let _guard = IrqGuard::lock();
    sdk::tim2_int_clear();
    // SAFETY: `active` is the registered, opened events object.
    let Some(events) = (unsafe { active().as_mut() }).filter(|e| e.opened) else {
        return 0;
    };

    let raw = sdk::systimer_ticks();
    if raw == events.timer_raw_at {
        if !events.timer_raw_repeat {
            // A pending update taken at unmask can coincide with the next one
            // within a SYSTIMER tick: skip once without advancing anything.
            events.timer_raw_repeat = true;
            return 0;
        }
        events.fail(EventsFault::CLOCK);
        return 0;
    }

    events.timer_raw_repeat = false;
    let value = events.clock.read(raw);
    let Some(value) = value else {
        events.fail(EventsFault::CLOCK);
        return 0;
    };
    events.timer_raw_at = raw;
    events.timer_count = events.timer_count.wrapping_add(1);
    events.timer_at = value;
    events.timer_seen = true;

    if events.fault != EventsFault::OK {
        let fault = events.fault;
        events.fail(fault); // Retry unverified GPIO stops.
    } else if !events.board().motor_tick(value) {
        events.fail(EventsFault::OUTPUT);
    } else if events.board().optics_enabled && !events.owner_ready(value) {
        events.fail(EventsFault::OWNER);
    } else if events.board().optics_enabled
        && (!events.optics_armed || value.wrapping_sub(events.optics_at) >= events.optics_max_ms)
    {
        events.fail(EventsFault::OPTICS);
    }
    0
}

unsafe extern "C" fn feedback_interrupt(_: *mut c_void) -> u32 {
    let _guard = IrqGuard::lock();
    let pending = sdk::gpioa_int_status();
    sdk::gpioa_eoi(pending);
    // SAFETY: as for `timer_interrupt`.
    let Some(events) = (unsafe { active().as_mut() }).filter(|e| e.opened) else {
        return 0;
    };

    if pending & !FEEDBACK_MASK != 0 {
        events.fail(EventsFault::RESOURCE);
        sdk::irq_disable(Irq::GpioA);
    } else if pending & FEEDBACK_MASK != 0
        && events.capture_enabled
        && let Some(value) = events.now()
    {
        let high = sdk::gpioa_ext_port() & FEEDBACK_MASK != 0;
        if !events.feedback.push(value, high) {
            events.fail(EventsFault::FEEDBACK);
        }
    }
    0
}
