//! The wall clock carried across a warm reset in the fake's RAM record,
//! stamped with the fake RTC.

use wellfed_core::clock::wall::{CARRY_WORDS, Origin, Restart, Sync};
use wellfed_hal::clock;
use wellfed_hal::sdk::fake;

// 2026-10-22T13:00:00Z.
const NOON: u64 = 1_792_674_000_000;

/// The RTC's time register for `seconds` since its reset.
fn tr(seconds: u32) -> u32 {
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (hours, minutes, secs) = (rest / 3600, rest / 60 % 60, rest % 60);
    let bcd = |value: u32| ((value / 10) << 4) | (value % 10);
    (days << 23) | (bcd(hours) << 16) | (bcd(minutes) << 8) | bcd(secs)
}

fn at(uptime_ms: u32, rtc_s: u32) {
    fake::with(|h| {
        h.uptime_ms = uptime_ms;
        h.rtc[0] = tr(rtc_s);
    });
}

fn record() -> [u32; CARRY_WORDS] {
    fake::with(|h| h.time_carry)
}

#[test]
fn the_rtc_count_reads_days_and_bcd_time() {
    assert_eq!(clock::rtc_count(tr(1_613), 0), Some(1_613));
    assert_eq!(clock::rtc_count(0x2653, 0), Some(1_613));
    assert_eq!(
        clock::rtc_count(tr(3 * 86_400 + 86_399), 0),
        Some(3 * 86_400 + 86_399)
    );
    assert_eq!(clock::rtc_count(tr(511 * 86_400), 0), Some(511 * 86_400));

    // The 12-hour format, the PM flag and digits out of range.
    assert_eq!(clock::rtc_count(tr(5), 1 << 7), None);
    assert_eq!(clock::rtc_count(tr(5) | (1 << 22), 0), None);
    for bad in [0x0000_000a, 0x0000_0060, 0x0000_6000, 0x0024_0000] {
        assert_eq!(clock::rtc_count(bad, 0), None, "{bad:#x}");
    }
}

/// One process, one clock: the scenario runs in order.
#[test]
fn the_clock_survives_a_warm_reset_but_not_a_power_on() {
    let owner = Sync {
        unix_ms: NOON,
        offset_s: -6 * 3600,
        next_change_s: 0,
        next_offset_s: -6 * 3600,
    };

    // Invalid: the record is cleared, not left from an earlier boot.
    fake::with(|h| h.time_carry = [0xa5a5_a5a5; CARRY_WORDS]);
    at(20_000, 1_000);
    clock::keep();
    assert_eq!(record(), [0; CARRY_WORDS]);

    // A sync is stored at once, and the loop keeps it fresh.
    clock::sync(owner, Origin::Owner).unwrap();
    assert_ne!(record(), [0; CARRY_WORDS]);
    at(80_000, 1_060);
    clock::keep();
    let kept = record();
    at(85_000, 1_065);
    clock::keep();
    assert_eq!(record(), kept, "kept at most every 10 s");

    // A watchdog reset 12 s later: the clock comes back with its error
    // grown by the RTC's gap.
    clock::reset();
    at(3_000, 1_072);
    clock::resume(Restart::Warm);
    let reading = clock::state().reading.expect("resumed");
    assert_eq!(reading.unix_ms, NOON + 72_000);
    assert_eq!(reading.error_ms, 1_300 + 1_000 + 130);
    assert_eq!(reading.offset_s, Some(-6 * 3600));

    // A power-on finds the same record and ignores it, then clears it.
    clock::reset();
    at(3_000, 4);
    clock::resume(Restart::Cold);
    assert_eq!(clock::state().reading, None);
    assert_eq!(record(), [0; CARRY_WORDS]);
}
