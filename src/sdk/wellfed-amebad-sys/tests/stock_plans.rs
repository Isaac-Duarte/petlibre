//! The replacement never writes stock's plan sectors: a rollback to stock
//! finds its plans where it left them. Every flash span a Wellfed writer
//! reaches (the storage region, bank 1 for the updater, bank 0 for a recovery
//! write) lies apart from them.

use std::ops::Range;

use wellfed_amebad_sys::{BANK0_OFFSET, BANK1_OFFSET, BANK1_SECTORS, SECTOR_BYTES};
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
}
