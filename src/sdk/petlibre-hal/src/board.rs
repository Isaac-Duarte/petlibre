//! Experimental stock-derived PLAF103 pin map: motor PA13/PA14, optics
//! emitters 15/35, inputs PA16 outlet, PA17 food, PA18 feedback, PA19 DC, ADC
//! channels 4 (current) and 5 (battery). Not a physically verified board port.
//!
//! These functions TOUCH GPIO/ADC when executed. The diagnostic never calls
//! them. Verify the board, startup/output behavior and recovery first.

use core::ffi::c_void;
use core::ptr::{self, addr_of};

use petlibre_core::HALF_RANGE;
use petlibre_core::adc_scale::{AdcCalibration, SAMPLES};
use petlibre_core::feed::controller::MotorCommand;
use petlibre_core::feed::motor_gate::{MotorGate, MotorGateFault};
use petlibre_core::feed::sensor_inputs::SensorReading;
use petlibre_core::flash_coord::FlashCoord;

use crate::adc;
use crate::sdk::board::{
    ADAPTER, ADC_BATTERY, ADC_CURRENT, ADC_SUPPLY, FEEDBACK, FOOD, IR_PA, IR_PB, MOTOR_A, MOTOR_B,
    OUTLET, bit,
};
use crate::sdk::{self, GPIO_PORT_A, IrqGuard};

pub use crate::sdk::MOTOR_MASK;
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

/// Keep this object at a fixed address while opened; its gate refers back to it.
#[repr(C)]
#[derive(Debug)]
pub struct Board {
    pub motor: MotorGate,
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

impl Default for Board {
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

/// Motor pins callback. Reads board fields through the raw context pointer so
/// it never aliases the gate borrow that invokes it.
unsafe extern "C" fn motor_pair(context: *mut c_void, a: bool, b: bool) -> bool {
    let board = context.cast::<Board>().cast_const();
    let stopped = a && b;
    let guard = IrqGuard::lock();

    // SAFETY: `context` is the opened board, serialized by the guard.
    let (opened, faulted, flash) = unsafe {
        (
            addr_of!((*board).opened).read(),
            addr_of!((*board).faulted).read(),
            addr_of!((*board).flash).read(),
        )
    };
    if (!a && !b) || (!stopped && (!opened || faulted || outputs_blocked(flash))) {
        return false;
    }

    let value = if a { sdk::MOTOR_A_BIT } else { 0 } | if b { sdk::MOTOR_B_BIT } else { 0 };
    sdk::gpio_port_write(GPIO_PORT_A, MOTOR_MASK, value);
    let verified = sdk::gpio_port_read(GPIO_PORT_A, MOTOR_MASK) & MOTOR_MASK == value;
    drop(guard);

    if !verified {
        // SAFETY: as above; a single bool store.
        unsafe { ptr::addr_of_mut!((*context.cast::<Board>()).faulted).write(true) };
    }
    verified
}

impl Board {
    /// Keep the board at this address until closed. Invalid arguments leave
    /// `self` and the hardware untouched.
    pub fn open(
        &mut self,
        deadtime_ms: u32,
        lease_ms: u32,
        adc_poll_limit: u32,
        now_ms: u32,
    ) -> bool {
        if adc_poll_limit == 0 || adc_poll_limit > MAX_ADC_POLLS {
            return false;
        }
        self.open_outputs(deadtime_ms, lease_ms, now_ms) && self.open_sensors(adc_poll_limit).ok
    }

    /// After `open_outputs`: sensor inputs, the explicit LP ADC clock and
    /// function enable with readback, efuse calibration and the ADC. A
    /// failure after the outputs opened faults the board.
    pub fn open_sensors(&mut self, adc_poll_limit: u32) -> SensorsOpen {
        let mut result = SensorsOpen::default();
        if !self.opened
            || self.sensors_ready
            || adc_poll_limit == 0
            || adc_poll_limit > MAX_ADC_POLLS
        {
            return result;
        }

        self.opened = false;
        self.adc_poll_limit = adc_poll_limit;

        for pin in [OUTLET, FOOD, FEEDBACK, ADAPTER] {
            sdk::gpio_init(pin, false);
        }

        let opened = adc::open();
        result.clock = opened.clock;
        result.efuse = opened.efuse;
        let Some(calibration) = opened.calibration else {
            self.faulted = true;
            return result;
        };

        self.calibration = calibration;
        self.sensors_ready = true;
        self.opened = true;
        result.ok = true;
        result
    }

