//! Experimental stock-derived PLAF103 pin map: motor PA13/PA14, optics
//! emitters 15/35, inputs PA16 outlet, PA17 food, PA18 feedback, PA19 DC, ADC
//! channels 4 (current) and 5 (battery). Not a physically verified board port.
//!
//! These functions TOUCH GPIO/ADC when executed. The diagnostic never calls
//! them. Verify the board, startup/output behavior and recovery first.
//!
//! [`Board`] is a shared handle: the owning task and the supervision timer
//! interrupt both reach it through `&Board`, and its state is borrowed only
//! inside `IrqGuard` for the length of one call, so no borrow of it ever
//! spans a window in which an interrupt can run, and a call nested inside
//! another is refused rather than lent the state twice. The ADC conversions are the
//! one exception to the mask: they poll with interrupts enabled and touch no
//! board state while they do.

use core::cell::{Cell, UnsafeCell};
use core::ptr;

use wellfed_core::HALF_RANGE;
use wellfed_core::adc_scale::{AdcCalibration, SAMPLES};
use wellfed_core::feed::controller::MotorCommand;
use wellfed_core::feed::motor_gate::{MotorGate, MotorGateFault, MotorPins};
use wellfed_core::feed::sensor_inputs::SensorReading;
use wellfed_core::flash_coord::FlashCoord;

use crate::adc;
use crate::plaf103::pins::{
    ADAPTER, ADC_BATTERY, ADC_CURRENT, ADC_SUPPLY, FEEDBACK, FOOD, IR_PA, IR_PB, MOTOR_A_BIT,
    MOTOR_B_BIT, OUTLET, bit,
};
use crate::sdk::{self, GPIO_PORT_A, IrqGuard};

pub use crate::plaf103::pins::MOTOR_MASK;
const SENSOR_MASK: u32 = bit(OUTLET) | bit(FOOD) | bit(ADAPTER);
const OPTICS: [u32; 2] = [IR_PA, IR_PB];

/// Polls per conversion accepted by `open` and `open_sensors`.
pub const MAX_ADC_POLLS: u32 = 1_000_000;

/// What `Board::open_sensors` did, for diagnostics.
#[derive(Clone, Copy, Debug, Default)]
pub struct SensorsOpen {
    /// The LP ADC enable, with the registers before and after.
    pub clock: sdk::LpClock,
    /// Efuse calibration bytes 0x1d0..0x1d3, when all four read.
    pub efuse: Option<[u8; 4]>,
    pub ok: bool,
}

/// The board's state, as [`Board::view`] copies it out.
#[derive(Clone, Copy, Debug)]
pub struct BoardState {
    pub motor: MotorGate<BoardPins>,
    pub calibration: AdcCalibration,
    pub adc_poll_limit: u32,
    pub opened: bool,
    pub faulted: bool,
    pub optics_enabled: bool,
    pub optics_initialized: bool,
    pub flash: *mut FlashCoord,
    /// Sensor inputs, calibration and ADC initialized by `open`.
    pub sensors_ready: bool,
}

impl Default for BoardState {
    fn default() -> Self {
        Self {
            motor: MotorGate::default(),
            calibration: AdcCalibration::default(),
            adc_poll_limit: 0,
            opened: false,
            faulted: false,
            optics_enabled: false,
            optics_initialized: false,
            flash: ptr::null_mut(),
            sensors_ready: false,
        }
    }
}

/// The board handle; see the module documentation. Keep it at a fixed
/// address while an `Events` supervisor holds it.
#[derive(Debug, Default)]
pub struct Board {
    state: UnsafeCell<BoardState>,
    /// Set while a call holds the state; see `Board::with`.
    entered: Cell<bool>,
}

/// A null or not-yet-attached authority blocks every output. Callers pass an
/// attached authority, which `Board::attach_flash` requires to stay live.
pub(crate) fn outputs_blocked(flash: *const FlashCoord) -> bool {
    // SAFETY: the attached authority stays at a fixed address while in use.
    unsafe { flash.as_ref() }.is_none_or(FlashCoord::outputs_blocked)
}

/// Output latch and pad input of one emitter both at `level`. A LOW input
/// alone cannot prove OFF: a pad whose input path is dead also reads LOW.
fn optic_is(pin: u32, level: bool) -> bool {
    let latch = sdk::gpio_output_latch(pin >> 5) & (1 << (pin & 31)) != 0;
    latch == level && sdk::gpio_read_bit(pin) == level
}

