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

#[test]
fn a_feed_during_a_wifi_join_is_refused_busy() {
    use wellfed_fw_feeder::feeding::claim_outputs;
    use wellfed_proto::api::Refusal;
    use wellfed_sdk::hal::interlock::{Held, Interlock, JoinStart};

    let interlock = Interlock::new();
    assert_eq!(interlock.begin_join(|| 0, |_| {}), JoinStart::Free);

    let mut held = None;
    assert_eq!(
        claim_outputs(&interlock, |why| held = Some(why)),
        Err(Refusal::Busy)
    );
    assert_eq!(held, Some(Held::Joining));

    interlock.end_join();
    assert_eq!(claim_outputs(&interlock, |_| unreachable!()), Ok(()));
}

#[test]
fn a_join_waits_out_the_longest_feed_and_no_longer() {
    use wellfed_fw_feeder::feeding::{FEED_BOUND_MS, claim_outputs};
    use wellfed_sdk::hal::interlock::{Interlock, JoinStart};
    use std::cell::Cell;

    // The owner's own cap on a feed's emitters-on time, which holds the
    // prepare timeout and the longest drive, fits inside the bound.
    let longest = CONFIG.prepare_timeout_ms + CONFIG.feed.max_duration_ms;
    assert!(longest <= CONFIG.optics_max_on_ms);

    let interlock = Interlock::new();
    claim_outputs(&interlock, |_| unreachable!()).unwrap();
    let now = Cell::new(0_u32);
    let start = interlock.begin_join(|| now.get(), |ms| now.set(now.get() + ms));
    assert_eq!(start, JoinStart::Forced { ms: FEED_BOUND_MS });
}
