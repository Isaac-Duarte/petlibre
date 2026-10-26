//! Behavior tests for the feeding state machine: feedback inhibit windows,
//! double-bowl direction pauses, bounded reversals, deadlines, input faults
//! and clock wrap.
//!
//! A fault latches the motor at STOP until an explicit reset, and a request
//! ID is never accepted twice. The limits below are compressed test
//! intervals, not hardware settings.

use wellfed_core::feed::controller::{
    FeedController, FeedLimits, FeedReason, FeedRequest, FeedSample, FeedStartResult, FeedState,
    MotorCommand, OVERCURRENT_POLLS,
};
use wellfed_core::feed::progress::{BowlMode, Direction};

/// Compressed test intervals; these are not deployable hardware settings.
const LIMITS: FeedLimits = FeedLimits {
    feedback_inhibit_ms: 5,
    feedback_timeout_ms: 30,
    direction_pause_ms: 3,
    reverse_ms: 3,
    max_duration_ms: 120,
    portion_ms: 120,
    max_observation_gap_ms: 2,
    mark_gap_ms: 1000,
    mark_low_ms: 1000,
    current_limit_ma: 300,
    current_max_ma: 1000,
    surge_ms: 0,
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
    mark_low: false,
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
fn a_request_gets_its_portions_share_of_the_deadline() {
    let settings = FeedLimits {
        feedback_timeout_ms: 20,
        portion_ms: 12,
        ..LIMITS
    };

    // Two portions at 10 ms each fit 24 ms; a third at 10 ms would too, but
    // the request asked for two, so it ends Done before its deadline.
    let mut f = begin_with(&settings, BowlMode::Single, Direction::D0, 2, 0);
    for _ in 0..2 {
        f.advance(10);
        f.edge(true);
    }
    assert!(f.controller.state == FeedState::Complete && f.controller.deadline_ms == 24);

    // Every gap within the feedback timeout, but 15 ms a portion on average
    // overruns 3 x 12 ms.
    let mut f = begin_with(&settings, BowlMode::Single, Direction::D0, 3, 0);
    for _ in 0..2 {
        f.advance(15);
        f.edge(true);
    }
    f.advance(6);
    f.expect_fault(FeedReason::TotalTimeout);
    assert_eq!(f.controller.progress.completed, 2);

    // The cap bounds the share of a large request.
    let f = begin_with(
        &settings,
        BowlMode::Single,
        Direction::D0,
        LIMITS.max_portions,
        0,
    );
    assert_eq!(f.controller.deadline_ms, LIMITS.max_duration_ms);
}

#[test]
fn total_deadline_wins_over_feedback() {
    let settings = FeedLimits {
        feedback_timeout_ms: 20,
        max_duration_ms: 30,
        portion_ms: 30,
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
    f.advance(OVERCURRENT_POLLS.into());
    assert_eq!(f.controller.state, FeedState::Reversing);
    assert_eq!(f.controller.motor, MotorCommand::Drive1);
    f.edge(true);
    assert_eq!(f.controller.progress.completed, 0);

    // Backed off for `reverse_ms`, then stopped for the pause, then forward.
    f.sample.current_ma = 100;
    f.advance(3);
    assert_eq!(f.controller.state, FeedState::DirectionPause);
    assert_eq!(f.controller.motor, MotorCommand::Stop);
    f.advance(3);
    assert_eq!(f.controller.motor, MotorCommand::Drive0);
    assert_eq!(f.controller.inhibit_at, f.now);

    f.advance(4);
    f.edge(true);
    assert_eq!(f.controller.progress.completed, 0);
    f.advance(1);
    f.edge(true);
    assert_eq!(f.controller.progress.completed, 1);

    f.sample.current_ma = 301;
    f.advance(OVERCURRENT_POLLS.into());
    assert_eq!(f.controller.reverse_attempts, 2);
    f.sample.current_ma = 100;
    f.advance(6);
    assert_eq!(f.controller.state, FeedState::Running);

    f.sample.current_ma = 301;
    f.advance(OVERCURRENT_POLLS.into());
    f.expect_fault(FeedReason::Overcurrent);
    assert_eq!(f.controller.reverse_attempts, 2);
}

#[test]
fn the_surge_window_counts_toward_no_jam_but_the_ceiling_stops_at_once() {
    let settings = FeedLimits {
        surge_ms: 2,
        ..LIMITS
    };
    let mut f = begin_with(&settings, BowlMode::Single, Direction::D0, 3, 0);

    // Under the ceiling: the window's poll counts for nothing, the two
    // after it are a jam.
    f.sample.current_ma = 999;
    f.advance(1);
    assert!(f.controller.state == FeedState::Running && f.controller.over_limit == 0);
    f.advance(2);
    assert_eq!(f.controller.state, FeedState::Reversing);

    // Backing off is a change of drive: its own window.
    f.advance(1);
    assert!(f.controller.state == FeedState::Reversing && f.controller.over_limit == 0);

    // Over the ceiling, inside a window: stopped, with no reversal.
    let mut f = begin_with(&settings, BowlMode::Single, Direction::D0, 3, 0);
    f.sample.current_ma = 1001;
    f.advance(1);
    f.expect_fault(FeedReason::Overcurrent);
    assert_eq!(f.controller.reverse_attempts, 0);
}

#[test]
fn a_reversal_extends_the_deadline_and_the_feedback_window_by_its_cost() {
    // One portion: a 20 ms deadline; each reversal costs 3 + 3 + 3 = 9.
    let settings = FeedLimits {
        portion_ms: 20,
        ..LIMITS
    };
    assert_eq!(settings.reversal_ms(), 9);
    let mut f = begin_with(&settings, BowlMode::Single, Direction::D0, 1, 0);
    f.sample.current_ma = 301;
    f.advance(OVERCURRENT_POLLS.into());
    assert_eq!(f.controller.deadline_ms, 29);
    f.sample.current_ma = 100;
    f.advance(26);
    assert_eq!(f.controller.state, FeedState::Running);
    f.advance(1);
    f.expect_fault(FeedReason::TotalTimeout);

    // Never past the cap: two reversals would make it 38.
    let settings = FeedLimits {
        portion_ms: 20,
        max_duration_ms: 30,
        ..LIMITS
    };
    let mut f = begin_with(&settings, BowlMode::Single, Direction::D0, 1, 0);
    for _ in 0..2 {
        f.sample.current_ma = 301;
        f.advance(OVERCURRENT_POLLS.into());
        f.sample.current_ma = 100;
        f.advance(6);
    }
    assert_eq!(f.controller.deadline_ms, 30);

    // The feedback window grows the same way, until a portion is credited.
    let mut f = begin(BowlMode::Single, Direction::D0, 3);
    f.sample.current_ma = 301;
    f.advance(OVERCURRENT_POLLS.into());
    f.sample.current_ma = 100;
    f.advance(36);
    assert_eq!(f.controller.state, FeedState::Running);
    f.advance(1);
    f.expect_fault(FeedReason::FeedbackTimeout);

    let mut f = begin(BowlMode::Single, Direction::D0, 3);
    f.sample.current_ma = 301;
    f.advance(OVERCURRENT_POLLS.into());
    f.sample.current_ma = 100;
    f.advance(11);
    f.edge(true);
    assert_eq!(f.controller.progress.completed, 1);
    assert_eq!(f.controller.feedback_grace_ms, 0);
}

#[test]
fn a_drive_with_no_edge_for_the_mark_gap_has_lost_its_feedback() {
    let settings = FeedLimits {
        mark_gap_ms: 10,
        ..LIMITS
    };
    let mut f = begin_with(&settings, BowlMode::Single, Direction::D0, 3, 0);

    // A fall is no portion, but shows the rotor turning.
    f.advance(9);
    f.edge(false);
    assert_eq!(f.controller.progress.completed, 0);
    f.advance(9);
    assert_eq!(f.controller.state, FeedState::Running);
    f.advance(1);
    f.expect_fault(FeedReason::FeedbackTimeout);

    // Backing off and pausing do not count: the gap starts again with the
    // drive.
    let mut f = begin_with(&settings, BowlMode::Single, Direction::D0, 3, 0);
    f.advance(7);
    f.sample.current_ma = 301;
    f.advance(OVERCURRENT_POLLS.into());
    f.sample.current_ma = 100;
    f.advance(6);
    assert!(f.controller.state == FeedState::Running && f.controller.edge_at == 15);
    f.advance(9);
    assert_eq!(f.controller.state, FeedState::Running);
    f.advance(1);
    f.expect_fault(FeedReason::FeedbackTimeout);
}

#[test]
fn a_mark_input_low_past_any_mark_is_a_sensor_fault() {
    let settings = FeedLimits {
        mark_low_ms: 5,
        ..LIMITS
    };
    let mut f = begin_with(&settings, BowlMode::Single, Direction::D0, 3, 0);

    // A mark's LOW, then HIGH again: nothing.
    f.sample.mark_low = true;
    f.advance(5);
    f.sample.mark_low = false;
    f.advance(1);
    f.sample.mark_low = true;
    f.advance(5);
    assert_eq!(f.controller.state, FeedState::Running);
    f.advance(1);
    f.expect_fault(FeedReason::Sensor);

    // Only forward drive counts: a reversal starts the run again.
    let mut f = begin_with(&settings, BowlMode::Single, Direction::D0, 3, 0);
    f.sample.mark_low = true;
    f.sample.current_ma = 301;
    f.advance(OVERCURRENT_POLLS.into());
    f.sample.current_ma = 100;
    f.advance(6);
    assert_eq!(f.controller.state, FeedState::Running);
    f.advance(5);
    assert_eq!(f.controller.state, FeedState::Running);
    f.advance(1);
    f.expect_fault(FeedReason::Sensor);
}

#[test]
fn a_refused_start_names_its_input() {
    let controller = FeedController::new(&LIMITS, 0).unwrap();
    assert_eq!(controller.input_refusal(&HEALTHY), FeedReason::None);

    let cases = [
        (
            FeedSample {
                outlet_clear: false,
                ..HEALTHY
            },
            FeedReason::Outlet,
        ),
        (
            FeedSample {
                power_healthy: false,
                ..HEALTHY
            },
            FeedReason::Power,
        ),
        (
            FeedSample {
                optical_valid: false,
                ..HEALTHY
            },
            FeedReason::Sensor,
        ),
        // Over the limit with the motor stopped: only a bad reading shows it.
        (
            FeedSample {
                current_ma: 301,
                ..HEALTHY
            },
            FeedReason::Sensor,
        ),
    ];
    for (sample, reason) in cases {
        assert_eq!(controller.input_refusal(&sample), reason);
        let mut controller = controller;
        let r = request(1, 1, BowlMode::Single, Direction::D0);
        assert_eq!(
            controller.start(&r, 0, &sample),
            FeedStartResult::RejectInput
        );
    }
}

#[test]
fn one_poll_over_the_limit_is_a_surge_not_a_jam() {
    let mut f = begin(BowlMode::Single, Direction::D0, 3);
    f.sample.current_ma = 301;
    f.advance(1);
    f.sample.current_ma = 100;
    f.advance(1);
    assert!(f.controller.state == FeedState::Running && f.controller.reverse_attempts == 0);

    // An edge reuses its poll's sample, so it cannot make a second poll.
    f.sample.current_ma = 301;
    f.advance(1);
    f.edge(false);
    assert!(f.controller.state == FeedState::Running && f.controller.over_limit == 1);
    f.advance(1);
    assert_eq!(f.controller.state, FeedState::Reversing);
}

#[test]
fn overcurrent_while_backing_off_ends_the_feed() {
    let mut f = begin(BowlMode::Single, Direction::D0, 3);
    f.sample.current_ma = 301;
    f.advance(OVERCURRENT_POLLS.into());
    assert_eq!(f.controller.state, FeedState::Reversing);
    f.advance(OVERCURRENT_POLLS.into());
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
                f.advance(OVERCURRENT_POLLS.into());
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

    // A ceiling at or under the jam limit, a surge window that would hide a
    // jam while backing off or paused, and a mark limit of zero.
    for limits in [
        FeedLimits {
            current_max_ma: LIMITS.current_limit_ma,
            ..LIMITS
        },
        FeedLimits {
            surge_ms: LIMITS.reverse_ms,
            ..LIMITS
        },
        FeedLimits {
            reverse_ms: 5,
            surge_ms: LIMITS.direction_pause_ms,
            ..LIMITS
        },
        FeedLimits {
            mark_gap_ms: 0,
            ..LIMITS
        },
        FeedLimits {
            mark_low_ms: 0,
            ..LIMITS
        },
    ] {
        assert!(FeedController::new(&limits, 0).is_none(), "{limits:?}");
    }

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
fn reversals_extend_the_feedback_window_only_as_far_as_their_bound() {
    // Both reversals, then nothing: the window is the timeout plus their
    // cost, and no more reversal can stretch it.
    let mut f = begin(BowlMode::Single, Direction::D0, 2);
    for _ in 0..2 {
        f.sample.current_ma = 301;
        f.advance(OVERCURRENT_POLLS.into());
        f.sample.current_ma = 100;
        f.advance(6);
    }
    let window = LIMITS.feedback_timeout_ms + 2 * LIMITS.reversal_ms();
    f.advance(window - f.now - 1);
    assert_eq!(f.controller.state, FeedState::Running);
    f.advance(1);
    f.expect_fault(FeedReason::FeedbackTimeout);
}

#[test]
fn stopped_outputs_do_not_resume_on_bad_current() {
    let mut f = begin(BowlMode::Double, Direction::D1, 3);
    f.advance(5);
    f.edge(false);
    assert_eq!(f.controller.motor, MotorCommand::Stop);
    f.sample.current_ma = 301;
    f.advance(OVERCURRENT_POLLS.into());
    f.expect_fault(FeedReason::Overcurrent);

    let settings = FeedLimits {
        max_reverse_attempts: 0,
        ..LIMITS
    };
    let mut f = begin_with(&settings, BowlMode::Single, Direction::D0, 3, 0);
    f.sample.current_ma = 301;
    f.advance(OVERCURRENT_POLLS.into());
    f.expect_fault(FeedReason::Overcurrent);
    assert_eq!(f.controller.reverse_attempts, 0);

    let mut f = begin(BowlMode::Single, Direction::D0, 3);
    f.controller.update(1, None);
    f.expect_fault(FeedReason::Sensor);
}
