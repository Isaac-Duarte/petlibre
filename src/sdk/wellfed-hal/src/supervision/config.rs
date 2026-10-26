//! Production supervision constants derived from bench measurements.
//!
//! Physical, research unit, count 11 / divisor 31 (131 040 counts): in the supervision probe
//! the KM4 watchdog reset the platform about 3.0-3.1 s after the last refresh
//! (effective clock near 43 kHz); in the park capture both injected
//! resets came about 4.0-4.2 s after it (near 31-33 kHz). The cause of the
//! difference is unknown. The bounds cover both with margin; they are not a
//! characterization of the clock.

use wellfed_core::flash_coord::FlashLimits;

use crate::supervision::watchdog::WatchdogConfig;

/// SDK-documented 31 us SYSTIMER tick; the supervision probe measured it within host timing
/// uncertainty (about -475 ppm, +/-0.1 %).
pub const SYSTIMER_HZ: u32 = 32_768;

/// Shortest expiry 2 426 ms (54 kHz), longest 5 460 ms (24 kHz).
pub const WATCHDOG: WatchdogConfig = WatchdogConfig {
    systimer_hz: SYSTIMER_HZ,
    clock_min_hz: 24_000,
    clock_max_hz: 54_000,
    min_refresh_ms: 200,
    max_progress_gap_ms: 1000,
    max_expiry_ms: 6000,
    divisor: 31,
    count_id: 11,
};

/// One mediated flash lease. `active_ms` covers a 4 KiB sector erase: the
/// PLAF103's GD25Q64ESIG (-40 to 85 C grade) specifies 45 ms typical and
/// 300 ms maximum, and a page program 2.4 ms maximum; the bench measured
/// 27-35 ms erases on the scratch sector. Block and chip erases do not fit
/// and must stay unlinked.
pub const FLASH_LIMITS: FlashLimits = FlashLimits {
    quiesce_ms: 100,
    park_ms: 50,
    active_ms: 500,
    release_ms: 50,
    total_ms: 700,
};

/// Largest owner sample period for which a whole lease fits the watchdog
/// progress gap with a 10 ms supervision timer (see `OwnerConfig::lease_fits`).
pub const MAX_SAMPLE_PERIOD_MS: u32 =
    (WATCHDOG.max_progress_gap_ms - FLASH_LIMITS.total_ms - 2 * 10 - 1) / 2;

// Margin around both measurements: 43 kHz and 31 kHz.
const _: () = assert!(
    FLASH_LIMITS.total_ms < WATCHDOG.max_progress_gap_ms
        && WATCHDOG.clock_max_hz >= 43_000 * 5 / 4
        && WATCHDOG.clock_min_hz <= 31_000 * 4 / 5
);
