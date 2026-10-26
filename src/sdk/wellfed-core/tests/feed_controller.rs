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
        homing: false,
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
    // The inhibit window stays where the request started: the rotor wins
    // its ground back at once, and a mark just past the jam is real.
    assert_eq!(f.controller.inhibit_at, 0);
    assert_eq!(f.controller.resumed_at, f.now);

    // An edge captured before the resume belongs to the pause.
    let early = FeedSample {
        feedback_edge: true,
        feedback_high: true,
        ..f.sample
    };
    f.controller.update_captured(f.now, Some(&early), f.now - 1);
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

/// The PLAF103's feed limits, stepped every millisecond against a rotor
/// with its marks: a rise every `PERIOD` ms of drive, LOW for `LOW` ms
/// before it, position 0 just past a rise.
const PLAF103: FeedLimits = FeedLimits {
    feedback_inhibit_ms: 5000,
    feedback_timeout_ms: 15_000,
    direction_pause_ms: 500,
    reverse_ms: 1000,
    max_duration_ms: 12_000 * 48,
    portion_ms: 12_000,
    max_observation_gap_ms: 100,
    mark_gap_ms: 11_000,
    mark_low_ms: 4000,
    current_limit_ma: 300,
    current_max_ma: 1000,
    surge_ms: 150,
    max_portions: 48,
    max_reverse_attempts: 3,
};
const PERIOD: i64 = 9100;
const LOW: i64 = 1200;

fn rotor_high(position: i64) -> bool {
    position.rem_euclid(PERIOD) <= PERIOD - LOW
}

/// A feed of `portions` with a jam at `jam` ms of rotor travel that stalls
/// forward drive until the motor has backed off `stalls` times: the
/// controller when it stopped, and where the rotor stopped.
fn jammed_feed(portions: u16, jam: i64, stalls: u8) -> (FeedController, i64) {
    let r = request(1, portions, BowlMode::Single, Direction::D0);
    jammed(&PLAF103, &r, jam, stalls)
}

/// As `jammed_feed`, for any request: `Drive0` turns the rotor toward
/// higher positions and `Drive1` toward lower ones, and the jam stalls the
/// request's own direction at `jam`.
fn jammed(limits: &FeedLimits, r: &FeedRequest, jam: i64, stalls: u8) -> (FeedController, i64) {
    jammed_from(limits, r, 0, jam, stalls)
}

/// As `jammed`, with the rotor starting at `start` rather than just past a
/// rise: where an interrupted feed left it.
fn jammed_from(
    limits: &FeedLimits,
    r: &FeedRequest,
    start: i64,
    jam: i64,
    stalls: u8,
) -> (FeedController, i64) {
    let mut controller = FeedController::new(limits, 0).unwrap();
    assert_eq!(controller.start(r, 0, &HEALTHY), FeedStartResult::Accepted);
    let forward = MotorCommand::drive(r.direction);

    let (mut position, mut stalls) = (start, stalls);
    let mut last = controller.motor;
    for now in 1..120_000 {
        let motor = controller.motor;
        if last == forward && motor != forward && motor != MotorCommand::Stop {
            stalls = stalls.saturating_sub(1);
        }
        last = motor;

        let stalled = motor == forward && stalls > 0 && position == jam;
        let before = rotor_high(position);
        match motor {
            _ if stalled => {}
            MotorCommand::Drive0 => position += 1,
            MotorCommand::Drive1 => position -= 1,
            MotorCommand::Stop => {}
        }
        let high = rotor_high(position);

        let poll = FeedSample {
            current_ma: if stalled { 400 } else { 100 },
            mark_low: !high,
            ..HEALTHY
        };
        controller.update(now, Some(&poll));
        if high != before {
            let edge = FeedSample {
                feedback_edge: true,
                feedback_high: high,
                ..poll
            };
            controller.update(now, Some(&edge));
        }
        if !controller.active() {
            break;
        }
    }
    (controller, position)
}

/// Wherever a jam the reversals free comes, before, inside or just past a
/// mark, the feed credits every mark the rotor really passes once, and
/// stops just past the last: no mark hidden by an inhibit window restarted
/// at the resume, and none counted twice when backing off recrossed it.
#[test]
fn a_freed_jam_anywhere_in_a_mark_period_credits_each_mark_once() {
    let last_rise = 2 * PERIOD;
    let mut jams: Vec<i64> = (100..last_rise).step_by(53).collect();
    // Just after a credited rise, recrossed while backing off or not; and
    // inside a mark, before its rise.
    jams.extend([PERIOD + 1, PERIOD + 300, PERIOD + 999, PERIOD + 1000]);
    jams.extend([PERIOD - 600, PERIOD - 1, PERIOD - LOW + 1]);

    for stalls in 1..=3 {
        for &jam in &jams {
            let (controller, position) = jammed_feed(2, jam, stalls);
            assert_eq!(
                (controller.state, controller.progress.completed),
                (FeedState::Complete, 2),
                "jam at {jam}, {stalls} reversals: {:?}",
                controller.reason
            );
            assert_eq!(controller.reverse_attempts, stalls, "jam at {jam}");
            assert!(
                (last_rise..=last_rise + 1).contains(&position),
                "jam at {jam}, {stalls} reversals: stopped at {position}"
            );
        }
    }
}

