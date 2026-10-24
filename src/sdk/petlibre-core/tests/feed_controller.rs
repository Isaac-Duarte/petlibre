//! Behavior tests for the feeding state machine: feedback inhibit windows,
//! double-bowl direction pauses, bounded reversals, deadlines, input faults
//! and clock wrap.
//!
//! A fault latches the motor at STOP until an explicit reset, and a request
//! ID is never accepted twice. The limits below are compressed test
//! intervals, not hardware settings.

use petlibre_core::feed::controller::{
    FeedController, FeedLimits, FeedReason, FeedRequest, FeedSample, FeedStartResult, FeedState,
    MotorCommand,
};
use petlibre_core::feed::progress::{BowlMode, Direction};

/// Compressed test intervals; these are not deployable hardware settings.
const LIMITS: FeedLimits = FeedLimits {
    feedback_inhibit_ms: 5,
    feedback_timeout_ms: 30,
    direction_pause_ms: 3,
    reverse_ms: 3,
    max_duration_ms: 120,
    max_observation_gap_ms: 2,
    current_limit_ma: 300,
    max_portions: 50,
    max_reverse_attempts: 2,
};

const HEALTHY: FeedSample = FeedSample {
    power_healthy: true,
    optical_valid: true,
    outlet_clear: true,
    current_valid: true,
    current_ma: 100,
    feedback_edge: false,
    feedback_high: false,
};

struct Fixture {
    controller: FeedController,
    sample: FeedSample,
    now: u32,
}

fn request(id: u64, portions: u16, mode: BowlMode, direction: Direction) -> FeedRequest {
    FeedRequest {
        id,
        portions,
        mode,
        direction,
        credit_next_event: true,
        phase_known: true,
    }
}

fn begin_with(
    settings: &FeedLimits,
    mode: BowlMode,
    direction: Direction,
    portions: u16,
    now: u32,
) -> Fixture {
    let mut f = Fixture {
        controller: FeedController::new(settings, 0).unwrap(),
        sample: HEALTHY,
        now,
    };

    let r = request(1, portions, mode, direction);
    assert_eq!(
        f.controller.start(&r, now, &f.sample),
        FeedStartResult::Accepted
    );

    f
}

fn begin(mode: BowlMode, direction: Direction, portions: u16) -> Fixture {
    begin_with(&LIMITS, mode, direction, portions, 0)
}

impl Fixture {
    fn advance(&mut self, elapsed: u32) {
        for _ in 0..elapsed {
            self.now = self.now.wrapping_add(1);
            self.controller.update(self.now, Some(&self.sample));
        }
    }

    fn edge(&mut self, high: bool) {
        let sample = FeedSample {
            feedback_edge: true,
            feedback_high: high,
            ..self.sample
        };
        self.controller.update(self.now, Some(&sample));
    }

    fn expect_fault(&self, reason: FeedReason) {
        assert_eq!(self.controller.state, FeedState::Fault);
        assert_eq!(self.controller.reason, reason);
        assert_eq!(self.controller.motor, MotorCommand::Stop);
    }
}

#[test]
fn single_feedback_and_terminal_events() {
    let mut f = begin(BowlMode::Single, Direction::D0, 2);
    let next = request(2, 1, BowlMode::Single, Direction::D0);

    assert!(!f.controller.reset());
    assert_eq!(
        f.controller.start(&next, f.now, &f.sample),
        FeedStartResult::RejectBusy
    );
    assert_eq!(f.controller.last_request_id, 1);

    f.edge(true);
    assert_eq!(f.controller.progress.completed, 0);
    f.advance(5);
    f.edge(false);
    assert_eq!(f.controller.progress.completed, 0);

    f.edge(true);
    assert_eq!(f.controller.progress.completed, 1);
    f.edge(true);
    assert_eq!(f.controller.progress.completed, 1);

    f.advance(5);
    f.edge(true);
    assert_eq!(f.controller.state, FeedState::Complete);
    assert_eq!(f.controller.motor, MotorCommand::Stop);
    assert_eq!(f.controller.reason, FeedReason::Done);

    f.edge(true);
    f.advance(100);
    assert_eq!(f.controller.progress.completed, 2);
    assert_eq!(f.controller.motor, MotorCommand::Stop);
}

