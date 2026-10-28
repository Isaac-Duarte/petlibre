//! The latest feed's trace between the loop and the settings writer: who
//! holds it at each step, what a read answers, and what counts as a trace
//! that never reached storage.

use wellfed_core::feed::controller::MotorCommand;
use wellfed_core::feed::observe::Observation;
use wellfed_feeder::trace::DriveFigures;
use wellfed_feeder_runtime::traces;
use wellfed_proto::api::{EventBody, FeedOutcome, FeedReason, FoodLevel, Refusal, Source};

fn fed(feed: u32) -> EventBody {
    EventBody::Fed {
        feed,
        requested: 1,
        completed: 1,
        outcome: FeedOutcome::Done,
        reason: FeedReason::Done,
        hopper: FoodLevel::Present,
        by: Source::Panel,
    }
}

fn poll(at_ms: u32) -> Observation {
    Observation {
        at_ms,
        drive_at_ms: Some(0),
        current: Some(60),
        motor: MotorCommand::Drive0,
        ..Observation::default()
    }
}

/// One test: the hand-offs are process-wide.
#[test]
fn a_trace_passes_from_the_loop_to_the_writer_and_back() {
    traces::reset();
    let mut out = [0; 64];
    assert_eq!(traces::page(1, 0, &mut out), None, "nothing recorded");

    assert!(traces::begin(1));
    traces::observe(&poll(0));
    traces::observe(&poll(30));
    assert_eq!(traces::page(1, 0, &mut out), Some(Err(Refusal::Busy)));
    assert_eq!(traces::page(2, 0, &mut out), None, "another feed's");
    assert!(!traces::queued());
    assert_eq!(traces::seal(&fed(2), &DriveFigures::default()), None);
    let summary = traces::seal(&fed(1), &DriveFigures::default()).unwrap();
    assert!(traces::queued());
    traces::observe(&poll(60));

    let read = traces::page(1, 0, &mut out).unwrap().unwrap();
    // The motor's start and two readings, two bytes each.
    assert_eq!((read.summary, read.stored, read.len), (summary, false, 6));

    // The writer stores it; reads go on, now stored.
    let written = traces::write_sealed(|stored, body| {
        assert_eq!((*stored, body.len()), (summary, 6));
        true
    });
    assert_eq!(written, Some((1, true)));
    assert!(!traces::queued());
    assert_eq!(traces::write_sealed(|_, _| true), None, "nothing waits");
    assert!(traces::page(1, 0, &mut out).unwrap().unwrap().stored);
    assert_eq!(traces::fill().unsaved, 0);

    // A sealed trace the writer never reached is lost to the next feed,
    // and counted; one that never got a result is not.
    assert!(traces::begin(2));
    traces::seal(&fed(2), &DriveFigures::default()).unwrap();
    assert!(traces::begin(3));
    assert_eq!(traces::fill().unsaved, 1);
    assert!(traces::begin(4));
    assert_eq!(traces::fill().unsaved, 1);

    // A failed append is counted, and the trace still reads from RAM.
    traces::seal(&fed(4), &DriveFigures::default()).unwrap();
    assert_eq!(traces::write_sealed(|_, _| false), Some((4, false)));
    assert_eq!(traces::fill().unsaved, 2);
    let read = traces::page(4, 0, &mut out).unwrap().unwrap();
    assert!(!read.stored);

    // While the writer appends, the next feed goes untraced rather than
    // wait.
    assert!(traces::begin(5));
    traces::seal(&fed(5), &DriveFigures::default()).unwrap();
    traces::write_sealed(|_, _| {
        assert!(!traces::begin(6));
        true
    });
    assert_eq!(
        traces::page(5, 0, &mut out).map(|r| r.map(|r| r.stored)),
        Some(Ok(true))
    );
    traces::reset();
}
