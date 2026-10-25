//! KM4 TIM2 supervision tick and GPIOA feedback capture.
//!
//! EXPERIMENTAL hardware path; not called by the diagnostic. Requires an
//! opened, stopped board and exclusive ownership of KM4 TIM2 and the GPIOA IRQ
//! vector, plus stable SYSTIMER/GTIMER frequencies supplied by the caller.
//! Every public method and both ISRs run under the interrupt mask.

use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::ptr;

use wellfed_core::clock::tick::TickClock;
use wellfed_core::feed::controller::{
    FeedController, FeedReason, FeedRequest, FeedSample, FeedStartResult, MotorCommand,
};
use wellfed_core::feed::feedback::{CAPACITY, FeedbackDrive, FeedbackEvents, FeedbackFault};
use wellfed_core::feed::motor_gate::MotorGateFault;
use wellfed_core::feed::sensor_inputs::SensorInputs;
use wellfed_core::interval_valid;

use crate::board::{self, Board};
use crate::sdk::{self, Irq, IrqGuard};
use crate::supervision::active::active_pointer;

const FEEDBACK_PIN: u32 = sdk::board::FEEDBACK;
const FEEDBACK_MASK: u32 = sdk::board::bit(FEEDBACK_PIN);
const PRIORITY: u32 = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum EventsFault {
    #[default]
    Ok = 0,
    Clock = 1,
    Feedback = 2,
    Output = 3,
    Timer = 4,
    Resource = 5,
    Setup = 6,
    Owner = 7,
    Optics = 8,
}

impl EventsFault {
    /// The variant's number, as records and reports print it.
    pub const fn code(self) -> u32 {
        self as u32
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventsConfig {
    pub ticks_per_second: u32,
    pub timer_period_ms: u32,
    pub max_feedback_age_ms: u32,
    pub feedback_capacity: u8,
}

/// The supervisor's state, as [`Events::view`] copies it out.
#[derive(Clone, Copy, Debug)]
pub struct EventsState {
    pub board: *const Board,
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
    /// Every feedback edge since capture was last enabled. Diagnostic only.
    pub edges: FeedbackEdges,
}

/// Every edge the feedback interrupt saw, qualified or not, counted apart
/// from the controller's queue so a run shows what the sensor does even
/// when the controller credits nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeedbackEdges {
    pub rising: u16,
    pub falling: u16,
    /// The supervisor's clock at the first one.
    pub first_at: Option<u32>,
    /// The first `EDGE_LOG` edges: the supervisor's clock and the level after.
    pub log: [Option<(u32, bool)>; EDGE_LOG],
    pub pins: PinWatch,
}

/// How many feedback edges a feed keeps the time of.
pub const EDGE_LOG: usize = 8;

/// Both GPIO ports' pad inputs, sampled every supervision tick while capture
/// is on, so a run shows which pad a sensor really reaches. Only pads muxed
/// to GPIO read back; a pulse shorter than the tick can be missed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PinWatch {
    /// Ports A and B at the first sample.
    pub start: Option<[u32; 2]>,
    last: [u32; 2],
    /// Every pad that read differently from the sample before.
    pub changed: [u32; 2],
    /// Changes per pad, by port, saturating.
    pub toggles: [[u8; 32]; 2],
}

impl PinWatch {
    fn sample(&mut self, ports: [u32; 2]) {
        if self.start.is_none() {
            self.start = Some(ports);
            self.last = ports;
            return;
        }

        for (port, &value) in ports.iter().enumerate() {
            let diff = value ^ self.last[port];
            self.changed[port] |= diff;
            for (pin, count) in self.toggles[port].iter_mut().enumerate() {
                if diff & (1 << pin) != 0 {
                    *count = count.saturating_add(1);
                }
            }
        }
        self.last = ports;
    }
}

impl Default for EventsState {
    fn default() -> Self {
        Self {
            board: ptr::null(),
            feedback: FeedbackEvents::default(),
            clock: TickClock::default(),
            timer_period_ms: 0,
            timer_raw_at: 0,
            timer_at: 0,
            timer_arr: 0,
            owner_at: 0,
            optics_at: 0,
            optics_max_ms: 0,
            fault: EventsFault::Ok,
            opened: false,
            capture_enabled: false,
            timer_seen: false,
            owner_seen: false,
            optics_armed: false,
            timer_raw_repeat: false,
            timer_count: 0,
            edges: FeedbackEdges::default(),
        }
    }
}

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

