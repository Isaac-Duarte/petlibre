//! Tests for the bounded feedback-edge queue and its handoff to the feeding
//! controller: FIFO wrap, capture-time inhibition, overflow and stale
//! batches, clock errors, drive continuity and the motor-gate pipeline.
//!
//! An edge is credited at most once and judged by its capture time, not by
//! when it is processed. An invalid batch credits nothing and faults the feed
//! to STOP.

use core::ffi::c_void;

use wellfed_core::feed::controller::{
    FeedController, FeedLimits, FeedReason, FeedRequest, FeedSample, FeedStartResult, FeedState,
    MotorCommand,
};
use wellfed_core::feed::feedback::{
    CAPACITY, FeedbackDrive, FeedbackEvent, FeedbackEvents, FeedbackFault, FeedbackTakeResult,
};
use wellfed_core::feed::motor_gate::{MotorGate, MotorGateFault, MotorPins};
use wellfed_core::feed::progress::{BowlMode, Direction};

/// Compressed intervals; none is a deployment default.
const LIMITS: FeedLimits = FeedLimits {
    feedback_inhibit_ms: 5,
    feedback_timeout_ms: 30,
    direction_pause_ms: 3,
    reverse_ms: 3,
    max_duration_ms: 120,
    max_observation_gap_ms: 4,
    current_limit_ma: 300,
    max_portions: 5,
    max_reverse_attempts: 2,
};

struct Fixture {
    events: FeedbackEvents,
    controller: FeedController,
    sample: FeedSample,
    drive: FeedbackDrive,
    now: u32,
}

fn begin_mode(mode: BowlMode, now: u32) -> Fixture {
    let sample = FeedSample {
        power_healthy: true,
        optical_valid: true,
        outlet_clear: true,
        current_valid: true,
        current_ma: 100,
        ..FeedSample::default()
    };

    let mut f = Fixture {
        events: FeedbackEvents::new(32, 2, now).unwrap(),
        controller: FeedController::new(&LIMITS, 0).unwrap(),
        sample,
        drive: FeedbackDrive {
            healthy: true,
            applied_known: true,
            applied: MotorCommand::Drive0,
            applied_at: now,
        },
        now,
    };

    let request = FeedRequest {
        id: 1,
        portions: 2,
        mode,
        direction: Direction::D0,
        credit_next_event: true,
        phase_known: true,
    };
    assert_eq!(
        f.controller.start(&request, now, &f.sample),
        FeedStartResult::Accepted
    );
    f
}

fn begin() -> Fixture {
    begin_mode(BowlMode::Single, 0)
}

impl Fixture {
    fn process(&mut self, now: u32) -> bool {
        self.now = now;
        self.events
            .process(&mut self.controller, now, &self.sample, self.drive)
    }

    fn advance(&mut self, elapsed: u32) {
        for _ in 0..elapsed {
            assert!(self.process(self.now.wrapping_add(1)));
        }
    }

    fn fault(&self, reason: FeedReason) {
        assert_eq!(self.controller.state, FeedState::Fault);
        assert_eq!(self.controller.reason, reason);
        assert_eq!(self.controller.motor, MotorCommand::Stop);
    }
}

fn event(captured_at: u32, high: bool) -> (FeedbackTakeResult, Option<FeedbackEvent>) {
    (
        FeedbackTakeResult::Event,
        Some(FeedbackEvent { captured_at, high }),
    )
}

const EMPTY: (FeedbackTakeResult, Option<FeedbackEvent>) = (FeedbackTakeResult::Empty, None);

#[test]
fn fifo_wrap_and_exactly_once() {
    let mut q = FeedbackEvents::new(3, 10, 0).unwrap();
    assert_eq!(q.take(0), EMPTY);

    assert!(q.push(1, true));
    assert!(q.push(2, false));
    assert_eq!(q.take(2), event(1, true));
    assert!(q.push(3, true));
    assert!(q.push(4, false));

    for stamp in 2..=4 {
        assert_eq!(q.take(4), event(stamp, stamp == 3));
    }

    assert_eq!(q.take(4), EMPTY);
    assert_eq!(q.count, 0);
}

#[test]
fn delayed_inhibited_edges_never_become_valid() {
    let mut f = begin();
    f.advance(3);

    assert!(f.events.push(4, true));
    assert!(f.process(6)); // Old controller API would qualify processing time.
    assert!(f.controller.progress.completed == 0 && f.events.count == 0);

    assert!(f.events.push(7, true));
    assert!(f.process(7));
    assert_eq!(f.controller.progress.completed, 1);

    assert!(f.process(8));
    assert_eq!(f.controller.progress.completed, 1); // No replay on later polls.
}