/// The same in a double bowl's D1, whose boundary is the fall into a mark:
/// after it the rotor reads LOW, so backing off that stays inside the mark
/// recrosses nothing, and only backing out over the fall (reading HIGH)
/// makes the next fall the same one. The request counts a portion every
/// second fall; the first, at the start, falls in the inhibit window.
#[test]
fn a_freed_jam_in_a_double_bowl_d1_credits_each_fall_once() {
    let limits = FeedLimits {
        portion_ms: 30_000,
        ..PLAF103
    };
    let r = FeedRequest {
        credit_next_event: false,
        ..request(1, 1, BowlMode::Double, Direction::D1)
    };
    let last_fall = -2 * PERIOD - 1;
    let mut jams: Vec<i64> = (-2 * PERIOD + 1..-100).step_by(53).collect();
    // Just past the counted fall at -PERIOD - 1, backing out over it or
    // staying inside its mark; and just before it.
    jams.extend([-PERIOD - 2, -PERIOD - 300, -PERIOD - 999, -PERIOD - 1100]);
    jams.extend([-PERIOD + 600, -PERIOD]);

    for stalls in 1..=3 {
        for &jam in &jams {
            let (controller, position) = jammed(&limits, &r, jam, stalls);
            assert_eq!(
                (controller.state, controller.progress.completed),
                (FeedState::Complete, 1),
                "jam at {jam}, {stalls} reversals: {:?}",
                controller.reason
            );
            assert_eq!(controller.reverse_attempts, stalls, "jam at {jam}");
            assert!(
                (last_fall - 1..=last_fall).contains(&position),
                "jam at {jam}, {stalls} reversals: stopped at {position}"
            );
        }
    }
}

fn begin_homing(settings: &FeedLimits, portions: u16) -> Fixture {
    let mut f = Fixture {
        controller: FeedController::new(settings, 0).unwrap(),
        sample: HEALTHY,
        now: 0,
    };
    let r = FeedRequest {
        homing: true,
        ..request(1, portions, BowlMode::Single, Direction::D0)
    };
    assert_eq!(
        f.controller.start(&r, 0, &f.sample),
        FeedStartResult::Accepted
    );
    f
}

#[test]
fn a_homing_mark_inside_the_inhibit_is_never_credited() {
    let mut f = begin_homing(&LIMITS, 1);
    f.advance(2);
    f.edge(false);
    f.edge(true);
    assert_eq!(f.controller.progress.completed, 0);
    assert_eq!(f.controller.homed_ms, Some(2));
    assert!(!f.controller.homing);

    // The portion counts from the homing mark: a mark inside the inhibit
    // after it is ignored, the next past it completes the request.
    f.advance(4);
    f.edge(true);
    assert_eq!(f.controller.progress.completed, 0);
    f.advance(1);
    f.edge(true);
    assert_eq!(f.controller.state, FeedState::Complete);
    assert_eq!(f.controller.progress.completed, 1);
    assert_eq!(f.controller.homed_ms, Some(2));
}

#[test]
fn a_first_mark_past_the_inhibit_is_credited_and_ends_homing() {
    let mut f = begin_homing(&LIMITS, 2);
    f.advance(6);
    f.edge(true);
    assert_eq!(f.controller.progress.completed, 1);
    assert!(!f.controller.homing);

    // At a mark now: one inside the next inhibit window is ignored.
    f.advance(2);
    f.edge(true);
    assert_eq!(f.controller.progress.completed, 1);
    assert_eq!(f.controller.homed_ms, None);
}

#[test]
fn homing_extends_the_deadline_by_one_inhibit_window() {
    let settings = FeedLimits {
        max_duration_ms: 1000,
        ..LIMITS
    };
    let f = begin_homing(&settings, 2);
    assert_eq!(f.controller.deadline_ms, 2 * 120 + 5);
    // Never past the longest request.
    let f = begin_homing(&LIMITS, 1);
    assert_eq!(f.controller.deadline_ms, 120);

    // Without homing, a mark inside the inhibit is ignored and records nothing.
    let mut plain = begin_with(&settings, BowlMode::Single, Direction::D0, 2, 0);
    assert_eq!(plain.controller.deadline_ms, 2 * 120);
    plain.advance(2);
    plain.edge(true);
    assert_eq!(plain.controller.homed_ms, None);
    assert_eq!(plain.controller.progress.completed, 0);
}

