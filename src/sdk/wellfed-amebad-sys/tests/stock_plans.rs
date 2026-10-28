//! The replacement never writes stock's plan sectors: a rollback to stock
//! finds its plans where it left them, and the import reads them in place.
//! Every flash span a Wellfed writer reaches (the storage region, bank 1
//! for the updater, bank 0 for a recovery write) lies apart from them, and
//! the read itself stays inside them.

use std::ops::Range;

use wellfed_amebad_sys::{
    BANK0_OFFSET, BANK1_OFFSET, BANK1_SECTORS, SECTOR_BYTES, STOCK_PLAN_COUNT, STOCK_PLAN_ENTRIES,
    STOCK_PLAN_ENTRIES_LEN, fake, stock_plans_read,
};
use wellfed_board_plaf103::{STORAGE_OFFSET, STORAGE_SECTORS};

/// Stock's two plan sectors.
const PLAN_SECTORS: Range<u32> = 0x7d_0000..0x7d_2000;

fn apart(writer: &Range<u32>) -> bool {
    writer.end <= PLAN_SECTORS.start || PLAN_SECTORS.end <= writer.start
}

#[test]
fn no_writer_reaches_stock_s_plan_sectors() {
    let writers = [
        STORAGE_OFFSET..STORAGE_OFFSET + STORAGE_SECTORS * SECTOR_BYTES,
        BANK1_OFFSET..BANK1_OFFSET + BANK1_SECTORS * SECTOR_BYTES,
        BANK0_OFFSET..BANK1_OFFSET,
    ];
    for writer in &writers {
        assert!(apart(writer), "{writer:x?} reaches {PLAN_SECTORS:x?}");
    }

    // The read: the count byte at the count sector's start, and the ten
    // entries at the next one's, each inside its own sector.
    assert_eq!(STOCK_PLAN_COUNT, PLAN_SECTORS.start);
    assert_eq!(STOCK_PLAN_ENTRIES, PLAN_SECTORS.start + SECTOR_BYTES);
    assert_eq!(STOCK_PLAN_ENTRIES_LEN, 10 * 44);
    assert!(STOCK_PLAN_ENTRIES_LEN <= SECTOR_BYTES as usize);
}

#[test]
fn the_fake_serves_erased_sectors_until_a_test_sets_them() {
    fake::reset_hardware();
    let mut entries = [0; STOCK_PLAN_ENTRIES_LEN];
    assert_eq!(stock_plans_read(&mut entries), 0xff);
    assert!(entries.iter().all(|&byte| byte == 0xff));
}