/// The supervisor handle. The owning task and both interrupts reach it
/// through `&Events`; its state is borrowed only inside `IrqGuard`, for the
/// length of one call, so no borrow of it spans an interrupt-enabled window.
/// Keep it at a fixed address from `open` until `close` succeeds.
#[derive(Debug, Default)]
pub struct Events {
    state: UnsafeCell<EventsState>,
}

active_pointer!(Events);

/// Period in timer ticks, rounded up; `None` when it does not fit the counter.
pub(crate) fn period_ticks(config: &EventsConfig) -> Option<u32> {
    let ticks =
        (u64::from(config.ticks_per_second) * u64::from(config.timer_period_ms)).div_ceil(1000);
    u32::try_from(ticks).ok().filter(|&t| t != 0)
}

impl EventsState {
    /// The supervised board. A shared handle, so the borrow is not tied to
    /// this state's.
    fn board<'b>(&self) -> &'b Board {
        // SAFETY: `open` requires the caller to retain the board at a fixed
        // address until close; `Board` is reached only through `&`.
        unsafe { &*self.board }
    }

    // All helpers below run with KM4 interrupts disabled. No RTOS calls in ISRs.

    fn fail(&mut self, fault: EventsFault) -> bool {
        if self.fault == EventsFault::Ok {
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
            self.fail(EventsFault::Clock);
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
        let lease = self.board().view().motor.lease_ms;
        self.owner_seen && value.wrapping_sub(self.owner_at) < lease
    }

    fn stopped(&self) -> bool {
        let gate = self.board().view().motor;
        gate.applied_known
            && gate.applied == MotorCommand::Stop
            && gate.desired == MotorCommand::Stop
    }

    /// As `Events::open`, which passes its own address as `this`.
    ///
    /// # Safety
    /// As for `Events::open`.
    unsafe fn open(
        &mut self,
        this: *const Events,
        board: *const Board,
        config: &EventsConfig,
    ) -> bool {
        // SAFETY: caller supplies null or a live board.
        let Some(b) = (unsafe { board.as_ref() }).map(Board::view) else {
            return false;
        };

        if !b.opened
            || b.faulted
            || b.optics_enabled
            || b.motor.fault != MotorGateFault::Ok
            || !b.motor.applied_known
            || b.motor.applied != MotorCommand::Stop
            || b.motor.desired != MotorCommand::Stop
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
            self.fault = EventsFault::Setup;
            return false;
        }

        set_active(this.cast_mut());
        self.opened = true;
        sdk::tim2_int_clear();
        sdk::irq_enable(Irq::Timer2, PRIORITY);
        sdk::irq_enable(Irq::GpioA, PRIORITY);
        sdk::tim2_int_config(true);
        sdk::tim2_cmd(true);
        true
    }

    fn now_ms(&mut self) -> Option<u32> {
        if !self.opened {
            return None;
        }
        self.now()
    }

    fn snapshot(&mut self, snapshot: &mut EventsSnapshot) -> bool {
        if !self.opened {
            return false;
        }
        let read = self.now();
        let value = read.unwrap_or(self.clock.millis);

        let b = self.board().view();
        *snapshot = EventsSnapshot {
            now_ms: value,
            fault: self.fault,
            healthy: read.is_some()
                && self.fault == EventsFault::Ok
                && !b.faulted
                && b.motor.fault == MotorGateFault::Ok,
            timer_ready: self.timer_ready(value),
            owner_ready: self.owner_ready(value),
            stopped: self.stopped(),
            optics_enabled: b.optics_enabled,
            optics_off: board::optics_off(),
            capture_enabled: self.capture_enabled,
        };

        read.is_some()
    }

    fn optics_select(&mut self, a: bool, b: bool, max_on_ms: u32) -> bool {
        let enabled = a || b;
        if !self.opened || (enabled && !interval_valid(max_on_ms)) {
            return false;
        }

        let Some(value) = self.now() else {
            return false;
        };

        if enabled
            && (self.fault != EventsFault::Ok
                || !self.timer_ready(value)
                || !self.owner_ready(value))
        {
            return false;
        }

        if enabled && self.board().view().optics_enabled {
            return board::optics_are(a, b);
        }
        if !self.board().optics_select(a, b) {
            return self.fail(EventsFault::Output);
        }

        self.optics_armed = enabled;
        self.optics_at = value;
        self.optics_max_ms = max_on_ms;
        true
    }

