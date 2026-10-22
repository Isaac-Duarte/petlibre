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

pub const PA_MASK: u32 = MOTOR_MASK | (1 << 15);
pub const PB_MASK: u32 = 1 << 3;
const IR_A_MASK: u32 = 1 << 15;

/// Emitters the FW13 probe holds on under an armed supervisor budget: bit 0
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
        let a = if emitters & 1 != 0 { IR_A_MASK } else { 0 };
        let b = if emitters & 2 != 0 { PB_MASK } else { 0 };
        emitters != 0
            && self.motor_verified()
            && self.pa & IR_A_MASK == a
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
        latch_a: sdk::gpio_output_latch(0) & IR_A_MASK,
        latch_b: sdk::gpio_output_latch(1) & PB_MASK,
        swd: sdk::swd_pinmux(),
    }
}

/// Preload all four output latches before enabling any output direction.
/// Only HIGH/HIGH motor and LOW/LOW infrared requests exist in this path.
pub fn initialize() -> Snapshot {
    let _guard = IrqGuard::lock();
    let clock = sdk::gpio_clock_enable();
    if !clock.verified() {
        return Snapshot { clock, ..observe() };
    }
    sdk::swd_off();
    sdk::gpio_port_write(0, PA_MASK, MOTOR_MASK);
    sdk::gpio_port_write(1, PB_MASK, 0);
    for pin in [13, 14, 15, 35] {
        sdk::gpio_init(pin, true);
    }
    Snapshot { clock, ..observe() }
}
