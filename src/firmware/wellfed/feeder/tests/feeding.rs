//! The PLAF103's feeding owner configuration passes the owner's own checks,
//! and the journal it opens agrees with it.

use wellfed_fw_feeder::feeding::{CONFIG, JOURNAL, MAX_PORTIONS};
use wellfed_sdk::hal::supervision::config::FLASH_LIMITS;

/// Lost sensors held at rest restart the feeder once, after ten minutes,
/// and only from the first boot of a power cycle: a repeat holds for good.
#[test]
fn lost_sensors_held_at_rest_restart_once_a_power_cycle() {
    use wellfed_fw_feeder::feeding::{PowerCycle, RECOVERY_HOLD_MS, Recovery};

    let first = Some(PowerCycle::FirstBoot);
    let mut recovery = Recovery::default();
    assert!(!recovery.step(0, None));
    assert!(!recovery.step(1000, first));
    assert!(!recovery.step(1000 + RECOVERY_HOLD_MS - 1, first));
    assert_eq!(recovery.held_ms(1000 + RECOVERY_HOLD_MS), RECOVERY_HOLD_MS);
    assert!(recovery.step(1000 + RECOVERY_HOLD_MS, first));
    assert!(!recovery.step(1000 + 3 * RECOVERY_HOLD_MS, first), "once");

    // A hold that lapses (a feed in flight, an install) counts afresh.
    let mut recovery = Recovery::default();
    assert!(!recovery.step(0, first));
    assert!(!recovery.step(RECOVERY_HOLD_MS - 1, None));
    assert!(!recovery.step(RECOVERY_HOLD_MS, first));
    assert!(recovery.step(2 * RECOVERY_HOLD_MS, first));

    // After any reset this power cycle, the hold is for good.
    let mut recovery = Recovery::default();
    let restarted = Some(PowerCycle::Restarted);
    assert!(!recovery.step(0, restarted));
    assert!(!recovery.step(10 * RECOVERY_HOLD_MS, restarted));
}

#[test]
fn the_owner_accepts_the_configuration() {
    assert!(CONFIG.valid());
    // A whole flash lease fits between two beats.
    assert!(CONFIG.lease_fits(&FLASH_LIMITS));
}

/// The virtual feeder runs its owner under these limits against the
/// measured mechanism; it cannot link this crate, so it keeps a copy.
#[test]
fn the_virtual_feeder_runs_these_limits() {
    use wellfed_fw_feeder::feeding::FEED_BOUND_MS;
    use wellfed_sim::plaf103;

    assert_eq!(format!("{:?}", plaf103::OWNER), format!("{CONFIG:?}"));
    assert_eq!(plaf103::MAX_PORTIONS, MAX_PORTIONS);
    assert_eq!(plaf103::FEED_BOUND_MS, FEED_BOUND_MS);
}