    fn capture(&mut self, enabled: bool) -> bool {
        if !self.opened {
            return false;
        }
        let stopped = self.stopped();
        let read = self.now();

        if !enabled {
            sdk::gpio_int_config(FEEDBACK_PIN, false);
            self.capture_enabled = false;
            if !stopped {
                return self.fail(EventsFault::Feedback);
            }
            if let Some(value) = read
                && self.fault == EventsFault::Ok
                && self.feedback.fault == FeedbackFault::Ok
            {
                // Discard completed/cancelled-request edges only while stopped.
                // Never erase a latched fault to prepare a later request.
                self.reset_feedback(value);
            }
            return read.is_some();
        }

        let Some(value) = read else { return false };
        if !stopped || self.fault != EventsFault::Ok || !self.timer_ready(value) {
            return false;
        }

        self.reset_feedback(value);
        self.edges = FeedbackEdges::default();
        sdk::gpioa_eoi(FEEDBACK_MASK);
        sdk::gpio_int_config(FEEDBACK_PIN, true);
        self.capture_enabled = sdk::gpioa_int_en() & FEEDBACK_MASK != 0
            && sdk::gpioa_int_mask() & FEEDBACK_MASK == 0
            && sdk::gpioa_int_bothedge() & FEEDBACK_MASK != 0;
        self.capture_enabled || self.fail(EventsFault::Setup)
    }

    fn reset_feedback(&mut self, value: u32) {
        if let Some(fresh) =
            FeedbackEvents::new(self.feedback.capacity, self.feedback.max_age_ms, value)
        {
            self.feedback = fresh;
        }
    }

    fn process(
        &mut self,
        controller: &mut FeedController,
        sample: Option<&FeedSample>,
        inputs: Option<&SensorInputs>,
    ) -> bool {
        if !self.opened {
            return false;
        }
        let read = self.now().filter(|_| self.fault == EventsFault::Ok);
        let mut result = read.is_some();
        if let Some(value) = read
            && self.board().view().optics_enabled
            && !self.owner_ready(value)
        {
            result = self.fail(EventsFault::Owner);
        }

        let Some(value) = read.filter(|_| result) else {
            controller.trip(if self.fault == EventsFault::Feedback {
                FeedReason::FeedbackStream
            } else {
                FeedReason::Output
            });
            return false;
        };

        let composed = inputs.map(|inputs| inputs.sample(value, false, false));
        let b = self.board().view();
        let drive = FeedbackDrive {
            healthy: !b.faulted && b.motor.fault == MotorGateFault::Ok,
            applied_known: b.motor.applied_known,
            applied: b.motor.applied,
            applied_at: b.motor.applied_at,
        };

        let ok = match composed.as_ref().or(sample) {
            Some(sample) => self.feedback.process(controller, value, sample, drive),
            None => {
                // A missing sample fails the stream exactly like the C ABI.
                self.feedback.fail(FeedbackFault::Argument);
                controller.trip(FeedReason::FeedbackStream);
                false
            }
        };

        if !ok {
            self.fail(if self.feedback.fault == FeedbackFault::Ok {
                EventsFault::Output
            } else {
                EventsFault::Feedback
            });
        } else {
            self.owner_at = value;
            self.owner_seen = true;
        }

        ok
    }

    fn start(
        &mut self,
        controller: &mut FeedController,
        inputs: &SensorInputs,
        request: &FeedRequest,
        latest_start_ms: u32,
    ) -> FeedStartResult {
        if !self.opened {
            return FeedStartResult::RejectRequest;
        }
        let Some(value) = self.now() else {
            return FeedStartResult::RejectInput;
        };

        let remaining = latest_start_ms.wrapping_sub(value);
        if self.fault != EventsFault::Ok
            || !self.timer_ready(value)
            || !self.owner_ready(value)
            || !interval_valid(remaining)
            || self.feedback.max_age_ms >= controller.limits.feedback_inhibit_ms
        {
            return FeedStartResult::RejectInput;
        }

        let sample = inputs.sample(value, false, false);
        let result = controller.start(request, value, &sample);
        if result == FeedStartResult::Accepted {
            let motor = controller.motor;
            let ready = self.capture(true) && self.board().motor(motor, value);
            if !ready {
                self.fail(EventsFault::Output);
                controller.trip(FeedReason::Output);
                return FeedStartResult::RejectInput;
            }
        }
        result
    }

