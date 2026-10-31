//! Initial stock-derived STOP and infrared-off request, without opening Board.
//! Readback describes GPIO logic; physical polarity and transients need measurement.
//! Infrared OFF requires both a LOW latch and a LOW pad input: a dead input
//! path reads LOW whatever is driven, so input alone cannot prove OFF.
//! PB3 is also SWD_CLK: until SWD is released (as stock does before it
//! configures PB3) the pad ignores the latch and reads its pull-down, so
//! the release is part of the verified state.

use core::sync::atomic::{AtomicU32, Ordering};

use crate::board::MOTOR_MASK;
use crate::plaf103::pins::{IR_PA_BIT, IR_PB_BIT};
use crate::sdk::{self, IrqGuard};

pub const PA_MASK: u32 = MOTOR_MASK | IR_PA_BIT;
pub const PB_MASK: u32 = IR_PB_BIT;

/// Emitters held on at rest under an armed supervisor budget: bit 0 PA15,
/// bit 1 PB3. Written by the optics probe and by the owner's food readings,
/// which never share a firmware; read by health.
static SUPERVISED: AtomicU32 = AtomicU32::new(0);

/// Both emitters, as [`set_supervised`] takes them.
pub const BOTH_EMITTERS: u32 = 3;

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
        let a = if emitters & 1 != 0 { IR_PA_BIT } else { 0 };
        let b = if emitters & 2 != 0 { PB_MASK } else { 0 };
        emitters != 0
            && self.motor_verified()
            && self.pa & IR_PA_BIT == a
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
        latch_a: sdk::gpio_output_latch(0) & IR_PA_BIT,
        latch_b: sdk::gpio_output_latch(1) & PB_MASK,
        swd: sdk::swd_pinmux(),
    }
}

/// Enable both motor pins as outputs at STOP, without the driver ever seeing
/// its inputs differ, whatever its undriven inputs read.
///
/// Neither undriven level is safe to assume. `gpio_init` enables one pin per
/// call, so whichever level the first pin drives, the driver sees it against
/// the other's undriven level for one call: about 105 µs of drive, measured
/// at the motor with the latches preloaded HIGH against inputs at their
/// pull-down's LOW, and on the inputs with the latches preloaded LOW against
/// inputs that read HIGH (with the analyzer's hooks on them). So both latches go HIGH
/// first, which the data register holds while the pins are still inputs, and
/// one direction write then makes both outputs at once: the driver goes from
/// whatever it saw straight to brake. When both are already outputs (the
/// board opening after stopped startup), the latch write is the whole change,
/// one port write, so the motor never passes through coast.
pub(crate) fn enable_motor_stopped() {
    // SAFETY: the motor pins, which this module and the board own: called at
    // startup and as the board opens, before its gate can drive them.
    unsafe {
        sdk::gpio_port_write(0, MOTOR_MASK, MOTOR_MASK);
        sdk::gpio_enable_outputs(0, MOTOR_MASK);
    }
}

/// Preload the infrared latches LOW before enabling their outputs, and bring
/// the motor pair to STOP with both pins enabled together.
pub fn initialize() -> Snapshot {
    let _guard = IrqGuard::lock();
    let clock = sdk::gpio_clock_enable();
    if !clock.verified() {
        return Snapshot { clock, ..observe() };
    }

    sdk::swd_off();
    // SAFETY: the emitter latches, which this module owns until the board opens.
    unsafe {
        sdk::gpio_port_write(0, IR_PA_BIT, 0);
        sdk::gpio_port_write(1, PB_MASK, 0);
    }
    enable_motor_stopped();
    use crate::plaf103::pins::{IR_PA, IR_PB};
    for pin in [IR_PA, IR_PB] {
        // SAFETY: as above, with each latch already LOW.
        unsafe { sdk::gpio_init(pin, true) };
    }
    Snapshot { clock, ..observe() }
}