/// Both emitters OFF, latch and pad input each.
pub fn optics_off() -> bool {
    OPTICS.iter().all(|&pin| optic_is(pin, false))
}

/// PA15 at `a` and PB3 at `b`, latch and pad input each.
pub fn optics_are(a: bool, b: bool) -> bool {
    optic_is(OPTICS[0], a) && optic_is(OPTICS[1], b)
}

/// The board's motor pins. They hold no pointer back to the board: before
/// every gate call the board says whether a drive is permitted, and after
/// it reads whether a write failed, so the gate's borrow never overlaps
/// another borrow of the board.
#[derive(Clone, Copy, Debug, Default)]
pub struct BoardPins {
    drive_allowed: bool,
    write_failed: bool,
}

impl MotorPins for BoardPins {
    fn write_pair(&mut self, a: bool, b: bool) -> bool {
        let stopped = a && b;
        if (!a && !b) || (!stopped && !self.drive_allowed) {
            return false;
        }

        let _guard = IrqGuard::lock();
        let value = if a { MOTOR_A_BIT } else { 0 } | if b { MOTOR_B_BIT } else { 0 };
        // SAFETY: the motor pins, which the board owns; the gate makes this
        // its only writer while the board is open.
        unsafe { sdk::gpio_port_write(GPIO_PORT_A, MOTOR_MASK, value) };
        let verified = sdk::gpio_port_read(GPIO_PORT_A, MOTOR_MASK) & MOTOR_MASK == value;
        if !verified {
            // The board faults after this call; within it, no later write drives.
            self.write_failed = true;
            self.drive_allowed = false;
        }
        verified
    }
}

impl BoardState {
    /// A gate call with the pins told whether a drive is permitted, and a
    /// failed write latched as the board's fault afterwards.
    fn gate<R>(&mut self, f: impl FnOnce(&mut MotorGate<BoardPins>) -> R) -> R {
        let allowed = self.opened && !self.faulted && !outputs_blocked(self.flash);
        if let Some(pins) = self.motor.pins.as_mut() {
            pins.drive_allowed = allowed;
        }

        let result = f(&mut self.motor);

        if let Some(pins) = self.motor.pins.as_mut()
            && core::mem::take(&mut pins.write_failed)
        {
            self.faulted = true;
        }
        result
    }

    fn open_outputs(&mut self, deadtime_ms: u32, lease_ms: u32, now_ms: u32) -> bool {
        if deadtime_ms == 0 || lease_ms <= deadtime_ms || lease_ms >= HALF_RANGE {
            return false;
        }

        *self = Self::default();
        if !sdk::gpio_clock_enable().verified() {
            self.faulted = true;
            return false;
        }

        crate::stopped::enable_motor_stopped();
        let motor_ok =
            self.gate(|gate| gate.init(BoardPins::default(), deadtime_ms, lease_ms, now_ms));

        // PB3 is SWD_CLK: release it first, as stock does, or the latch
        // never reaches the pad.
        let swd_released = sdk::swd_off() & sdk::SWD_PMUX_BIT == 0;
        for pin in OPTICS {
            // SAFETY: an emitter latch, which the board owns.
            unsafe { sdk::gpio_write_bit(pin, false) };
        }
        for pin in OPTICS {
            // SAFETY: as above, with the latch already LOW.
            unsafe { sdk::gpio_init(pin, true) };
        }

        self.optics_initialized = true;
        if !motor_ok || !swd_released || !optics_off() {
            self.faulted = true;
            return false;
        }

        self.opened = true;
        true
    }

    fn trip(&mut self, now_ms: u32) {
        self.faulted = true;
        if self.motor.fault == MotorGateFault::Ok {
            self.gate(|gate| gate.trip(now_ms));
        } else {
            self.gate(|gate| gate.request(MotorCommand::Stop, now_ms));
        }
    }

    fn trip_now(&mut self) {
        self.trip(self.motor.observed_at);
    }

