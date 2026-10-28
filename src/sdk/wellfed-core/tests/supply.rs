//! Tests for the supply's readings and the pack's floor: each kind of
//! uncertainty is named rather than read as a value, and the floor's hold,
//! cutoff and hysteresis.

use wellfed_core::supply::{
    Curve, Floor, FloorLimits, Pack, PackLimits, Reading, Sample, classify,
};

const PACK: PackLimits = PackLimits {
    divider_per_mille: 2_063,
    absent_pin_mv: 500,
    max_pack_mv: 5_500,
    min_running_mv: 2_500,
    stale_ms: 15_001,
    curve: Curve {
        empty_pin_mv: 1650,
        full_pin_mv: 2250,
    },
};

const FLOOR: FloorLimits = FloorLimits {
    floor: 1650,
    clear: 1710,
    cutoff: 1550,
    hold_ms: 300,
};

fn sample(pin_mv: u32, adapter: bool, age_ms: u32) -> Option<Sample> {
    Some(Sample {
        pin_mv,
        adapter,
        age_ms,
    })
}

#[test]
fn readings_name_their_uncertainty() {
    assert_eq!(classify(None, &PACK), Reading::UNREAD);
    assert_eq!(
        classify(sample(2306, false, 1200), &PACK),
        Reading {
            adapter: Some(false),
            pack: Pack::Reading {
                mv: 4757,
                percent: 100,
            },
        }
    );
    assert_eq!(
        classify(sample(16, true, 0), &PACK),
        Reading {
            adapter: Some(true),
            pack: Pack::Absent,
        }
    );
    assert_eq!(
        classify(sample(2306, true, 15_001), &PACK),
        Reading {
            adapter: None,
            pack: Pack::Stale { age_ms: 15_001 },
        }
    );
    assert_eq!(
        classify(sample(2306, true, 15_000), &PACK).pack_mv(),
        Some(4757)
    );
}

/// Stock's lookup: 0 % at or below 1650, then each 60 above it a 10 %
/// step, a reading taking the first step at or above it, 100 % from 2250.
#[test]
fn the_charge_follows_stocks_steps() {
    let curve = PACK.curve;
    let cases = [
        (0, 0),
        (1200, 0),
        (1650, 0),
        (1651, 10),
        (1710, 10),
        (1711, 20),
        (1770, 20),
        (1890, 40),
        (1891, 50),
        (2190, 90),
        (2191, 100),
        (2250, 100),
        (2306, 100),
        (u32::MAX, 100),
    ];
    for (pin_mv, percent) in cases {
        assert_eq!(curve.percent(pin_mv), percent, "{pin_mv} mV");
    }

    let flat = Curve {
        empty_pin_mv: 1650,
        full_pin_mv: 1650,
    };
    assert_eq!(flat.percent(1650), 0);
    assert_eq!(flat.percent(1651), 100);
}

#[test]
fn a_reading_carries_its_charge() {
    let reading = classify(sample(1800, false, 0), &PACK);
    assert_eq!(
        reading.pack,
        Pack::Reading {
            mv: 3713,
            percent: 30,
        }
    );
}

#[test]
fn a_value_no_pack_can_read_is_implausible() {
    // Above what three fresh alkaline cells read.
    assert_eq!(
        classify(sample(2700, true, 0), &PACK).pack,
        Pack::Implausible { mv: 5570 }
    );
    // On the pack alone, too low for the board to be running.
    assert_eq!(
        classify(sample(1200, false, 0), &PACK).pack,
        Pack::Implausible { mv: 2475 }
    );
    // On the adapter a deeply drained pack can read that low.
    assert_eq!(
        classify(sample(1200, true, 0), &PACK).pack,
        Pack::Reading {
            mv: 2475,
            percent: 0,
        }
    );
    assert_eq!(classify(sample(1200, false, 0), &PACK).pack_mv(), None);
}

#[test]
fn the_floor_holds_a_sag_and_cuts_off_a_deep_one() {
    assert!(FLOOR.valid());
    let mut floor = Floor::default();
    assert_eq!(floor.below(), None);
    assert!(!floor.above());

    floor.update(0, Some(2000), &FLOOR);
    assert!(floor.above());

    floor.update(20, Some(1600), &FLOOR);
    floor.update(319, Some(1600), &FLOOR);
    assert!(floor.above(), "inside the hold");
    floor.update(320, Some(1600), &FLOOR);
    assert_eq!(floor.below(), Some(true));

    floor.update(340, Some(1700), &FLOOR);
    assert_eq!(floor.below(), Some(true), "inside the band");
    floor.update(360, Some(1710), &FLOOR);
    assert!(floor.above());

    floor.update(380, Some(1549), &FLOOR);
    assert_eq!(floor.below(), Some(true), "under the cutoff");
}

#[test]
fn a_first_reading_judges_the_pack() {
    let mut floor = Floor::default();
    floor.update(0, Some(1660), &FLOOR);
    assert!(
        floor.above(),
        "the band counts as above until a pack is low"
    );

    let mut floor = Floor::default();
    floor.update(0, Some(1600), &FLOOR);
    assert_eq!(floor.below(), None, "unjudged until the hold");
    floor.update(300, None, &FLOOR);
    assert_eq!(floor.below(), Some(true), "a missing reading counts under");
}

#[test]
fn the_hold_survives_the_clock_wrap() {
    let mut floor = Floor::default();
    floor.update(u32::MAX - 100, Some(2000), &FLOOR);
    floor.update(u32::MAX - 10, Some(1600), &FLOOR);
    floor.update(200, Some(1600), &FLOOR);
    assert!(floor.above());
    floor.update(290, Some(1600), &FLOOR);
    assert_eq!(floor.below(), Some(true));
}

#[test]
fn limits_must_order_cutoff_floor_and_clear() {
    for limits in [
        FloorLimits { cutoff: 0, ..FLOOR },
        FloorLimits {
            cutoff: 1650,
            ..FLOOR
        },
        FloorLimits {
            clear: 1650,
            ..FLOOR
        },
        FloorLimits {
            hold_ms: 0,
            ..FLOOR
        },
    ] {
        assert!(!limits.valid());
    }
}