#[test]
fn inhibition_retains_capture_time() {
    let mut f = begin();
    f.advance(4);

    assert!(f.events.push(5, true));
    assert!(f.process(7));
    assert!(f.controller.progress.completed == 1 && f.controller.inhibit_at == 5);
    assert_eq!(f.controller.last_feedback_at, 5);

    f.advance(2);
    assert!(f.events.push(10, true));
    assert!(f.process(11));
    assert!(f.controller.state == FeedState::Complete && f.controller.progress.completed == 2);
    assert!(f.controller.last_feedback_at == 10 && f.controller.motor == MotorCommand::Stop);
}

#[test]
fn full_batch_credits_at_most_once() {
    let mut f = begin();
    f.advance(4);
    for _ in 0..CAPACITY {
        assert!(f.events.push(5, true));
    }

    assert!(f.process(5));
    assert!(f.controller.progress.completed == 1 && f.events.count == 0);
}

#[test]
fn overflow_and_stale_batches_credit_nothing() {
    let mut f = begin();
    f.advance(4);
    for _ in 0..CAPACITY {
        assert!(f.events.push(5, true));
    }

    assert!(!f.events.push(5, true));
    assert!(!f.process(5));
    assert!(f.events.fault == FeedbackFault::Overflow && f.controller.progress.completed == 0);
    f.fault(FeedReason::FeedbackStream);

    assert!(!f.events.push(6, true));
    assert!(f.controller.reset());
    assert!(!f.process(6) && f.events.fault == FeedbackFault::Overflow);

    let mut f = begin();
    f.advance(3);
    assert!(f.events.push(4, true));
    assert!(f.events.push(5, true));
    assert!(!f.process(7)); // Newer valid edge cannot rescue an invalid batch.
    assert!(f.events.fault == FeedbackFault::Stale && f.controller.progress.completed == 0);
    f.fault(FeedReason::FeedbackStream);
}

#[test]
fn current_faults_and_deadlines_precede_old_feedback() {
    let mut f = begin();
    f.advance(4);
    assert!(f.events.push(5, true));
    f.sample.power_healthy = false;
    assert!(f.process(5));
    f.fault(FeedReason::Power);
    assert_eq!(f.controller.progress.completed, 0);

    let mut f = begin();
    f.advance(29);
    assert!(f.events.push(29, true));
    assert!(f.process(30));
    f.fault(FeedReason::FeedbackTimeout);
    assert_eq!(f.controller.progress.completed, 0);
}

#[test]
fn clock_errors_wrap_and_future_capture() {
    let mut q = FeedbackEvents::new(3, 3, u32::MAX - 2).unwrap();
    assert!(q.push(u32::MAX - 1, true));
    assert!(q.push(0, false));
    assert_eq!(q.take(1), event(u32::MAX - 1, true));
    assert_eq!(q.take(1), event(0, false));
    assert!(!q.push(u32::MAX, true));
    assert_eq!(q.fault, FeedbackFault::Clock);

    let mut f = begin();
    f.advance(4);
    assert!(f.events.push(6, true));
    assert!(!f.process(5));
    f.fault(FeedReason::FeedbackStream);

    let mut f = begin();
    let edge = FeedSample {
        feedback_edge: true,
        feedback_high: true,
        ..f.sample
    };
    f.controller.update_captured(1, Some(&edge), 2);
    f.fault(FeedReason::Clock);

    let mut f = begin_mode(BowlMode::Single, u32::MAX - 3);
    f.advance(4);
    assert!(f.events.push(1, true));
    assert!(f.process(2));
    assert!(f.controller.progress.completed == 1 && f.controller.inhibit_at == 1);
}

#[test]
fn drive_must_be_known_healthy_and_continuous() {
    for mode in 0..3 {
        let mut f = begin();
        f.advance(4);
        assert!(f.events.push(5, true));
        match mode {
            0 => f.drive.healthy = false,
            1 => f.drive.applied_known = false,
            _ => f.drive.applied = MotorCommand::Drive1,
        }
        assert!(!f.process(5));
        f.fault(FeedReason::Output);
        assert_eq!(f.controller.progress.completed, 0);
    }

    let mut f = begin();
    f.advance(4);
    f.drive.applied = MotorCommand::Stop;
    assert!(f.events.push(5, true));
    assert!(f.process(5));
    assert_eq!(f.controller.progress.completed, 0);

    assert!(f.events.push(6, true));
    f.drive.applied = MotorCommand::Drive0;
    f.drive.applied_at = 7;
    assert!(f.process(7));
    assert_eq!(f.controller.progress.completed, 0);

    assert!(f.events.push(8, true));
    assert!(f.process(8));
    assert_eq!(f.controller.progress.completed, 1);
}