    fn inactive(&self) -> bool {
        self.optics_initialized
            && self.motor.applied_known
            && self.motor.applied == MotorCommand::Stop
            && self.motor.desired == MotorCommand::Stop
            && sdk::gpio_port_read(GPIO_PORT_A, MOTOR_MASK) == MOTOR_MASK
            && optics_off()
    }

    fn flash_quiesce(&mut self) -> bool {
        if !self.optics_initialized || self.motor.pins.is_none() {
            return false;
        }
        if !self.inactive() {
            self.trip_now(); // Never auto-resume interrupted feeding.
        } else {
            let observed = self.motor.observed_at;
            self.gate(|gate| gate.request(MotorCommand::Stop, observed));
        }
        self.optics_select(false, false);
        self.inactive()
    }

    fn flash_hold(&mut self) -> bool {
        if !outputs_blocked(self.flash) {
            return false;
        }
        self.flash_quiesce();
        true
    }

    fn motor(&mut self, command: MotorCommand, now_ms: u32) -> bool {
        if !self.opened || (self.faulted && command != MotorCommand::Stop) {
            return false;
        }
        if command != MotorCommand::Stop && self.flash_hold() {
            return false;
        }
        self.gate(|gate| gate.request(command, now_ms))
    }

    fn motor_tick(&mut self, now_ms: u32) -> bool {
        if !self.opened {
            return false;
        }
        if (self.motor.desired != MotorCommand::Stop || self.optics_enabled) && self.flash_hold() {
            return false;
        }
        self.gate(|gate| gate.tick(now_ms))
    }

    fn optics_select(&mut self, a: bool, b: bool) -> bool {
        let enabled = a || b;
        if !self.optics_initialized || (enabled && (!self.opened || self.faulted)) {
            return false;
        }
        if enabled && self.flash_hold() {
            return false;
        }

        for (pin, level) in OPTICS.into_iter().zip([a, b]) {
            // SAFETY: an emitter latch, which the board owns.
            unsafe { sdk::gpio_write_bit(pin, level) };
        }

        let verified = optics_are(a, b);
        self.optics_enabled = verified && enabled;
        if !verified {
            for pin in OPTICS {
                // SAFETY: an emitter latch, which the board owns.
                unsafe { sdk::gpio_write_bit(pin, false) };
            }
            self.trip_now();
        }
        verified
    }

    fn capture_gpio(&self, reading: &mut SensorReading) -> Option<u32> {
        *reading = SensorReading::default();
        if !self.opened || self.faulted || !self.sensors_ready {
            return None;
        }
        let pins = sdk::gpio_port_read(GPIO_PORT_A, SENSOR_MASK | bit(FEEDBACK));
        reading.gpio_valid = true;
        reading.food_high = pins & bit(FOOD) != 0;
        reading.outlet_high = pins & bit(OUTLET) != 0;
        reading.dc_present = pins & bit(ADAPTER) != 0;
        reading.optics_enabled = self.optics_enabled && optics_are(true, true);
        Some(pins)
    }
}

/// A board call made while another call on the same board held its state:
/// refused rather than handed a second `&mut` to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reentered;

impl Board {
    /// The state, inside the guard, for the length of `f`. A call from
    /// inside `f` (from the closure, or from code it calls) is refused with
    /// [`Reentered`]: the state is already borrowed, so lending it again
    /// would alias it, and waiting for it would never end on one core.
    fn with<R>(&self, f: impl FnOnce(&mut BoardState) -> R) -> Result<R, Reentered> {
        let _guard = IrqGuard::lock();
        if self.entered.replace(true) {
            return Err(Reentered);
        }

        // SAFETY: inside the guard, on the single KM4 core, no task or
        // interrupt can run, and `entered` refuses any call nested in `f`,
        // so this is the only reference to the state until `f` returns.
        let result = f(unsafe { &mut *self.state.get() });

        self.entered.set(false);
        Ok(result)
    }

    /// A copy of the state, for status and tests. A reentered call sees
    /// a closed, faulted board, which permits nothing.
    pub fn view(&self) -> BoardState {
        self.with(|state| *state).unwrap_or(BoardState {
            faulted: true,
            ..BoardState::default()
        })
    }