#[test]
fn the_journal_matches_the_owner() {
    assert_eq!(JOURNAL.mode, CONFIG.bowl_mode);
    assert_eq!(JOURNAL.max_portions, CONFIG.feed.max_portions);
    assert_eq!(JOURNAL.max_portions, MAX_PORTIONS);
    // Every feed request is checked against the wire's cap before it
    // reaches the owner, which must accept every one that passes.
    assert_eq!(MAX_PORTIONS, u16::from(wellfed_proto::api::PORTIONS_MAX));
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
    use std::cell::Cell;
    use wellfed_fw_feeder::feeding::{FEED_BOUND_MS, claim_outputs};
    use wellfed_sdk::hal::interlock::{Interlock, JoinStart};

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
    use std::cell::Cell;
    use wellfed_fw_feeder::feeding::claim_outputs;
    use wellfed_proto::api::Refusal;
    use wellfed_sdk::hal::interlock::{
        Held, Interlock, WRITE_WAIT_MS, WriteStart, WriteStep, WriteTurn,
    };

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

/// The owner's records, as the loop logs them, decode through the host's
/// parser to what the bench acts on.
#[test]
fn owner_records_decode_for_the_bench() {
    use wellfed_core::feed::controller::FeedReason;
    use wellfed_core::feed::storage::{OwnerOutcome, OwnerResult};
    use wellfed_core::record::{Event, Owner, Record as Line, Tag};
    use wellfed_feeder::feed::Durability;
    use wellfed_fw_feeder::feeding::Record;
    use wellfed_sdk::hal::interlock::{self, Held};
    use wellfed_sdk::hal::owner::{FeedTrace, OwnerFault, OwnerState, SubmitResult};
    use wellfed_sdk::hal::supervision::events::EventsFault;

    fn decodes(record: Record, expected: Option<Event<'_>>) {
        let line = format!("{} {record}", Tag::Owner);
        let event = Line::parse(&line).and_then(|line| line.event());
        assert_eq!(event, expected, "{line}");
    }

    decodes(
        Record::Status {
            state: OwnerState::Feeding,
            fault: OwnerFault::Ok,
            supervisor: EventsFault::Ok,
            stopped: true,
            optics_off: false,
        },
        Some(Event::Owner(Owner::State {
            state: OwnerState::Feeding,
            stopped: true,
        })),
    );
    decodes(
        Record::Submitted {
            feed: 12,
            portions: 2,
            result: SubmitResult::Enqueued,
        },
        Some(Event::Owner(Owner::Submitted {
            id: 12,
            accepted: true,
        })),
    );
    decodes(
        Record::Submitted {
            feed: 12,
            portions: 2,
            result: SubmitResult::RejectPhase,
        },
        Some(Event::Owner(Owner::Submitted {
            id: 12,
            accepted: false,
        })),
    );
    decodes(
        Record::Held {
            portions: 2,
            held: Held::Joining,
        },
        Some(Event::Owner(Owner::Held)),
    );
    decodes(
        Record::Fed {
            result: OwnerResult {
                id: 12,
                requested: 2,
                completed: 1,
                outcome: OwnerOutcome::Cancelled,
                reason: FeedReason::Cancelled,
                motion_possible: false,
            },
            durability: Durability::Durable,
            trace: FeedTrace::default(),
        },
        Some(Event::Owner(Owner::Fed {
            id: 12,
            requested: 2,
            completed: 1,
            outcome: Some(OwnerOutcome::Cancelled),
            reason: Some(FeedReason::Cancelled),
        })),
    );
    let counters = interlock::Status {
        deferred: 2,
        writes_deferred: 3,
        ..interlock::Status::default()
    };
    decodes(
        Record::Writes(counters),
        Some(Event::Owner(Owner::Interlock {
            writes_deferred: Some(3),
        })),
    );
    decodes(Record::Joins(counters), None);
    decodes(Record::Refused, None);
    decodes(
        Record::Marks {
            feed: 12,
            homed_ms: Some(3_990),
            outlet_breaks: 4,
            phase: wellfed_core::feed::storage::RotorPhase::AtMark,
        },
        None,
    );
    decodes(
        Record::Current {
            feed: 12,
            trace: FeedTrace {
                current_max: 168,
                current_sum: 600,
                current_samples: 10,
                ..FeedTrace::default()
            },
        },
        None,
    );
}

/// A finished feed's result and its drive's figures, as the loop logs them
/// one after the other.
#[test]
fn a_feeds_result_and_its_current_are_logged() {
    use wellfed_core::feed::controller::FeedReason;
    use wellfed_core::feed::storage::{OwnerOutcome, OwnerResult};
    use wellfed_feeder::feed::Durability;
    use wellfed_fw_feeder::feeding::Record;
    use wellfed_sdk::hal::owner::FeedTrace;

    let trace = FeedTrace {
        drive_ms: 9_000,
        rising: 3,
        falling: 3,
        first_edge_ms: Some(7_840),
        current_max: 168,
        current_sum: 17_700,
        current_samples: 300,
        reversals: 1,
        hopper: Some(true),
        ..FeedTrace::default()
    };
    let fed = Record::Fed {
        result: OwnerResult {
            id: 7,
            requested: 3,
            completed: 3,
            outcome: OwnerOutcome::Done,
            reason: FeedReason::Done,
            motion_possible: false,
        },
        durability: Durability::Durable,
        trace,
    };
    assert_eq!(
        fed.to_string(),
        "fed id=7 requested=3 completed=3 outcome=1 reason=1 durable=1 drive_ms=9000 \
         rising=3 falling=3"
    );

    let current = Record::Current { feed: 7, trace };
    assert_eq!(
        current.to_string(),
        "current id=7 first_edge_ms=7840 current_max=168 current_mean=59 samples=300 \
         reversals=1 hopper=present"
    );

    // A drive that never ran measured nothing.
    let none = Record::Current {
        feed: 8,
        trace: FeedTrace::default(),
    };
    assert_eq!(
        none.to_string(),
        "current id=8 first_edge_ms=none current_max=0 current_mean=none samples=0 \
         reversals=0 hopper=unread"
    );
}

/// Every record the owner's loop logs fits the log's line, whatever its
/// figures: a line that does not is dropped whole.
#[test]
fn a_feeds_records_fit_a_log_line_at_their_widest() {
    use wellfed_core::feed::controller::FeedReason;
    use wellfed_core::feed::storage::{OwnerOutcome, OwnerResult, RotorPhase};
    use wellfed_core::record::Tag;
    use wellfed_core::store::feed_journal::JournalError;
    use wellfed_feeder::feed::Durability;
    use wellfed_fw_feeder::feeding::Record;
    use wellfed_sdk::hal::interlock::{self, Held};
    use wellfed_sdk::hal::owner::{FeedTrace, OwnerFault, OwnerState, SubmitResult};
    use wellfed_sdk::hal::supervision::events::{EDGE_LOG, EventsFault, PinWatch};

    // The log's buffer, which holds the line, its CRLF and a NUL.
    const LINE: usize = 192;
    let line = |record: Record| format!("{} {record}\r\n", Tag::Owner);

    let mut pins = PinWatch::default();
    pins.start = Some([u32::MAX; 2]);
    pins.changed = [u32::MAX; 2];
    pins.toggles = [[u8::MAX; 32]; 2];
    // The widest listing: ten two-digit pads, then more.
    pins.toggles[0][..10].fill(0);
    let trace = FeedTrace {
        drive_ms: u32::MAX,
        rising: u16::MAX,
        falling: u16::MAX,
        first_edge_ms: Some(u32::MAX),
        current_max: u32::MAX,
        current_sum: u32::MAX,
        current_samples: 1,
        reversals: u8::MAX,
        hopper: Some(true),
        edge_ms: [Some((u32::MAX, true)); EDGE_LOG],
        pins,
        homed_ms: Some(u32::MAX),
    };
    let result = OwnerResult {
        id: u64::MAX,
        requested: u16::MAX,
        completed: u16::MAX,
        outcome: OwnerOutcome::Uncertain,
        reason: FeedReason::MarkStuck,
        motion_possible: true,
    };
    let feed = u64::MAX;
    let interlock = interlock::Status {
        refused: u32::MAX,
        deferred: u32::MAX,
        forced: u32::MAX,
        waited_max_ms: u32::MAX,
        long_joins: u32::MAX,
        writes_deferred: u32::MAX,
        writes_forced: u32::MAX,
        write_waited_max_ms: u32::MAX,
        feeds_delayed: u32::MAX,
    };

    let widest = [
        Record::Journal {
            error: JournalError::Worn,
            created: true,
        },
        Record::Refused,
        Record::Opened {
            created: true,
            high_water: u64::MAX,
        },
        Record::Held {
            portions: u8::MAX,
            held: Held::Writing,
        },
        Record::Status {
            state: OwnerState::Fault,
            fault: OwnerFault::Ok,
            supervisor: EventsFault::Ok,
            stopped: true,
            optics_off: true,
        },
        Record::Submitted {
            feed,
            portions: u8::MAX,
            result: SubmitResult::RejectStopped,
        },
        Record::Fed {
            result,
            durability: Durability::Durable,
            trace,
        },
        // The widest mean, over one sample, and the most samples.
        Record::Current { feed, trace },
        Record::Current {
            feed,
            trace: FeedTrace {
                current_samples: u16::MAX,
                ..trace
            },
        },
        Record::Edges { feed, trace },
        Record::Steps {
            feed,
            max_gap_ms: u32::MAX,
            handshakes: u32::MAX,
        },
        Record::Marks {
            feed,
            homed_ms: Some(u32::MAX),
            outlet_breaks: u32::MAX,
            phase: RotorPhase::Unknown,
        },
        Record::Pins { feed, pins },
        Record::Recover { held_ms: u32::MAX },
        Record::Joins(interlock),
        Record::Writes(interlock),
    ];
    for record in widest {
        let line = line(record);
        assert!(line.len() < LINE, "{} bytes: {line}", line.len());
    }
}

/// The marks record says what a feed's marks and outlet beam showed, for
/// the owner's calibration runs.
#[test]
fn a_feeds_marks_and_outlet_breaks_are_logged() {
    use wellfed_core::feed::storage::RotorPhase;
    use wellfed_fw_feeder::feeding::{Record, outlet_breaks};
    use wellfed_sdk::hal::supervision::events::PinWatch;

    let marks = Record::Marks {
        feed: 7,
        homed_ms: None,
        outlet_breaks: 3,
        phase: RotorPhase::Unknown,
    };
    assert_eq!(
        marks.to_string(),
        "marks id=7 homed_ms=none outlet_breaks=3 phase=unknown"
    );
    let homed = Record::Marks {
        feed: 8,
        homed_ms: Some(3_990),
        outlet_breaks: 0,
        phase: RotorPhase::AtMark,
    };
    assert_eq!(
        homed.to_string(),
        "marks id=8 homed_ms=3990 outlet_breaks=0 phase=at_mark"
    );

    // Each break is two changes of PA16; one cut at either end counts.
    let mut pins = PinWatch::default();
    assert_eq!(outlet_breaks(&pins), 0);
    pins.toggles[0][16] = 8;
    assert_eq!(outlet_breaks(&pins), 4);
    pins.toggles[0][16] = 7;
    assert_eq!(outlet_breaks(&pins), 4);
    pins.toggles[0][17] = 9;
    pins.toggles[1][16] = 9;
    assert_eq!(outlet_breaks(&pins), 4, "only port A's pad 16");
}
