//! Initial stock-derived STOP and infrared-off request, without opening Board.
//! Readback describes GPIO logic; physical polarity and transients need measurement.
//! Infrared OFF requires both a LOW latch and a LOW pad input: a dead input
//! path reads LOW whatever is driven, so input alone cannot prove OFF.
//! PB3 is also SWD_CLK: until SWD is released (as stock does before it
//! configures PB3) the pad ignores the latch and reads its pull-down, so
//! the release is part of the verified state.

use core::sync::atomic::{AtomicU32, Ordering};

use crate::board::MOTOR_MASK;
use crate::sdk::{self, IrqGuard};

pub const PA_MASK: u32 = MOTOR_MASK | sdk::IR_PA_BIT;
pub const PB_MASK: u32 = sdk::IR_PB_BIT;

/// Emitters the probe holds on under an armed supervisor budget: bit 0
/// PA15, bit 1 PB3. Written only by that probe; read by health.
static SUPERVISED: AtomicU32 = AtomicU32::new(0);

/// Publish the emitters now on under supervision (0 before any emitter change).
pub fn set_supervised(emitters: u32) {
    SUPERVISED.store(emitters & 3, Ordering::Release);
}

#[derive(Clone, Copy, Debug)]
pub struct Snapshot {
    pub clock: sdk::LpClock,
    pub pa: u32,
    pub pb: u32,
    pub dira: u32,
    pub dirb: u32,
    /// Output latches of the infrared emitters (PA15, PB3).
    pub latch_a: u32,
    pub latch_b: u32,
    /// LP `REG_SWD_PMUX_EN`; `SWD_PMUX_BIT` must be clear.
    pub swd: u32,
}

impl Snapshot {
    pub fn verified(self) -> bool {
        self.clock.verified()
            && self.pa == MOTOR_MASK
            && self.pb == 0
            && self.dira == PA_MASK
            && self.dirb == PB_MASK
            && self.latch_a == 0
            && self.latch_b == 0
            && self.swd & sdk::SWD_PMUX_BIT == 0
    }

    /// Motor STOP and every output direction, emitters aside.
    pub fn motor_verified(self) -> bool {
        self.clock.verified()
            && self.pa & MOTOR_MASK == MOTOR_MASK
            && self.dira == PA_MASK
            && self.dirb == PB_MASK
            && self.swd & sdk::SWD_PMUX_BIT == 0
    }

    /// Motor STOP, and the emitters' latches and pads exactly those published
    /// as supervised. Never true with none published.
    pub fn sensing(self) -> bool {
        let emitters = SUPERVISED.load(Ordering::Acquire);
        let a = if emitters & 1 != 0 { sdk::IR_PA_BIT } else { 0 };
        let b = if emitters & 2 != 0 { PB_MASK } else { 0 };
        emitters != 0
            && self.motor_verified()
            && self.pa & sdk::IR_PA_BIT == a
            && self.latch_a == a
            && self.pb == b
            && self.latch_b == b
    }
}

pub fn observe() -> Snapshot {
    let _guard = IrqGuard::lock();
    Snapshot {
        clock: sdk::gpio_clock_state(),
        pa: sdk::gpio_port_read(0, PA_MASK) & PA_MASK,
        pb: sdk::gpio_port_read(1, PB_MASK) & PB_MASK,
        dira: sdk::gpio_direction(0) & PA_MASK,
        dirb: sdk::gpio_direction(1) & PB_MASK,
        latch_a: sdk::gpio_output_latch(0) & sdk::IR_PA_BIT,
        latch_b: sdk::gpio_output_latch(1) & PB_MASK,
        swd: sdk::swd_pinmux(),
    }
}

/// Enable both motor pins as outputs and request STOP, without the driver
/// ever seeing its inputs differ.
///
/// `gpio_init` enables one pin per call, and an undriven input reads LOW
/// through the driver's pull-down. With the latches preloaded HIGH, the first
/// enabled pin drove HIGH against the other's LOW: forward drive for one call,
/// measured as a 105 µs pulse across the motor at every boot. Enabling both
/// LOW (standby, which the pull-downs already hold) and then writing STOP to
/// both in one port write goes from standby straight to brake. From brake,
/// the LOW write is a coast, still with equal inputs.
pub(crate) fn enable_motor_stopped() {
    use sdk::board::{MOTOR_A, MOTOR_B};

    sdk::gpio_port_write(0, MOTOR_MASK, 0);
    sdk::gpio_init(MOTOR_A, true);
    sdk::gpio_init(MOTOR_B, true);
    sdk::gpio_port_write(0, MOTOR_MASK, MOTOR_MASK);
}

/// Preload the infrared latches LOW before enabling their outputs, and bring
/// the motor pair to STOP through standby only.
pub fn initialize() -> Snapshot {
    let _guard = IrqGuard::lock();
    let clock = sdk::gpio_clock_enable();
    if !clock.verified() {
        return Snapshot { clock, ..observe() };
    }

    sdk::swd_off();
    sdk::gpio_port_write(0, sdk::IR_PA_BIT, 0);
    sdk::gpio_port_write(1, PB_MASK, 0);
    enable_motor_stopped();
    use sdk::board::{IR_PA, IR_PB};
    for pin in [IR_PA, IR_PB] {
        sdk::gpio_init(pin, true);
    }
    Snapshot { clock, ..observe() }
}