    /// Keep the board at this address until closed. Invalid arguments leave
    /// the board and the hardware untouched.
    pub fn open(&self, deadtime_ms: u32, lease_ms: u32, adc_poll_limit: u32, now_ms: u32) -> bool {
        if adc_poll_limit == 0 || adc_poll_limit > MAX_ADC_POLLS {
            return false;
        }
        self.open_outputs(deadtime_ms, lease_ms, now_ms) && self.open_sensors(adc_poll_limit).ok
    }

    /// After `open_outputs`: sensor inputs, the explicit LP ADC clock and
    /// function enable with readback, efuse calibration and the ADC. A
    /// failure after the outputs opened faults the board.
    pub fn open_sensors(&self, adc_poll_limit: u32) -> SensorsOpen {
        let mut result = SensorsOpen::default();
        let started = self.with(|state| {
            if !state.opened
                || state.sensors_ready
                || adc_poll_limit == 0
                || adc_poll_limit > MAX_ADC_POLLS
            {
                return false;
            }
            state.opened = false;
            state.adc_poll_limit = adc_poll_limit;
            true
        });
        if started != Ok(true) {
            return result;
        }

        for pin in [OUTLET, FOOD, FEEDBACK, ADAPTER] {
            // SAFETY: a sensor input, which the board owns.
            unsafe { sdk::gpio_init(pin, false) };
        }

        // Unmasked: the ADC and efuse reads poll, and the board is closed
        // (`opened` false) meanwhile.
        let opened = adc::open();
        result.clock = opened.clock;
        result.efuse = opened.efuse;

        // Nothing borrows the state between the two calls, so this one
        // cannot be refused; were it, the board would stay closed.
        let _ = self.with(|state| {
            let Some(calibration) = opened.calibration else {
                state.faulted = true;
                return;
            };
            state.calibration = calibration;
            state.sensors_ready = true;
            state.opened = true;
            result.ok = true;
        });
        result
    }

    /// Motor STOP and emitters OFF only: no sensor inputs, efuse or ADC, so
    /// `capture` refuses. For supervision checks with motion disabled.
    pub fn open_outputs(&self, deadtime_ms: u32, lease_ms: u32, now_ms: u32) -> bool {
        self.with(|state| state.open_outputs(deadtime_ms, lease_ms, now_ms))
            .unwrap_or(false)
    }

    /// Bounded GPIO stop attempt, including from an independent ISR. A
    /// reentered trip is dropped: the call holding the state is the one
    /// writing the pins, and it latches its own failures.
    pub fn trip(&self, now_ms: u32) {
        let _ = self.with(|state| state.trip(now_ms));
    }

    pub(crate) fn inactive(&self) -> bool {
        self.with(|state| state.inactive()).unwrap_or(false)
    }

    /// Mandatory before any drive or emitter enable; attach only while
    /// inactive.
    ///
    /// # Safety
    /// `flash` is null or a live authority that stays at this address, under
    /// the SAME IRQ serialization as this board, for the board's lifetime.
    pub unsafe fn attach_flash(&self, flash: *mut FlashCoord) -> bool {
        // SAFETY: caller supplies null or a live authority.
        let initialized = unsafe { flash.as_ref() }.is_some_and(|f| f.initialized);
        self.with(|state| {
            let ok = initialized
                && state.opened
                && !state.faulted
                && (state.flash.is_null() || state.flash == flash)
                && state.inactive();
            if ok {
                state.flash = flash;
            }
            ok
        })
        .unwrap_or(false)
    }

    /// Stops/inhibits motion for a flash lease. Interrupted output latches the
    /// board fault; success establishes GPIO logic only, not motor mechanics.
    pub fn flash_quiesce(&self) -> bool {
        self.with(BoardState::flash_quiesce).unwrap_or(false)
    }

    /// Critical sections serialize gate calls between task and timer ISR.
    pub fn motor(&self, command: MotorCommand, now_ms: u32) -> bool {
        self.with(|state| state.motor(command, now_ms))
            .unwrap_or(false)
    }

    pub fn motor_tick(&self, now_ms: u32) -> bool {
        self.with(|state| state.motor_tick(now_ms)).unwrap_or(false)
    }

    pub fn optics(&self, enabled: bool) -> bool {
        self.optics_select(enabled, enabled)
    }

    /// PA15 to `a` and PB3 to `b`, with readback. Either one on counts as
    /// enabled; a failed change attempts both off and trips the motor gate.
    pub fn optics_select(&self, a: bool, b: bool) -> bool {
        self.with(|state| state.optics_select(a, b))
            .unwrap_or(false)
    }

