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
        claim_outputs(&interlock, || false, || 0, |_| {}, |why| held = Some(why)),
        Err(Refusal::Busy)
    );
    assert_eq!(held, Some(Held::Joining));

    interlock.end_join();
    assert_eq!(
        claim_outputs(&interlock, || false, || 0, |_| {}, |_| unreachable!()),
        Ok(())
    );
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
    claim_outputs(&interlock, || false, || 0, |_| {}, |_| unreachable!()).unwrap();
    let now = Cell::new(0_u32);
    let start = interlock.begin_join(|| now.get(), |ms| now.set(now.get() + ms));
    assert_eq!(start, JoinStart::Forced { ms: FEED_BOUND_MS });
}

#[test]
fn a_store_write_waits_out_the_longest_feed_and_no_longer() {
    use wellfed_fw_feeder::feeding::{FEED_BOUND_MS, claim_outputs};
    use wellfed_sdk::hal::interlock::{Interlock, WriteStart, WriteStep, WriteTurn};

    let interlock = Interlock::new();
    claim_outputs(&interlock, || false, || 0, |_| {}, |_| unreachable!()).unwrap();
    let mut turn = WriteTurn::new();

    assert_eq!(turn.begin(&interlock, 0, || true), WriteStep::Wait);
    assert_eq!(
        turn.begin(&interlock, FEED_BOUND_MS - 1, || true),
        WriteStep::Wait
    );
    assert_eq!(
        turn.begin(&interlock, FEED_BOUND_MS, || true),
        WriteStep::Go(WriteStart::Forced { ms: FEED_BOUND_MS })
    );
}

#[test]
fn a_feed_waits_for_a_running_store_write_and_is_refused_past_the_wait() {
    use wellfed_fw_feeder::feeding::claim_outputs;
    use wellfed_proto::api::Refusal;
    use wellfed_sdk::hal::interlock::{
        Held, Interlock, WRITE_WAIT_MS, WriteStart, WriteStep, WriteTurn,
    };
    use std::cell::Cell;

    let interlock = Interlock::new();
    let mut turn = WriteTurn::new();
    assert_eq!(
        turn.begin(&interlock, 0, || true),
        WriteStep::Go(WriteStart::Free)
    );

    // The write ends while the feed waits: the feed goes ahead.
    let now = Cell::new(0_u32);
    let claimed = claim_outputs(
        &interlock,
        || false,
        || now.get(),
        |ms| {
            now.set(now.get() + ms);
            if now.get() >= 100 {
                interlock.end_write();
            }
        },
        |_| unreachable!(),
    );
    assert_eq!(claimed, Ok(()));
    assert!(now.get() < WRITE_WAIT_MS);
    interlock.release_feed();

    // A write that never ends refuses the feed once the wait is over.
    assert_eq!(
        turn.begin(&interlock, 0, || true),
        WriteStep::Go(WriteStart::Free)
    );
    let now = Cell::new(0_u32);
    let mut held = None;
    let claimed = claim_outputs(
        &interlock,
        || false,
        || now.get(),
        |ms| now.set(now.get() + ms),
        |why| held = Some(why),
    );
    assert_eq!(claimed, Err(Refusal::Busy));
    assert_eq!(held, Some(Held::Writing));
    assert_eq!(now.get(), WRITE_WAIT_MS);
}

#[test]
fn the_outputs_turn_is_released_only_at_rest() {
    use wellfed_fw_feeder::feeding::{InFlight, may_release};
    use wellfed_sdk::hal::owner::{OwnerState, OwnerStatus};

    let at = |state| OwnerStatus {
        state,
        stopped: true,
        optics_off: true,
        ..OwnerStatus::default()
    };

    for state in [
        OwnerState::Queued,
        OwnerState::Preparing,
        OwnerState::Feeding,
        OwnerState::Reporting,
    ] {
        assert!(!may_release(InFlight::Nothing, &at(state)), "{state:?}");
        assert!(!may_release(InFlight::Feed, &at(state)), "{state:?}");
    }
    for state in [
        OwnerState::Starting,
        OwnerState::Idle,
        OwnerState::Terminal,
        OwnerState::Fault,
        OwnerState::Stopped,
    ] {
        assert!(may_release(InFlight::Nothing, &at(state)), "{state:?}");
        // A feed not yet recorded keeps the turn.
        assert!(!may_release(InFlight::Feed, &at(state)), "{state:?}");
        // So does a failed stop of either output.
        let moving = OwnerStatus {
            stopped: false,
            ..at(state)
        };
        assert!(!may_release(InFlight::Nothing, &moving), "{state:?}");
        let lit = OwnerStatus {
            optics_off: false,
            ..at(state)
        };
        assert!(!may_release(InFlight::Nothing, &lit), "{state:?}");
    }
}