#[test]
fn direction_handoff_discards_pre_drive_edges() {
    let mut f = begin_mode(BowlMode::Double, 0);
    f.advance(4);
    assert!(f.events.push(5, true));
    assert!(f.process(5));
    assert!(f.controller.motor == MotorCommand::Drive1 && f.controller.progress.completed == 1);

    f.drive.applied = MotorCommand::Stop;
    f.advance(4);
    assert!(f.events.push(10, false));
    f.drive.applied = MotorCommand::Drive1;
    f.drive.applied_at = 11;
    assert!(f.process(11));
    assert_eq!(f.controller.progress.pending_events, 0);

    assert!(f.events.push(12, false));
    assert!(f.process(12));
    assert!(f.controller.progress.pending_events == 1 && f.controller.progress.completed == 1);
}

#[test]
fn reverse_completion_cannot_consume_old_edges() {
    let mut f = begin();
    f.advance(3);

    f.sample.current_ma = 301;
    assert!(f.process(4));
    assert_eq!(f.controller.state, FeedState::Reversing);

    f.sample.current_ma = 100;
    f.drive.applied = MotorCommand::Drive1;
    f.drive.applied_at = 5;
    f.advance(2);

    assert!(f.events.push(7, true));
    assert!(f.process(7));
    assert!(f.controller.state == FeedState::Running && f.controller.progress.completed == 0);
    assert!(f.controller.inhibit_at == 7 && f.events.count == 0);
}

#[test]
fn invalid_configuration_and_external_trips() {
    assert!(FeedbackEvents::new(0, 2, 0).is_none());
    assert!(FeedbackEvents::new(33, 2, 0).is_none());
    assert!(FeedbackEvents::new(1, 0, 0).is_none());
    assert!(FeedbackEvents::new(1, 0x8000_0000, 0).is_none());

    let mut q = FeedbackEvents::default();
    assert!(!q.push(0, true));
    assert_eq!(q.fault, FeedbackFault::Argument);

    let mut f = begin();
    f.events = FeedbackEvents::new(2, 5, 0).unwrap();
    assert!(!f.process(0));
    f.fault(FeedReason::FeedbackStream);

    let mut f = begin();
    assert!(!f.controller.trip(FeedReason::Done));
    assert!(!f.controller.trip(FeedReason::None));
    assert!(!f.controller.trip(FeedReason::Cancelled));
    assert!(f.controller.trip(FeedReason::Output));
    assert!(!f.controller.trip(FeedReason::Sensor));
    f.fault(FeedReason::Output);
}

#[derive(Default)]
struct Output {
    a: bool,
    b: bool,
}

unsafe extern "C" fn write_pair(context: *mut c_void, a: bool, b: bool) -> bool {
    // SAFETY: `context` is the fixture's leaked pins, unaliased during the callback.
    let pins = unsafe { &mut *context.cast::<Output>() };
    assert!(a || b);
    pins.a = a;
    pins.b = b;
    true
}

/// The fixture's pins, read through the pointer between gate calls.
struct Pins(*mut Output);

impl MotorPins for Pins {
    fn write_pair(&mut self, a: bool, b: bool) -> bool {
        // SAFETY: every test keeps the pins alive for the gate's lifetime.
        unsafe { write_pair(self.0.cast(), a, b) }
    }
}

#[test]
fn pipeline_applies_gate_output_and_stops() {
    let mut f = begin();
    let pins: *mut Output = Box::into_raw(Box::default());
    let mut gate = MotorGate::default();
    assert!(gate.init(Pins(pins), 2, 4, 0));

    for now in 0..=6 {
        if now == 5 {
            assert!(f.events.push(now, true));
        }

        f.drive = FeedbackDrive {
            healthy: gate.fault == MotorGateFault::Ok,
            applied_known: gate.applied_known,
            applied: gate.applied,
            applied_at: gate.applied_at,
        };

        assert!(f.process(now));
        assert!(gate.request(f.controller.motor, now));
        assert!(gate.tick(now));
    }
    // SAFETY: the leaked pins outlive the fixture and nothing else borrows them here.
    assert!(f.controller.progress.completed == 1 && !unsafe { &*pins }.b);

    // An ingestion overflow must cause the owner to apply STOP, not continue.
    for _ in 0..CAPACITY {
        assert!(f.events.push(7, true));
    }
    assert!(!f.events.push(7, true));
    assert!(!f.process(7));
    assert!(gate.request(f.controller.motor, 7));

    // SAFETY: the box leaked when the fixture was built, freed once here.
    let pins = unsafe { Box::from_raw(pins) };
    assert!(pins.a && pins.b && gate.applied_known);
    f.fault(FeedReason::FeedbackStream);
}
