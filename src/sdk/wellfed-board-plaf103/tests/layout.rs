//! The storage map and pins as every firmware has written and driven them.
//! A store's sectors are on-flash state a reflash keeps, so a change here
//! strands records; this pins each value, not just the map's own consistency.

use wellfed_board_plaf103::storage::{self, Sectors};
use wellfed_board_plaf103::{STORAGE_OFFSET, STORAGE_SECTORS, pins};

const fn at(first: u32, count: u32) -> Sectors {
    Sectors { first, count }
}

#[test]
fn storage_region_and_sector_map_are_pinned() {
    assert_eq!(STORAGE_OFFSET, 0x34_0000);
    assert_eq!(STORAGE_SECTORS, 48);
    // Grown upward only: 0x340000 stays the first sector, and the end stays
    // under stock's settings at 0x3e8000.
    assert_eq!(STORAGE_OFFSET + STORAGE_SECTORS * 0x1000, 0x37_0000);

    assert_eq!(storage::JOURNAL, at(0, 2));
    assert_eq!(storage::SETTINGS, at(2, 2));
    assert_eq!(storage::CONFIRM, at(4, 1));
    assert_eq!(storage::NET, at(5, 2));
    assert_eq!(storage::SEED, at(7, 2));
    assert_eq!(storage::CLIENTS, at(9, 2));
    assert_eq!(storage::PLANS, at(11, 2));
    assert_eq!(storage::EVENTS, at(13, 12));
    assert_eq!(storage::UPDATE, at(25, 2));
    assert_eq!(storage::TIME, at(27, 2));
    assert_eq!(storage::TRACES, at(29, 3));
    assert_eq!(storage::STATS, at(32, 2));
    assert_eq!(storage::CALIBRATION, at(34, 2));
    assert_eq!(storage::RESET, at(36, 1));

    assert_eq!(
        storage::ALL,
        [
            storage::JOURNAL,
            storage::SETTINGS,
            storage::CONFIRM,
            storage::NET,
            storage::SEED,
            storage::CLIENTS,
            storage::PLANS,
            storage::EVENTS,
            storage::UPDATE,
            storage::TIME,
            storage::TRACES,
            storage::STATS,
            storage::CALIBRATION,
            storage::RESET,
        ]
    );
}

/// The factory reset erases exactly the stores the owner decided, and
/// never the journal, calibration, update store, confirmation or seed.
#[test]
fn the_factory_reset_clears_the_decided_stores_only() {
    let cleared: Vec<u32> = (0..storage::FACTORY_CLEARED_SECTORS)
        .map(|index| storage::factory_cleared(index).unwrap())
        .collect();
    assert_eq!(
        cleared,
        [9, 10, 5, 6, 2, 3, 27, 28, 11, 12, 32, 33, 29, 30, 31]
            .into_iter()
            .chain(13..25)
            .collect::<Vec<u32>>()
    );
    assert_eq!(
        storage::factory_cleared(storage::FACTORY_CLEARED_SECTORS),
        None
    );

    for kept in [
        storage::JOURNAL,
        storage::CONFIRM,
        storage::SEED,
        storage::UPDATE,
        storage::CALIBRATION,
        storage::RESET,
    ] {
        for sector in kept.first..kept.first + kept.count {
            assert!(!cleared.contains(&sector), "sector {sector} is kept");
        }
    }
}

#[test]
fn pins_are_pinned() {
    assert_eq!([pins::MOTOR_A, pins::MOTOR_B], [13, 14]);
    assert_eq!([pins::IR_PA, pins::IR_PB], [15, 35]);
    assert_eq!(
        [pins::OUTLET, pins::FOOD, pins::FEEDBACK, pins::ADAPTER],
        [16, 17, 18, 19]
    );
    assert_eq!(pins::BUTTONS, [36, 37, 38, 39]);
    assert_eq!(
        [
            pins::LED_CONNECTION,
            pins::LED_FOOD,
            pins::LED_LOCK,
            pins::LED_FAULT
        ],
        [0, 2, 4, 54]
    );
    assert_eq!(pins::AMP, 55);
    assert_eq!(
        [pins::ADC_CURRENT, pins::ADC_BATTERY, pins::ADC_SUPPLY],
        [4, 5, 10]
    );

    assert_eq!(pins::MOTOR_MASK, 0x6000);
    assert_eq!([pins::MOTOR_A_BIT, pins::MOTOR_B_BIT], [1 << 13, 1 << 14]);
    assert_eq!([pins::IR_PA_BIT, pins::IR_PB_BIT], [1 << 15, 1 << 3]);
}