#[test]
fn double_bowl_pause_and_direction() {
    let mut f = begin(BowlMode::Double, Direction::D0, 3);

    f.advance(5);
    f.edge(true);
    assert_eq!(f.controller.progress.completed, 1);
    assert_eq!(f.controller.motor, MotorCommand::Drive1);

    f.advance(5);
    f.edge(true); // Wrong polarity cannot advance double-bowl progress.
    assert_eq!(f.controller.progress.pending_events, 0);
    f.edge(false);
    assert_eq!(f.controller.progress.completed, 1);
    f.advance(5);
    f.edge(false);
    assert_eq!(f.controller.progress.completed, 2);
    assert_eq!(f.controller.state, FeedState::DirectionPause);
    assert_eq!(f.controller.motor, MotorCommand::Stop);

    f.edge(true);
    f.advance(3);
    assert_eq!(f.controller.motor, MotorCommand::Drive0);
    assert_eq!(f.controller.inhibit_at, f.now); // Resume re-arms inhibition.

    f.advance(2);
    f.edge(true); // Too soon after resuming drive.
    assert_eq!(f.controller.progress.pending_events, 0);
    f.advance(3);
    f.edge(true);
    assert_eq!(f.controller.progress.completed, 2);

    f.advance(5);
    f.edge(true);
    assert_eq!(f.controller.progress.completed, 3);
    assert_eq!(f.controller.state, FeedState::Complete);
    assert_eq!(f.controller.motor, MotorCommand::Stop);
    assert_eq!(f.controller.progress.direction, Direction::D0);
}

#[test]
fn missing_feedback_latches_stop() {
    let mut f = begin(BowlMode::Single, Direction::D0, 2);

    f.advance(30);
    f.expect_fault(FeedReason::FeedbackTimeout);

    f.edge(true);
    assert_eq!(f.controller.progress.completed, 0);

    let mut next = request(2, 1, BowlMode::Single, Direction::D0);
    assert_eq!(
        f.controller.start(&next, f.now, &f.sample),
        FeedStartResult::RejectResetRequired
    );

    assert!(f.controller.reset());
    assert_eq!(f.controller.last_request_id, 1);
    next.id = 1;
    assert_eq!(
        f.controller.start(&next, f.now, &f.sample),
        FeedStartResult::RejectId
    );
}

#[test]
fn total_deadline_wins_over_feedback() {
    let settings = FeedLimits {
        feedback_timeout_ms: 20,
        max_duration_ms: 30,
        ..LIMITS
    };
    let mut f = begin_with(&settings, BowlMode::Single, Direction::D0, 20, 0);

    for _ in 0..5 {
        f.advance(5);
        f.edge(true);
    }
    assert_eq!(f.controller.progress.completed, 5);

    f.advance(5);
    f.edge(true);
    f.expect_fault(FeedReason::TotalTimeout);
    assert_eq!(f.controller.progress.completed, 5);
}

#[test]
fn reverse_attempts_are_bounded() {
    let mut f = begin(BowlMode::Single, Direction::D0, 3);

    f.sample.current_ma = 301;
    f.advance(1);
    assert_eq!(f.controller.state, FeedState::Reversing);
    assert_eq!(f.controller.motor, MotorCommand::Drive1);
    f.edge(true);
    assert_eq!(f.controller.progress.completed, 0);

    f.sample.current_ma = 100;
    f.advance(3);
    assert_eq!(f.controller.motor, MotorCommand::Drive0);

    f.advance(4);
    f.edge(true);
    assert_eq!(f.controller.progress.completed, 0);
    f.advance(1);
    f.edge(true);
    assert_eq!(f.controller.progress.completed, 1);

    f.sample.current_ma = 301;
    f.advance(1);
    assert_eq!(f.controller.reverse_attempts, 2);
    f.sample.current_ma = 100;
    f.advance(3);

    f.sample.current_ma = 301;
    f.advance(1);
    f.expect_fault(FeedReason::Overcurrent);
    assert_eq!(f.controller.reverse_attempts, 2);
}

