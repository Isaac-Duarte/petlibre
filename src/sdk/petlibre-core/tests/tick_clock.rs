//! Tests for extending the free-running `u32` hardware counter into
//! milliseconds: fraction carry, partitioned reads, wrap of both counters
//! and latched time failures.
//!
//! Many small reads must agree with one large read, and a backward or
//! over-long step invalidates the clock instead of rebasing it.

use petlibre_core::clock::tick::TickClock;

#[test]
fn frequent_reads_preserve_fraction() {
    for rate in [32_000u32, 32_768, 1_000_000] {
        let mut clock = TickClock::new(rate, 0, 0).unwrap();
        let mut now = u32::MAX;
        for tick in 1..=rate {
            now = clock.read(tick).unwrap();
            assert_eq!(now, (u64::from(tick) * 1000 / u64::from(rate)) as u32);
        }
        assert_eq!(now, 1000);
        assert_eq!(clock.fraction, 0);
    }
}

#[test]
fn partitions_match_a_single_read() {
    let mut fine = TickClock::new(32_768, 100, 25).unwrap();
    let mut coarse = fine;
    for tick in (100..=100_000).step_by(37) {
        fine.read(tick).unwrap();
    }

    let a = fine.read(100_123).unwrap();
    let b = coarse.read(100_123).unwrap();
    assert_eq!(a, b);
    assert_eq!(fine.fraction, coarse.fraction);
}

#[test]
fn both_counters_wrap() {
    let mut clock = TickClock::new(32_000, u32::MAX - 31, u32::MAX).unwrap();
    assert_eq!(clock.read(0), Some(0));
    assert_eq!(clock.fraction, 0);
    assert_eq!(clock.read(64), Some(2));
}

#[test]
fn time_failures_latch_without_rebasing() {
    let mut clock = TickClock::new(32_768, 100, 7).unwrap();
    assert_eq!(clock.read(99), None);
    assert!(!clock.valid);
    assert_eq!((clock.raw_at, clock.millis), (100, 7));
    assert_eq!(clock.read(200), None);

    let mut clock = TickClock::new(32_768, 0, 0).unwrap();
    assert_eq!(clock.read(0x8000_0000), None);
    let mut clock = TickClock::new(1, 0, 0).unwrap();
    assert_eq!(clock.read(0x0040_0000), None);
    assert!(!clock.valid);
}

#[test]
fn invalid_arguments_never_create_a_clock() {
    assert!(TickClock::new(0, 0, 0).is_none());
    let mut clock = TickClock::new(u32::MAX, 0, 0).unwrap();
    assert_eq!(clock.read(0x7fff_ffff), Some(499));
    clock.invalidate();
    assert!(!clock.valid);
    assert_eq!(clock.read(0x7fff_ffff), None);
}
