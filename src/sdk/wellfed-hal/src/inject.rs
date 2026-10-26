//! A motor STOP that does not take, for the `stop-fail` bench variant only
//! (feature `stop-fail`; never in a product image).
//!
//! While a hold is armed, the board's pin write skips every STOP and then
//! reads the pins back as it always does: they still drive, so the write
//! fails its readback exactly as a pad that did not follow its latch would,
//! and every path that handles a failed stop runs unchanged (the gate's
//! latched fault, the timer interrupt's retry each tick, the owner's
//! hardware fault, the runtime's watchdog). Only STOP is held: a drive
//! write, which the gate never issues while faulted, is untouched.
//!
//! A hold lifts by itself once its length has passed, timed on SYSTIMER so
//! the interrupt that retries the stop can read it. The length is the bound
//! on what the injection can do: whatever else fails, the timer's next
//! retry after the lift stops the motor.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::sdk;
use crate::supervision::config::SYSTIMER_HZ;

/// The longest hold: past the owner's 1 s beat and the watchdog's longest
/// expiry (5.46 s), with margin, so a long hold shows the watchdog's reset
/// rather than its own lift, and no longer.
pub const MAX_HOLD_MS: u32 = 10_000;

static ARMED: AtomicBool = AtomicBool::new(false);
static FROM: AtomicU32 = AtomicU32::new(0);
static TICKS: AtomicU32 = AtomicU32::new(0);
static SKIPPED: AtomicU32 = AtomicU32::new(0);
static LIFTED: AtomicBool = AtomicBool::new(false);

/// What the injection has done since it was last armed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Hold {
    /// STOP writes are skipped now.
    pub armed: bool,
    /// The hold ran its length and lifted by itself.
    pub lifted: bool,
    /// STOP writes skipped since it was armed.
    pub skipped: u32,
}

/// Skip every STOP write for the next `ms` (at most `MAX_HOLD_MS`).
pub fn hold_stops(ms: u32) {
    let ms = ms.min(MAX_HOLD_MS);
    let ticks = u64::from(ms) * u64::from(SYSTIMER_HZ) / 1000;
    // At most 10 s of a 32 kHz clock: far inside a u32.
    let ticks = u32::try_from(ticks).unwrap_or(u32::MAX);

    let _guard = sdk::IrqGuard::lock();
    FROM.store(sdk::systimer_ticks(), Ordering::Relaxed);
    TICKS.store(ticks, Ordering::Relaxed);
    SKIPPED.store(0, Ordering::Relaxed);
    LIFTED.store(false, Ordering::Relaxed);
    ARMED.store(true, Ordering::Release);
}

/// The hold's state, for the variant's log.
pub fn status() -> Hold {
    let _guard = sdk::IrqGuard::lock();
    expire();
    Hold {
        armed: ARMED.load(Ordering::Acquire),
        lifted: LIFTED.load(Ordering::Relaxed),
        skipped: SKIPPED.load(Ordering::Relaxed),
    }
}

/// Lifts a hold whose length has passed.
fn expire() {
    if ARMED.load(Ordering::Acquire)
        && sdk::systimer_ticks().wrapping_sub(FROM.load(Ordering::Relaxed))
            >= TICKS.load(Ordering::Relaxed)
    {
        ARMED.store(false, Ordering::Release);
        LIFTED.store(true, Ordering::Relaxed);
    }
}

/// Whether this STOP write is skipped; counts it if so. Called by the pin
/// write, under its interrupt mask.
pub(crate) fn stop_skipped() -> bool {
    expire();
    let held = ARMED.load(Ordering::Acquire);
    if held {
        SKIPPED.fetch_add(1, Ordering::Relaxed);
    }
    held
}