    /// Motor STOP and emitters OFF only: no sensor inputs, efuse or ADC, so
    /// `capture` refuses. For supervision checks with motion disabled.
    pub fn open_outputs(&mut self, deadtime_ms: u32, lease_ms: u32, now_ms: u32) -> bool {
        if deadtime_ms == 0 || lease_ms <= deadtime_ms || lease_ms >= HALF_RANGE {
            return false;
        }

        *self = Self::default();
        if !sdk::gpio_clock_enable().verified() {
            self.faulted = true;
            return false;
        }

        // Preload before output enable; actual reset/ROM-init behavior needs measurement.
        sdk::gpio_port_write(GPIO_PORT_A, MOTOR_MASK, MOTOR_MASK);
        sdk::gpio_init(MOTOR_A, true);
        sdk::gpio_init(MOTOR_B, true);
        let context = ptr::from_mut(self).cast();
        // SAFETY: the callback reads this fixed-address board while opened.
        let motor_ok = unsafe {
            self.motor
                .init(Some(motor_pair), context, deadtime_ms, lease_ms, now_ms)
        };

        // PB3 is SWD_CLK: release it first, as stock does, or the latch
        // never reaches the pad.
        let swd_released = sdk::swd_off() & sdk::SWD_PMUX_BIT == 0;
        for pin in OPTICS {
            sdk::gpio_write_bit(pin, false);
        }
        for pin in OPTICS {
            sdk::gpio_init(pin, true);
        }

        self.optics_initialized = true;
        if !motor_ok || !swd_released || !optics_off() {
            self.faulted = true;
            return false;
        }

        self.opened = true;
        true
    }

    /// Bounded GPIO stop attempt, including from an independent ISR.
    pub fn trip(&mut self, now_ms: u32) {
        let _guard = IrqGuard::lock();
        self.faulted = true;
        if self.motor.fault == MotorGateFault::OK {
            self.motor.trip(now_ms);
        } else {
            self.motor.request(MotorCommand::STOP, now_ms);
        }
    }

    fn trip_now(&mut self) {
        self.trip(self.motor.observed_at);
    }

    pub(crate) fn inactive(&self) -> bool {
        self.optics_initialized
            && self.motor.applied_known
            && self.motor.applied == MotorCommand::STOP
            && self.motor.desired == MotorCommand::STOP
            && sdk::gpio_port_read(GPIO_PORT_A, MOTOR_MASK) == MOTOR_MASK
            && optics_off()
    }

    /// Mandatory before any drive or emitter enable; attach only while
    /// inactive.
    ///
    /// # Safety
    /// `flash` is null or a live authority that stays at this address, under
    /// the SAME IRQ serialization as this board, for the board's lifetime.
    pub unsafe fn attach_flash(&mut self, flash: *mut FlashCoord) -> bool {
        // SAFETY: caller supplies null or a live authority.
        let initialized = unsafe { flash.as_ref() }.is_some_and(|f| f.initialized);
        if !initialized || !self.opened || self.faulted {
            return false;
        }
        let _guard = IrqGuard::lock();
        let ok = (self.flash.is_null() || self.flash == flash) && self.inactive();
        if ok {
            self.flash = flash;
        }
        ok
    }

    /// Stops/inhibits motion for a flash lease. Interrupted output latches the
    /// board fault; success establishes GPIO logic only, not motor mechanics.
    pub fn flash_quiesce(&mut self) -> bool {
        if !self.optics_initialized || self.motor.write_pair.is_none() {
            return false;
        }
        let _guard = IrqGuard::lock();
        if !self.inactive() {
            self.trip_now(); // Never auto-resume interrupted feeding.
        } else {
            self.motor
                .request(MotorCommand::STOP, self.motor.observed_at);
        }
        self.optics(false);
        self.inactive()
    }