    fn motor(&mut self, command: MotorCommand) -> bool {
        if !self.opened {
            return false;
        }
        let Some(value) = self.now() else {
            return false;
        };

        if command != MotorCommand::Stop {
            if self.fault != EventsFault::Ok || !self.capture_enabled || !self.timer_ready(value) {
                return self.fail(EventsFault::Timer);
            }
            if !self.owner_ready(value) {
                return self.fail(EventsFault::Owner);
            }
        }

        self.board().motor(command, value) || self.fail(EventsFault::Output)
    }

    fn close(&mut self) -> bool {
        if !self.opened {
            return false;
        }
        let millis = self.clock.millis;
        self.board().motor(MotorCommand::Stop, millis);
        sdk::gpio_int_config(FEEDBACK_PIN, false);
        self.capture_enabled = false;
        let optics_off = self.board().optics(false) && board::optics_off();
        self.optics_armed = false;

        if !self.stopped_applied() || !optics_off {
            self.fail(EventsFault::Output);
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
        let gate = self.board().view().motor;
        gate.applied_known && gate.applied == MotorCommand::Stop
    }

    fn timer_tick(&mut self) {
        if !self.opened {
            return;
        }

        let raw = sdk::systimer_ticks();
        if raw == self.timer_raw_at {
            if !self.timer_raw_repeat {
                // A pending update taken at unmask can coincide with the next one
                // within a SYSTIMER tick: skip once without advancing anything.
                self.timer_raw_repeat = true;
                return;
            }
            self.fail(EventsFault::Clock);
            return;
        }

        self.timer_raw_repeat = false;
        let Some(value) = self.clock.read(raw) else {
            self.fail(EventsFault::Clock);
            return;
        };
        self.timer_raw_at = raw;
        self.timer_count = self.timer_count.wrapping_add(1);
        self.timer_at = value;
        self.timer_seen = true;

        if self.fault != EventsFault::Ok {
            let fault = self.fault;
            self.fail(fault); // Retry unverified GPIO stops.
            return;
        }
        if !self.board().motor_tick(value) {
            self.fail(EventsFault::Output);
            return;
        }
        if self.capture_enabled {
            self.edges.pins.sample([
                sdk::gpio_port_read(0, u32::MAX),
                sdk::gpio_port_read(1, u32::MAX),
            ]);
        }

        // Read after the tick, which a flash hold can use to turn them off.
        if !self.board().view().optics_enabled {
            return;
        }
        if !self.owner_ready(value) {
            self.fail(EventsFault::Owner);
        } else if !self.optics_armed || value.wrapping_sub(self.optics_at) >= self.optics_max_ms {
            self.fail(EventsFault::Optics);
        }
    }

    fn feedback_edge(&mut self, pending: u32) {
        if !self.opened {
            return;
        }

        if pending & !FEEDBACK_MASK != 0 {
            self.fail(EventsFault::Resource);
            sdk::irq_disable(Irq::GpioA);
        } else if pending & FEEDBACK_MASK != 0
            && self.capture_enabled
            && let Some(value) = self.now()
        {
            let high = sdk::gpioa_ext_port() & FEEDBACK_MASK != 0;
            let edges = &mut self.edges;
            if high {
                edges.rising = edges.rising.saturating_add(1);
            } else {
                edges.falling = edges.falling.saturating_add(1);
            }
            edges.first_at.get_or_insert(value);
            if let Some(slot) = edges.log.iter_mut().find(|slot| slot.is_none()) {
                *slot = Some((value, high));
            }
            if !self.feedback.push(value, high) {
                self.fail(EventsFault::Feedback);
            }
        }
    }
}

impl Events {
    /// The state, inside the guard; the borrow ends with `f`. The state's
    /// methods reach the board, never this handle, so `f` cannot re-enter.
    fn with<R>(&self, f: impl FnOnce(&mut EventsState) -> R) -> R {
        let _guard = IrqGuard::lock();
        // SAFETY: inside the guard, on the single KM4 core, no task or
        // interrupt can reach the state, and no method lets a borrow of it
        // outlive its call.
        f(unsafe { &mut *self.state.get() })
    }

