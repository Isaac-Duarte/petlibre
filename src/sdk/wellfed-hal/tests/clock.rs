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

/// The SysTimer's count `ms` after its start.
fn systimer(ms: u64) -> u32 {
    u32::try_from(ms * 32_768 / 1000).expect("tests stay inside the counter's range")
}

/// Uptime and the SysTimer both at `uptime_ms`, the RTC at `rtc_s`.
fn at(uptime_ms: u32, rtc_s: u32) {
    fake::with(|h| {
        h.uptime_ms = uptime_ms;
        h.raw_ticks = systimer(u64::from(uptime_ms));
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

/// The hardware since power-on, kept in step: the true time, the RTC's
/// whole seconds and the SysTimer follow it, and uptime (the scheduler's
/// tick) skips whatever a flash lease stopped.
struct Board {
    true_ms: u64,
    uptime_ms: u32,
}

impl Board {
    fn new(true_ms: u64) -> Self {
        let board = Self {
            true_ms,
            uptime_ms: u32::try_from(true_ms).unwrap(),
        };
        board.apply();
        board
    }

    fn apply(&self) {
        let rtc_s = u32::try_from(self.true_ms / 1000).unwrap();
        fake::with(|h| {
            h.uptime_ms = self.uptime_ms;
            h.raw_ticks = systimer(self.true_ms);
            h.rtc[0] = tr(rtc_s);
        });
    }

    /// `ms` pass with the tick running.
    fn run(&mut self, ms: u32) {
        self.true_ms += u64::from(ms);
        self.uptime_ms += ms;
        self.apply();
    }

    /// One erase's lease: interrupts masked and SysTick stopped for `ms`.
    fn erase(&mut self, ms: u32) {
        self.true_ms += u64::from(ms);
        self.apply();
    }

    /// A watchdog reset: uptime starts again, the RTC and true time go on.
    fn reset(&mut self, ms: u32) {
        self.true_ms += u64::from(ms);
        self.uptime_ms = 0;
        self.apply();
    }
}

/// How far the clock reads from the true time, and the error it claims.
fn miss(board: &Board) -> (u64, u64) {
    let reading = clock::state().reading.expect("valid");
    (
        reading.unix_ms.abs_diff(NOON + board.true_ms - 20_000),
        reading.error_ms,
    )
}

/// The recovery role's download of a bank (249 erases of about 45 ms, with
/// the loop storing the clock between them) stops the scheduler's tick for
/// 11.2 s of its 62 s. Counting those as elapsed is what keeps the clock,
/// and the record a trial boot resumes, within the error it claims: run on
/// the tick, the clock read 11 s behind with an error of about 1.3 s.
#[test]
fn a_download_that_stops_the_tick_keeps_the_clock_within_its_error() {
    let owner = Sync {
        unix_ms: NOON,
        offset_s: 0,
        next_change_s: 0,
        next_offset_s: 0,
    };
    clock::reset();
    let mut board = Board::new(20_000);
    clock::sync(owner, Origin::Owner).unwrap();
    assert_eq!(miss(&board), (0, 1_000));

    for _ in 0..249 {
        board.erase(45);
        board.run(205);
        clock::keep();
    }
    assert_eq!(board.true_ms - 20_000, 62_250);
    assert_eq!(board.uptime_ms - 20_000, 51_045, "11.2 s of ticks lost");
    let (missed, error) = miss(&board);
    assert!(missed <= error, "running: {missed} ms off, claims {error}");
    assert!(error < 1_400, "the erases cost no error: {error}");

    // The trial boot 6 s later resumes from the last store.
    board.reset(6_000);
    clock::reset();
    clock::resume(Restart::Warm);
    let (missed, error) = miss(&board);
    assert!(missed <= error, "resumed: {missed} ms off, claims {error}");
}

/// A SysTimer read that finds the count moved half its range or more lost
/// the time since the last read: the clock forgets the time rather than
/// project across it, and clears the record a reset would resume.
#[test]
fn a_count_that_lost_its_place_forgets_the_time() {
    let owner = Sync {
        unix_ms: NOON,
        offset_s: 0,
        next_change_s: 0,
        next_offset_s: 0,
    };
    clock::reset();
    at(20_000, 20);
    clock::sync(owner, Origin::Owner).unwrap();
    assert!(clock::state().reading.is_some());

    fake::with(|h| h.raw_ticks = h.raw_ticks.wrapping_add(1 << 31));
    assert_eq!(clock::state().reading, None);
    assert_eq!(record(), [0; CARRY_WORDS]);
}
