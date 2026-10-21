//! Experimental stock-derived PLAF103 pin map: motor PA13/PA14, optics
//! emitters 15/35, inputs PA16 outlet, PA17 food, PA18 feedback, PA19 DC, ADC
//! channels 4 (current) and 5 (battery). Not a physically verified board port.
//!
//! These functions TOUCH GPIO/ADC when executed. The diagnostic never calls
//! them. Verify the board, startup/output behavior and recovery first.

use core::ffi::c_void;
use core::ptr::{self, addr_of};

use petlibre_core::adc_scale::{AdcCalibration, MAX_RAW, SAMPLES};
use petlibre_core::feed_controller::MotorCommand;
use petlibre_core::flash_coord::FlashCoord;
use petlibre_core::motor_gate::{MotorGate, MotorGateFault};
use petlibre_core::sensor_inputs::SensorReading;

use crate::sdk::{self, GPIO_PORT_A, IrqGuard};

pub const MOTOR_MASK: u32 = (1 << 13) | (1 << 14);
const SENSOR_MASK: u32 = (1 << 16) | (1 << 17) | (1 << 19);
const OPTICS: [u32; 2] = [15, 35];

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

pub(crate) fn optics_off() -> bool {
    OPTICS.iter().all(|&pin| optic_is(pin, false))
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
    let value = if a { 1 << 13 } else { 0 } | if b { 1 << 14 } else { 0 };
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
        if adc_poll_limit == 0 || adc_poll_limit > 1_000_000 {
            return false;
        }
        if !self.open_outputs(deadtime_ms, lease_ms, now_ms) {
            return false;
        }
        self.opened = false;
        self.adc_poll_limit = adc_poll_limit;
        for pin in 16..=19 {
            sdk::gpio_init(pin, false);
        }
        let mut efuse = [0u8; 4];
        for (i, byte) in efuse.iter_mut().enumerate() {
            match sdk::efuse_read8(0x1d0 + i as u32) {
                Some(value) => *byte = value,
                None => {
                    self.faulted = true;
                    return false;
                }
            }
        }
        let Some(calibration) = AdcCalibration::decode(&efuse) else {
            self.faulted = true;
            return false;
        };
        self.calibration = calibration;
        sdk::pad_disable(33);
        sdk::pad_disable(34);
        sdk::adc_init_manual();
        sdk::adc_cmd(true);
        self.sensors_ready = true;
        self.opened = true;
        true
    }

    /// Motor STOP and emitters OFF only: no sensor inputs, efuse or ADC, so
    /// `capture` refuses. For supervision checks with motion disabled.
    pub fn open_outputs(&mut self, deadtime_ms: u32, lease_ms: u32, now_ms: u32) -> bool {
        if deadtime_ms == 0 || lease_ms <= deadtime_ms || lease_ms >= 0x8000_0000 {
            return false;
        }
        *self = Self::default();
        if !sdk::gpio_clock_enable().verified() {
            self.faulted = true;
            return false;
        }
        // Preload before output enable; actual reset/ROM-init behavior needs measurement.
        sdk::gpio_port_write(GPIO_PORT_A, MOTOR_MASK, MOTOR_MASK);
        sdk::gpio_init(13, true);
        sdk::gpio_init(14, true);
        let context = ptr::from_mut(self).cast();
        // SAFETY: the callback reads this fixed-address board while opened.
        let motor_ok = unsafe {
            self.motor
                .init(Some(motor_pair), context, deadtime_ms, lease_ms, now_ms)
        };
        for pin in OPTICS {
            sdk::gpio_write_bit(pin, false);
        }
        for pin in OPTICS {
            sdk::gpio_init(pin, true);
        }
        self.optics_initialized = true;
        if !motor_ok || !optics_off() {
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
        if !self.optics_initialized || (enabled && (!self.opened || self.faulted)) {
            return false;
        }
        let _guard = IrqGuard::lock();
        if enabled && self.flash_hold() {
            return false;
        }
        for pin in OPTICS {
            sdk::gpio_write_bit(pin, enabled);
        }
        let verified = OPTICS.iter().all(|&pin| optic_is(pin, enabled));
        self.optics_enabled = verified && enabled;
        if !verified {
            for pin in OPTICS {
                sdk::gpio_write_bit(pin, false);
            }
            self.trip_now();
        }
        verified
    }

    fn adc_channel(&self, channel: u32, samples: &mut [u16; SAMPLES]) -> bool {
        for sample in samples.iter_mut() {
            sdk::adc_select(channel);
            sdk::adc_clear_fifo();
            sdk::adc_sw_trigger(true);
            let ready = (0..self.adc_poll_limit).any(|_| sdk::adc_readable());
            sdk::adc_sw_trigger(false);
            if !ready {
                sdk::adc_clear_fifo();
                return false;
            }
            *sample = sdk::adc_read() & MAX_RAW;
        }
        true
    }

    /// Exclusive ADC ownership required. Polls are bounded per conversion.
    /// Timestamp at acquisition START, so its execution time counts toward age.
    pub fn capture(&mut self, reading: &mut SensorReading) -> bool {
        *reading = SensorReading::default();
        if !self.opened || self.faulted || !self.sensors_ready {
            return false;
        }
        let pins = sdk::gpio_port_read(GPIO_PORT_A, SENSOR_MASK);
        reading.gpio_valid = true;
        reading.food_high = pins & (1 << 17) != 0;
        reading.outlet_high = pins & (1 << 16) != 0;
        reading.dc_present = pins & (1 << 19) != 0;
        reading.optics_enabled =
            self.optics_enabled && OPTICS.iter().all(|&pin| optic_is(pin, true));
        reading.current_adc_valid = self.adc_channel(4, &mut reading.current);
        if reading.current_adc_valid {
            reading.battery_adc_valid = self.adc_channel(5, &mut reading.battery);
        }
        if !reading.current_adc_valid || !reading.battery_adc_valid {
            self.trip_now();
            return false;
        }
        true
    }

    pub fn close(&mut self) {
        if !self.opened {
            return;
        }
        self.trip_now();
        self.optics(false);
        if self.sensors_ready {
            sdk::adc_sw_trigger(false);
            sdk::adc_cmd(false);
        }
        self.opened = false;
    }
}