    /// 16 bounded conversions of `channel`: 4 (current), 5 (battery) or 10
    /// (the chip's internal supply measurement). Exclusive ADC ownership required.
    pub fn sample_channel(&self, channel: u32, samples: &mut [u16; SAMPLES]) -> bool {
        let known = [ADC_CURRENT, ADC_BATTERY, ADC_SUPPLY].contains(&channel);
        let limit = self.with(|state| {
            (state.opened && !state.faulted && state.sensors_ready).then_some(state.adc_poll_limit)
        });
        let Some(limit) = limit.ok().flatten().filter(|_| known) else {
            return false;
        };

        let ok = adc::convert(channel, limit, samples);
        if !ok {
            let _ = self.with(BoardState::trip_now);
        }
        ok
    }

    /// Exclusive ADC ownership required. Polls are bounded per conversion,
    /// with interrupts enabled. Timestamp at acquisition START, so its
    /// execution time counts toward age.
    pub fn capture(&self, reading: &mut SensorReading) -> bool {
        let limit = self.with(|state| {
            state.capture_gpio(reading)?;
            Some(state.adc_poll_limit)
        });
        let Ok(Some(limit)) = limit else {
            return false;
        };

        reading.current_adc_valid = adc::convert(ADC_CURRENT, limit, &mut reading.current);
        if reading.current_adc_valid {
            reading.battery_adc_valid = adc::convert(ADC_BATTERY, limit, &mut reading.battery);
        }
        if !reading.current_adc_valid || !reading.battery_adc_valid {
            let _ = self.with(BoardState::trip_now);
            return false;
        }
        true
    }

    /// GPIO only: PA16/PA17/PA19 and the emitter state, both ADC channels
    /// marked invalid. Returns the raw PA16..PA19 inputs, PA18 feedback
    /// included, read in the same port access.
    pub fn capture_gpio(&self, reading: &mut SensorReading) -> Option<u32> {
        self.with(|state| state.capture_gpio(reading))
            .ok()
            .flatten()
    }

    pub fn close(&self) {
        let sensors = self.with(|state| {
            if !state.opened {
                return false;
            }
            state.trip_now();
            state.optics_select(false, false);
            state.opened = false;
            state.sensors_ready
        });
        if sensors == Ok(true) {
            adc::close();
        }
    }
}

#[cfg(test)]
mod tests {
    use wellfed_core::flash_coord::FlashReply;

    use super::*;
    use crate::sdk::fake;
    use crate::supervision::config::FLASH_LIMITS;

    #[test]
    fn a_nested_call_is_refused_and_the_board_stays_usable() {
        let board = Board::default();

        let nested = board.with(|_| board.with(|state| state.opened));
        assert_eq!(nested, Ok(Err(Reentered)));
        assert!(board.with(|_| board.view()).is_ok_and(|view| view.faulted));

        assert_eq!(board.with(|state| state.faulted), Ok(false));
    }

    #[test]
    fn nested_outputs_are_refused_and_change_nothing() {
        fake::reset_hardware();
        let board = Board::default();
        assert!(board.open(3, 10, 8, 0));
        let mut flash = FlashCoord::new(&FLASH_LIMITS, 17, 0).expect("valid limits");
        assert_eq!(flash.ready(17, 29, true, 0), FlashReply::Accepted);
        // SAFETY: `flash` outlives the board's use of it and stays in place.
        assert!(unsafe { board.attach_flash(&mut flash) });
        let before = board.view();

        let nested = board.with(|_| {
            (
                board.motor(MotorCommand::Drive0, 1),
                board.optics_select(true, true),
                board.motor_tick(2),
            )
        });

        assert_eq!(nested, Ok((false, false, false)));
        let after = board.view();
        assert_eq!(after.motor.desired, before.motor.desired);
        assert_eq!(after.motor.applied, before.motor.applied);
        assert!(!after.faulted && !after.optics_enabled && after.opened);
        assert_eq!(sdk::gpio_port_read(GPIO_PORT_A, MOTOR_MASK), MOTOR_MASK);
        assert!(optics_off());

        assert!(board.motor(MotorCommand::Drive0, 3), "usable once unnested");
        board.close();
    }
}
