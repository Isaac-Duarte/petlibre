//! Checks that the production supervision constants stay consistent: the
//! watchdog clock bounds cover both bench-measured expiries (FW05, FW06) and
//! the flash lease fits up to the documented maximum sample period.

use wellfed_hal::config::{FLASH_LIMITS, MAX_SAMPLE_PERIOD_MS, WATCHDOG};
use wellfed_hal::owner::OwnerConfig;
use wellfed_hal::watchdog::WatchdogConfig;

fn owner(sample_period_ms: u32) -> OwnerConfig {
    let mut c = OwnerConfig {
        watchdog: WATCHDOG,
        sample_period_ms,
        ..OwnerConfig::default()
    };
    c.events.timer_period_ms = 10;
    c
}

#[test]
fn watchdog_bounds_cover_the_fw05_measurement() {
    assert!(WATCHDOG.valid());

    // Both measured expiries (FW05 ~3.0-3.1 s, FW06 ~4.0-4.2 s) lie inside the span.
    let counts = 4095u64 * 32;
    for measured_ms in [3000u64, 3100, 4000, 4200] {
        let hz = counts * 1000 / measured_ms;
        assert!(
            (u64::from(WATCHDOG.clock_min_hz)..=u64::from(WATCHDOG.clock_max_hz)).contains(&hz)
        );
    }

    // The FW05 probe bounds (25-45 kHz) left only ~5 % headroom above 43 kHz.
    let narrow = WatchdogConfig {
        clock_max_hz: 45_000,
        ..WATCHDOG
    };
    assert!(narrow.valid());
}

#[test]
fn faster_clock_beyond_bounds_would_break_the_gap_margin() {
    // At 66 kHz the shortest expiry (~1.99 s) no longer exceeds twice the gap.
    let fast = WatchdogConfig {
        clock_max_hz: 66_000,
        ..WATCHDOG
    };
    assert!(!fast.valid());
}

#[test]
fn flash_lease_fits_up_to_the_documented_sample_period() {
    assert!(owner(MAX_SAMPLE_PERIOD_MS).lease_fits(&FLASH_LIMITS));
    assert!(!owner(MAX_SAMPLE_PERIOD_MS + 1).lease_fits(&FLASH_LIMITS));
}