#[test]
fn persistent_overcurrent_does_not_resume_forward() {
    let mut f = begin(BowlMode::Single, Direction::D0, 3);
    f.sample.current_ma = 301;
    f.advance(1);
    f.advance(3);
    f.expect_fault(FeedReason::Overcurrent);
    assert_eq!(f.controller.reverse_attempts, 1);
}

#[test]
fn input_faults_in_every_motion_state() {
    for state in 0..3 {
        for fault in 0..4 {
            let mut f = begin(BowlMode::Double, Direction::D1, 3);
            if state == 1 {
                f.advance(5);
                f.edge(false);
                assert_eq!(f.controller.state, FeedState::DirectionPause);
            } else if state == 2 {
                f.sample.current_ma = 301;
                f.advance(1);
                f.sample.current_ma = 100;
                assert_eq!(f.controller.state, FeedState::Reversing);
            }

            let reason = match fault {
                0 => {
                    f.sample.power_healthy = false;
                    FeedReason::Power
                }
                1 => {
                    f.sample.outlet_clear = false;
                    FeedReason::Outlet
                }
                2 => {
                    f.sample.current_valid = false;
                    FeedReason::Sensor
                }
                _ => {
                    f.sample.optical_valid = false;
                    FeedReason::Sensor
                }
            };

            f.advance(1);
            f.expect_fault(reason);
            f.sample = HEALTHY;
            f.advance(10);
            f.expect_fault(reason);
        }
    }
}

#[test]
fn clock_wrap_and_bad_timestamps() {
    let mut f = begin_with(&LIMITS, BowlMode::Single, Direction::D0, 1, u32::MAX - 3);
    f.advance(5);
    f.edge(true);
    assert_eq!(f.now, 1);
    assert_eq!(f.controller.state, FeedState::Complete);

    let mut f = begin_with(&LIMITS, BowlMode::Single, Direction::D0, 1, 100);
    f.controller.update(99, Some(&f.sample));
    f.expect_fault(FeedReason::Clock);

    let mut f = begin(BowlMode::Single, Direction::D0, 1);
    f.controller.update(3, Some(&f.sample));
    f.expect_fault(FeedReason::ObservationGap);
}

#[test]
fn cancellation_and_id_replay() {
    let mut f = begin(BowlMode::Single, Direction::D0, 2);

    assert!(!f.controller.cancel(0));
    assert!(f.controller.cancel(1));
    assert_eq!(f.controller.state, FeedState::Cancelled);
    assert_eq!(f.controller.motor, MotorCommand::Stop);
    assert!(f.controller.reset());

    let next = request(2, 1, BowlMode::Single, Direction::D0);
    assert_eq!(
        f.controller.start(&next, f.now, &f.sample),
        FeedStartResult::Accepted
    );

    assert!(!f.controller.cancel(1));
    assert_eq!(f.controller.state, FeedState::Running);

    let mut restored = FeedController::new(&LIMITS, 2).unwrap();
    assert_eq!(restored.motor, MotorCommand::Stop);
    assert_eq!(
        restored.start(&next, 0, &HEALTHY),
        FeedStartResult::RejectId
    );
}