#[test]
fn a_double_bowl_never_homes() {
    let mut c = FeedController::new(&LIMITS, 0).unwrap();
    let r = FeedRequest {
        homing: true,
        ..request(1, 1, BowlMode::Double, Direction::D0)
    };
    assert_eq!(c.start(&r, 0, &HEALTHY), FeedStartResult::RejectRequest);
}

/// A feed from an unknown phase that jams before its homing mark backs off
/// and drives on as any feed does; the mark it then reaches inside the
/// inhibit window still homes it, uncredited, and its one portion is the
/// next mark.
#[test]
fn a_jam_before_the_homing_mark_still_homes_on_it() {
    let r = FeedRequest {
        homing: true,
        ..request(1, 1, BowlMode::Single, Direction::D0)
    };
    // 3 s of drive short of a mark, jammed 0.5 s in, for one reversal.
    let start = PERIOD - 3000;
    let (controller, position) = jammed_from(&PLAF103, &r, start, start + 500, 1);

    assert_eq!(controller.reverse_attempts, 1);
    assert_eq!(
        (controller.state, controller.reason),
        (FeedState::Complete, FeedReason::Done)
    );
    assert_eq!(controller.progress.completed, 1);
    // 3 s of travel plus the 2.5 s backing off cost: past the bare
    // inhibit, inside it once the reversal's cost is added back.
    let homed = controller.homed_ms.expect("the first mark homed it");
    assert!(
        (PLAF103.feedback_inhibit_ms..PLAF103.feedback_inhibit_ms + 1000).contains(&homed),
        "homed at {homed} ms"
    );
    // Stopped on the mark after the homing one, not the homing mark.
    assert!(
        (2 * PERIOD..2 * PERIOD + 50).contains(&position),
        "stopped at {position}"
    );
}

/// Where a homing mark ends and a credited one begins after a reversal:
/// the window is the inhibit plus what backing off cost, so a first mark
/// just inside it homes and one just past it is credited, as stock credits
/// a first mark past its plain window.
#[test]
fn a_first_mark_just_past_the_widened_window_is_credited() {
    let r = FeedRequest {
        homing: true,
        ..request(1, 1, BowlMode::Single, Direction::D0)
    };
    let window = PLAF103.feedback_inhibit_ms + PLAF103.reversal_ms();

    // Forward travel to the mark, jammed 0.5 s in for one reversal: the
    // mark arrives at the travel plus the reversal's cost (and the polls
    // the stall took to qualify).
    for (travel, homed) in [(4_950, true), (5_050, false)] {
        let start = PERIOD - travel;
        let (controller, position) = jammed_from(&PLAF103, &r, start, start + 500, 1);
        assert_eq!(controller.reverse_attempts, 1, "travel {travel}");
        assert_eq!(
            (controller.state, controller.reason),
            (FeedState::Complete, FeedReason::Done),
            "travel {travel}"
        );
        assert_eq!(controller.progress.completed, 1);
        match controller.homed_ms {
            Some(at) => {
                assert!(homed, "travel {travel}: homed at {at} ms");
                assert!(at < window, "travel {travel}: homed at {at} ms");
                // The portion is the mark after the homing one.
                assert!((2 * PERIOD..2 * PERIOD + 50).contains(&position));
            }
            None => {
                assert!(!homed, "travel {travel}: credited its first mark");
                assert!((PERIOD..PERIOD + 50).contains(&position));
            }
        }
    }
}

/// A homing feed fails as any feed does: no edge for the feedback timeout,
/// before or after its homing mark, and a mark input held LOW.
#[test]
fn a_homing_feed_ends_on_lost_feedback_or_a_stuck_mark() {
    // No mark at all: nothing homed, nothing credited.
    let mut f = begin_homing(&LIMITS, 1);
    f.advance(30);
    f.expect_fault(FeedReason::FeedbackTimeout);
    assert_eq!(
        (f.controller.homed_ms, f.controller.progress.completed),
        (None, 0)
    );

    // Homed, then no mark: the timeout counts from the homing mark.
    let mut f = begin_homing(&LIMITS, 1);
    f.advance(2);
    f.edge(false);
    f.edge(true);
    assert_eq!(f.controller.homed_ms, Some(2));
    f.advance(29);
    assert_eq!(f.controller.state, FeedState::Running);
    f.advance(1);
    f.expect_fault(FeedReason::FeedbackTimeout);
    assert_eq!(f.controller.progress.completed, 0);

    // The mark input LOW past any mark, as a rotor stopped inside one
    // reads: a sensor fault, never a homing mark.
    let settings = FeedLimits {
        mark_low_ms: 5,
        ..LIMITS
    };
    let mut f = begin_homing(&settings, 1);
    f.sample.mark_low = true;
    f.advance(5);
    assert_eq!(f.controller.state, FeedState::Running);
    f.advance(1);
    f.expect_fault(FeedReason::Sensor);
    assert_eq!(f.controller.homed_ms, None);
}
