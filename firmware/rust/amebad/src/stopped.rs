//! Initial stock-derived STOP and infrared-off request, without opening Board.
//! Readback describes GPIO logic; physical polarity and transients need measurement.

use crate::board::MOTOR_MASK;
use crate::sdk::{self, IrqGuard};

pub const PA_MASK: u32 = MOTOR_MASK | (1 << 15);
pub const PB_MASK: u32 = 1 << 3;

#[derive(Clone, Copy, Debug)]
pub struct Snapshot {
    pub clock: sdk::GpioClock,
    pub pa: u32,
    pub pb: u32,
    pub dira: u32,
    pub dirb: u32,
}

impl Snapshot {
    pub fn verified(self) -> bool {
        self.clock.verified()
            && self.pa == MOTOR_MASK
            && self.pb == 0
            && self.dira == PA_MASK
            && self.dirb == PB_MASK
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
    sdk::gpio_port_write(0, PA_MASK, MOTOR_MASK);
    sdk::gpio_port_write(1, PB_MASK, 0);
    for pin in [13, 14, 15, 35] {
        sdk::gpio_init(pin, true);
    }
    Snapshot { clock, ..observe() }
}