#[test]
fn invalid_configuration_and_requests() {
    assert!(
        FeedController::new(
            &FeedLimits {
                max_duration_ms: 0x8000_0000,
                ..LIMITS
            },
            0
        )
        .is_none()
    );

    assert!(
        FeedController::new(
            &FeedLimits {
                feedback_timeout_ms: LIMITS.feedback_inhibit_ms,
                ..LIMITS
            },
            0
        )
        .is_none()
    );

    assert!(
        FeedController::new(
            &FeedLimits {
                max_portions: 0,
                ..LIMITS
            },
            0
        )
        .is_none()
    );

    let mut controller = FeedController::new(&LIMITS, 0).unwrap();
    let mut r = request(1, 0, BowlMode::Single, Direction::D0);
    assert_eq!(
        controller.start(&r, 0, &HEALTHY),
        FeedStartResult::RejectRequest
    );

    r.portions = 51;
    assert_eq!(
        controller.start(&r, 0, &HEALTHY),
        FeedStartResult::RejectRequest
    );

    r = request(1, 1, BowlMode::Double, Direction::D0);
    r.phase_known = false;
    assert_eq!(
        controller.start(&r, 0, &HEALTHY),
        FeedStartResult::RejectRequest
    );

    r = request(1, 1, BowlMode::Single, Direction::D1);
    assert_eq!(
        controller.start(&r, 0, &HEALTHY),
        FeedStartResult::RejectRequest
    );

    r.direction = Direction::D0;
    let bad_sample = FeedSample {
        current_valid: false,
        ..HEALTHY
    };
    assert_eq!(
        controller.start(&r, 0, &bad_sample),
        FeedStartResult::RejectInput
    );
    assert_eq!(controller.last_request_id, 0);
    assert_eq!(controller.motor, MotorCommand::Stop);
}

#[test]
fn pause_completion_cannot_consume_old_edges() {
    let settings = FeedLimits {
        direction_pause_ms: 10,
        ..LIMITS
    };
    let mut f = begin_with(&settings, BowlMode::Double, Direction::D1, 3, 0);

    f.advance(5);
    f.edge(false);
    assert_eq!(f.controller.state, FeedState::DirectionPause);
    assert_eq!(f.controller.motor, MotorCommand::Stop);

    f.advance(10);
    assert_eq!(f.controller.state, FeedState::Running);
    assert_eq!(f.controller.motor, MotorCommand::Drive0);
    assert_eq!(f.controller.inhibit_at, 15);

    // Captured while stopped, past the old inhibit window, delivered after resume.
    let edge = FeedSample {
        feedback_edge: true,
        feedback_high: true,
        ..f.sample
    };
    f.controller.update_captured(f.now, Some(&edge), 11);
    assert_eq!(f.controller.progress.pending_events, 0);
    assert_eq!(f.controller.last_feedback_at, 5);
    assert_eq!(f.controller.state, FeedState::Running);

    // A fresh edge after the re-armed window still counts.
    f.advance(5);
    f.edge(true);
    assert_eq!(f.controller.progress.pending_events, 1);
}

#[test]
fn reversals_do_not_extend_feedback_deadline() {
    let mut f = begin(BowlMode::Single, Direction::D0, 2);
    f.sample.current_ma = 301;
    f.advance(1);
    f.sample.current_ma = 100;
    f.advance(3);
    f.sample.current_ma = 301;
    f.advance(1);
    f.sample.current_ma = 100;
    f.advance(25);

    f.expect_fault(FeedReason::FeedbackTimeout);
}

#[test]
fn stopped_outputs_do_not_resume_on_bad_current() {
    let mut f = begin(BowlMode::Double, Direction::D1, 3);
    f.advance(5);
    f.edge(false);
    assert_eq!(f.controller.motor, MotorCommand::Stop);
    f.sample.current_ma = 301;
    f.advance(1);
    f.expect_fault(FeedReason::Overcurrent);

    let settings = FeedLimits {
        max_reverse_attempts: 0,
        ..LIMITS
    };
    let mut f = begin_with(&settings, BowlMode::Single, Direction::D0, 3, 0);
    f.sample.current_ma = 301;
    f.advance(1);
    f.expect_fault(FeedReason::Overcurrent);
    assert_eq!(f.controller.reverse_attempts, 0);

    let mut f = begin(BowlMode::Single, Direction::D0, 3);
    f.controller.update(1, None);
    f.expect_fault(FeedReason::Sensor);
}
