//! The PLAF103's feeding owner configuration passes the owner's own checks,
//! and the journal it opens agrees with it.

use wellfed_fw_feeder::feeding::{CONFIG, JOURNAL, MAX_PORTIONS};
use wellfed_sdk::hal::supervision::config::FLASH_LIMITS;

#[test]
fn the_owner_accepts_the_configuration() {
    assert!(CONFIG.valid());
    // A whole flash lease fits between two beats.
    assert!(CONFIG.lease_fits(&FLASH_LIMITS));
}

#[test]
fn the_journal_matches_the_owner() {
    assert_eq!(JOURNAL.mode, CONFIG.bowl_mode);
    assert_eq!(JOURNAL.max_portions, CONFIG.feed.max_portions);
    assert_eq!(JOURNAL.max_portions, MAX_PORTIONS);
    assert_ne!(JOURNAL.binding, 0);
}