    /// A copy of the state, for status and tests.
    pub fn view(&self) -> EventsState {
        self.with(|state| *state)
    }

    /// The feedback edges since capture was last enabled.
    pub fn edges(&self) -> FeedbackEdges {
        self.with(|state| state.edges)
    }

    /// Whether `open` succeeded and `close` has not.
    pub fn opened(&self) -> bool {
        self.with(|state| state.opened)
    }

    /// Requires an opened, stopped board. Refuses enabled GPIOA interrupts or
    /// a running TIM2 before claiming them.
    ///
    /// # Safety
    /// `board` is null or a live board; the caller retains it and this object
    /// at fixed addresses until close succeeds.
    pub unsafe fn open(&self, board: *const Board, config: &EventsConfig) -> bool {
        let this = ptr::from_ref(self);
        // SAFETY: the caller's contract, passed on.
        self.with(|state| unsafe { state.open(this, board, config) })
    }

    /// Read at acquisition START; compose health using another read later.
    pub fn now_ms(&self) -> Option<u32> {
        self.with(EventsState::now_ms)
    }

    /// Fill `snapshot` even when the clock read fails; returns the read result.
    pub fn snapshot(&self, snapshot: &mut EventsSnapshot) -> bool {
        self.with(|state| state.snapshot(snapshot))
    }

    /// Arm a non-renewable maximum on-time. Repeated enables do not extend it.
    /// Only successful owner-side sample processing renews the liveness lease.
    pub fn optics(&self, enabled: bool, max_on_ms: u32) -> bool {
        self.optics_select(enabled, enabled, max_on_ms)
    }

    /// As `optics`, with PA15 to `a` and PB3 to `b`. While on, a repeated
    /// enable neither extends the budget nor changes the selection: it
    /// returns whether the emitters already match it.
    pub fn optics_select(&self, a: bool, b: bool, max_on_ms: u32) -> bool {
        self.with(|state| state.optics_select(a, b, max_on_ms))
    }

    /// Enable only while verified stopped and after observing a timer
    /// interrupt. Disabling capture while driving trips STOP.
    pub fn capture(&self, enabled: bool) -> bool {
        self.with(|state| state.capture(enabled))
    }

    /// Owner-side processing under one short lock; never acquires ADC here.
    pub fn process(&self, controller: &mut FeedController, sample: Option<&FeedSample>) -> bool {
        self.with(|state| state.process(controller, sample, None))
    }

    /// Compose freshness at the exact processing clock read, inside serialization.
    pub fn observe(&self, controller: &mut FeedController, inputs: &SensorInputs) -> bool {
        self.with(|state| state.process(controller, None, Some(inputs)))
    }

    /// Reserve the request durably BEFORE calling. Freshness, capture and
    /// initial output application are serialized. A rejected input never
    /// drives; an output failure can leave an accepted controller faulted.
    pub fn start(
        &self,
        controller: &mut FeedController,
        inputs: &SensorInputs,
        request: &FeedRequest,
        latest_start_ms: u32,
    ) -> FeedStartResult {
        self.with(|state| state.start(controller, inputs, request, latest_start_ms))
    }

    /// Drive requires a recently observed timer and enabled healthy capture.
    #[must_use]
    pub fn motor(&self, command: MotorCommand) -> bool {
        self.with(|state| state.motor(command))
    }

    /// If STOP or infrared OFF cannot be verified, returns false and retains
    /// the retry timer; supervision is never removed from an active output.
    pub fn close(&self) -> bool {
        self.with(EventsState::close)
    }
}

unsafe extern "C" fn timer_interrupt(_: *mut c_void) -> u32 {
    let _guard = IrqGuard::lock();
    sdk::tim2_int_clear();
    // SAFETY: `active` is the registered events handle, which stays in
    // place until `close` clears it; it is reached only through `&`.
    if let Some(events) = unsafe { active().as_ref() } {
        events.with(EventsState::timer_tick);
    }
    0
}

unsafe extern "C" fn feedback_interrupt(_: *mut c_void) -> u32 {
    let _guard = IrqGuard::lock();
    let pending = sdk::gpioa_int_status();
    sdk::gpioa_eoi(pending);
    // SAFETY: as for `timer_interrupt`.
    if let Some(events) = unsafe { active().as_ref() } {
        events.with(|state| state.feedback_edge(pending));
    }
    0
}
