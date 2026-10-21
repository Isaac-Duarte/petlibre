//! Stock-derived indicator GPIOs. Colors and physical polarity are bench results.

use crate::sdk::{self, IrqGuard};
use crate::stopped;

pub const PA_MASK: u32 = (1 << 0) | (1 << 2) | (1 << 4);
pub const PB_MASK: u32 = 1 << 22;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Led {
    Connection,
    Food,
    Lock,
    Fault,
}

impl Led {
    pub const ALL: [Self; 4] = [Self::Connection, Self::Food, Self::Lock, Self::Fault];

    pub const fn pin(self) -> u32 {
        match self {
            Self::Connection => 0,
            Self::Food => 2,
            Self::Lock => 4,
            Self::Fault => 54,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Connection => "PA0",
            Self::Food => "PA2",
            Self::Lock => "PA4",
            Self::Fault => "PB22",
        }
    }
}

fn directions() -> bool {
    sdk::gpio_direction(0) & PA_MASK == PA_MASK && sdk::gpio_direction(1) & PB_MASK == PB_MASK
}

/// LOW is the stock software's disabled-indicator request, pending polarity check.
pub fn all_low() -> bool {
    let _guard = IrqGuard::lock();
    sdk::gpio_port_write(0, PA_MASK, 0);
    sdk::gpio_port_write(1, PB_MASK, 0);
    directions()
        && sdk::gpio_output_latch(0) & PA_MASK == 0
        && sdk::gpio_output_latch(1) & PB_MASK == 0
        && sdk::gpio_port_read(0, PA_MASK) == 0
        && sdk::gpio_port_read(1, PB_MASK) == 0
}

pub fn initialize() -> bool {
    let _guard = IrqGuard::lock();
    if !stopped::observe().verified() {
        return false;
    }
    let _ = all_low(); // Preload before output direction; verify after enabling.
    for led in Led::ALL {
        sdk::gpio_no_pull(led.pin());
        sdk::gpio_init(led.pin(), true);
    }
    directions() && all_low() && Led::ALL.into_iter().all(|led| observe(led).matches(false))
}

/// Request an indicator level. The caller must wait and verify with `observe`.
/// Only indicator bits are writable; stopped-output drift refuses the request.
pub fn request(led: Led, high: bool) -> bool {
    let _guard = IrqGuard::lock();
    if !directions() || !stopped::observe().verified() {
        let _ = all_low();
        return false;
    }
    let pin = led.pin();
    let mask = 1 << (pin & 31);
    sdk::gpio_port_write(pin >> 5, mask, if high { mask } else { 0 });
    true
}

#[derive(Clone, Copy, Debug)]
pub struct Readback {
    pub latch_high: bool,
    pub input_high: bool,
    pub directions_ok: bool,
    pub stopped_ok: bool,
    pub pad: u32,
}

/// Whether a settled request is verified, remains a bounded input experiment,
/// or must stop immediately. An input mismatch is never a successful step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Verified,
    InputMismatch,
    Abort,
}

impl Readback {
    pub fn decision(self, high: bool) -> Decision {
        if self.latch_high != high
            || !self.directions_ok
            || !self.stopped_ok
            || self.pad & (0x1f | (1 << 15) | (3 << 8)) != 0
        {
            return Decision::Abort;
        }
        if self.input_high == high {
            Decision::Verified
        } else if high {
            // Hold only the scheduled HIGH interval, then require LOW readback.
            Decision::InputMismatch
        } else {
            Decision::Abort
        }
    }

    pub fn matches(self, high: bool) -> bool {
        self.decision(high) == Decision::Verified
    }
}

pub fn observe(led: Led) -> Readback {
    let _guard = IrqGuard::lock();
    let pin = led.pin();
    Readback {
        latch_high: sdk::gpio_output_latch(pin >> 5) & (1 << (pin & 31)) != 0,
        input_high: sdk::gpio_read_bit(pin),
        directions_ok: directions(),
        stopped_ok: stopped::observe().verified(),
        pad: sdk::pad_control(pin),
    }
}

pub const SETTLE_MS: u32 = 10;

#[derive(Clone, Copy, Debug)]
pub struct ProbeStep {
    pub led: Led,
    pub high: bool,
    pub delay_ms: u32,
}

/// Two HIGH/LOW pulses per line, then terminal. Scheduling delays are nominal.
pub fn probe_step(index: u32) -> Option<ProbeStep> {
    let led = *Led::ALL.get((index / 4) as usize)?;
    Some(ProbeStep {
        led,
        high: index.is_multiple_of(2),
        delay_ms: if index % 4 == 1 { 500 } else { 1000 },
    })
}
