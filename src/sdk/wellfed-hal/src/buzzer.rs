//! PA25, the line stock drives as a buzzer: a GPIO it pulses HIGH for 200
//! ticks. Wellfed sounds through the speaker, so nothing in the product
//! drives it; this module is for the probe that finds out whether the line
//! reaches anything.

use crate::sdk::{self, IrqGuard};

const PIN: u32 = crate::plaf103::pins::BUZZER;
const PORT: u32 = PIN >> 5;
const MASK: u32 = 1 << (PIN & 31);

// PA25 shares port A with the motor inputs, the emitter and three
// indicators, and every write here may come while the motor runs. Each one
// is a masked port write under the interrupt lock, so it cannot interleave
// with the supervision timer's motor writes, and the mask must reach no pin
// another owner holds.
const _: () = assert!(
    PORT == 0 && MASK & crate::stopped::PA_MASK == 0 && MASK & crate::leds::PA_MASK == 0,
    "the buzzer line overlaps a motor, emitter or indicator pin"
);

/// What the pad reads back after a write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Level {
    pub output: bool,
    pub latch_high: bool,
    pub input_high: bool,
}

/// Make PA25 an output, LOW. Stock holds it LOW at start; the latch is
/// written LOW before the direction too, so enabling it never pulses it.
pub fn enable() {
    let _guard = IrqGuard::lock();
    // SAFETY: PA25, which no other owner drives (checked above).
    unsafe { sdk::gpio_port_write(PORT, MASK, 0) };
    sdk::gpio_no_pull(PIN);
    // SAFETY: as above; `GPIO_Init` rewrites port A's direction register,
    // which the interrupt lock keeps from interleaving with the timer.
    unsafe { sdk::gpio_init(PIN, true) };
    // SAFETY: as above.
    unsafe { sdk::gpio_port_write(PORT, MASK, 0) };
}

/// Drive PA25 HIGH or LOW.
pub fn set(high: bool) {
    let _guard = IrqGuard::lock();
    // SAFETY: PA25, which no other owner drives (checked above).
    unsafe { sdk::gpio_port_write(PORT, MASK, if high { MASK } else { 0 }) };
}

/// The direction, latch and input of PA25.
pub fn level() -> Level {
    let _guard = IrqGuard::lock();
    Level {
        output: sdk::gpio_direction(PORT) & MASK != 0,
        latch_high: sdk::gpio_output_latch(PORT) & MASK != 0,
        input_high: sdk::gpio_port_read(PORT, MASK) != 0,
    }
}