    fn flash_hold(&mut self) -> bool {
        if !outputs_blocked(self.flash) {
            return false;
        }
        self.flash_quiesce();
        true
    }

    /// Critical sections serialize gate calls between task and timer ISR.
    pub fn motor(&mut self, command: MotorCommand, now_ms: u32) -> bool {
        if !self.opened || (self.faulted && command != MotorCommand::STOP) {
            return false;
        }
        let _guard = IrqGuard::lock();
        if command != MotorCommand::STOP && self.flash_hold() {
            return false;
        }
        self.motor.request(command, now_ms)
    }

    pub fn motor_tick(&mut self, now_ms: u32) -> bool {
        if !self.opened {
            return false;
        }
        let _guard = IrqGuard::lock();
        if (self.motor.desired != MotorCommand::STOP || self.optics_enabled) && self.flash_hold() {
            return false;
        }
        self.motor.tick(now_ms)
    }

    pub fn optics(&mut self, enabled: bool) -> bool {
        self.optics_select(enabled, enabled)
    }

    /// PA15 to `a` and PB3 to `b`, with readback. Either one on counts as
    /// enabled; a failed change attempts both off and trips the motor gate.
    pub fn optics_select(&mut self, a: bool, b: bool) -> bool {
        let enabled = a || b;
        if !self.optics_initialized || (enabled && (!self.opened || self.faulted)) {
            return false;
        }
        let _guard = IrqGuard::lock();
        if enabled && self.flash_hold() {
            return false;
        }

        for (pin, level) in OPTICS.into_iter().zip([a, b]) {
            sdk::gpio_write_bit(pin, level);
        }

        let verified = optics_are(a, b);
        self.optics_enabled = verified && enabled;
        if !verified {
            for pin in OPTICS {
                sdk::gpio_write_bit(pin, false);
            }
            self.trip_now();
        }
        verified
    }

    /// 16 bounded conversions of `channel`: 4 (current), 5 (battery) or 10
    /// (the chip's internal supply measurement). Exclusive ADC ownership required.
    pub fn sample_channel(&mut self, channel: u32, samples: &mut [u16; SAMPLES]) -> bool {
        let known = [ADC_CURRENT, ADC_BATTERY, ADC_SUPPLY].contains(&channel);
        if !self.opened || self.faulted || !self.sensors_ready || !known {
            return false;
        }
        let ok = self.adc_channel(channel, samples);
        if !ok {
            self.trip_now();
        }
        ok
    }

    fn adc_channel(&self, channel: u32, samples: &mut [u16; SAMPLES]) -> bool {
        adc::convert(channel, self.adc_poll_limit, samples)
    }

    /// Exclusive ADC ownership required. Polls are bounded per conversion.
    /// Timestamp at acquisition START, so its execution time counts toward age.
    pub fn capture(&mut self, reading: &mut SensorReading) -> bool {
        if self.capture_gpio(reading).is_none() {
            return false;
        }
        reading.current_adc_valid = self.adc_channel(ADC_CURRENT, &mut reading.current);
        if reading.current_adc_valid {
            reading.battery_adc_valid = self.adc_channel(ADC_BATTERY, &mut reading.battery);
        }
        if !reading.current_adc_valid || !reading.battery_adc_valid {
            self.trip_now();
            return false;
        }
        true
    }

    /// GPIO only: PA16/PA17/PA19 and the emitter state, both ADC channels
    /// marked invalid. Returns the raw PA16..PA19 inputs, PA18 feedback
    /// included, read in the same port access.
    pub fn capture_gpio(&mut self, reading: &mut SensorReading) -> Option<u32> {
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

    pub fn close(&mut self) {
        if !self.opened {
            return;
        }
        self.trip_now();
        self.optics(false);
        if self.sensors_ready {
            adc::close();
        }
        self.opened = false;
    }
}
