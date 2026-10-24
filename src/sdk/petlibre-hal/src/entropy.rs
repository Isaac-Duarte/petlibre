//! Raw noise for the random generator. The AmebaD exposes no hardware
//! RNG, so two physical sources stand in, both still unmeasured:
//! - **Clock jitter:** the CPU-clock SysTick count between successive edges
//!   of the 32 kHz system timer. The two clocks come from different
//!   oscillators, so the count's low bits wander with their relative phase
//!   noise and with interrupt timing.
//! - **ADC noise:** the low bits of a raw conversion.
//!
//! These return raw samples only; how much entropy each holds is the bench's
//! to measure, and `petlibre_core::rng` tests and conditions them.

use crate::{adc, sdk};

/// Polls of the 32 kHz timer before an edge counts as missing (about 30 us
/// apart; each poll is a short ROM call).
pub const EDGE_POLLS: u32 = 100_000;

/// Successive SysTick readings at 32 kHz edges.
#[derive(Clone, Copy, Debug, Default)]
pub struct Jitter {
    last: Option<u32>,
}

impl Jitter {
    pub const fn new() -> Self {
        Self { last: None }
    }

    /// The low byte of the CPU cycles since the previous edge, or `None`
    /// when no edge came within [`EDGE_POLLS`] or SysTick is not running.
    /// The first call only takes a reading.
    pub fn sample(&mut self) -> Option<u8> {
        let start = sdk::systimer_ticks();
        (0..EDGE_POLLS).find(|_| sdk::systimer_ticks() != start)?;
        let now = sdk::systick();
        if now.reload == 0 {
            return None;
        }

        let previous = self.last.replace(now.value)?;
        // SysTick counts down and wraps at its reload.
        let period = now.reload + 1;
        let cycles = (previous + period - now.value) % period;
        Some(cycles.to_le_bytes()[0])
    }
}

/// The low byte of one raw conversion of `channel`. The ADC must be open
/// (`adc::open`) and claimed from the runtime's power monitor.
pub fn adc_sample(channel: u32, poll_limit: u32) -> Option<u8> {
    adc::convert_one(channel, poll_limit).map(|raw| raw.to_le_bytes()[0])
}
