//! Feeding state machine: inhibit windows, pauses, reversal, deadlines and faults.
//!
//! The single serialized owner drives it with fresh input samples and captured
//! feedback edges. It only computes the desired motor command; the board
//! adapter applies it and backs it with the output gate and watchdog. Faults
//! and deadlines are checked before feedback, and every terminal state
//! commands STOP. No physical limits are built in: board validation must
//! supply [`FeedLimits`].

use crate::feed::progress::{BowlMode, Direction, FeedProgress, ProgressResult};
use crate::{HALF_RANGE, interval_valid};

c_enum!(MotorCommand { STOP = 0, DRIVE_0 = 1, DRIVE_1 = 2 });

c_enum!(FeedState {
    IDLE = 0,
    RUNNING = 1,
    DIRECTION_PAUSE = 2,
    REVERSING = 3,
    COMPLETE = 4,
    FAULT = 5,
    CANCELLED = 6,
});

c_enum!(
    /// Persistent journal v1 encodes these values; never renumber without migration.
    FeedReason {
        NONE = 0,
        DONE = 1,
        CANCELLED = 2,
        CLOCK = 3,
        OBSERVATION_GAP = 4,
        TOTAL_TIMEOUT = 5,
        FEEDBACK_TIMEOUT = 6,
        POWER = 7,
        SENSOR = 8,
        OUTLET = 9,
        OVERCURRENT = 10,
        FEEDBACK_STREAM = 11,
        OUTPUT = 12,
    }
);

c_enum!(FeedStartResult {
    ACCEPTED = 0,
    REJECT_BUSY = 1,
    REJECT_RESET_REQUIRED = 2,
    REJECT_ID = 3,
    REJECT_REQUEST = 4,
    REJECT_INPUT = 5,
});

impl MotorCommand {
    pub fn valid(self) -> bool {
        self.0 <= Self::DRIVE_1.0
    }

    pub fn drive(direction: Direction) -> Self {
        if direction == Direction::D0 {
            Self::DRIVE_0
        } else {
            Self::DRIVE_1
        }
    }
}

/// All intervals are monotonic milliseconds, nonzero and below 2^31. No
/// physical defaults are supplied: board validation must establish limits.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeedLimits {
    pub feedback_inhibit_ms: u32,
    pub feedback_timeout_ms: u32,
    pub direction_pause_ms: u32,
    pub reverse_ms: u32,
    pub max_duration_ms: u32,
    pub max_observation_gap_ms: u32,

    pub current_limit_ma: u32,
    pub max_portions: u16,
    pub max_reverse_attempts: u8,
}

impl FeedLimits {
    pub fn valid(&self) -> bool {
        interval_valid(self.feedback_inhibit_ms)
            && interval_valid(self.feedback_timeout_ms)
            && interval_valid(self.direction_pause_ms)
            && interval_valid(self.reverse_ms)
            && interval_valid(self.max_duration_ms)
            && interval_valid(self.max_observation_gap_ms)
            && self.feedback_timeout_ms > self.feedback_inhibit_ms
            && self.feedback_timeout_ms > self.direction_pause_ms
            && self.feedback_timeout_ms > self.reverse_ms
            && self.max_duration_ms >= self.feedback_timeout_ms
            && self.max_observation_gap_ms <= self.feedback_timeout_ms
            && self.current_limit_ma != 0
            && self.max_portions != 0
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeedRequest {
    /// Strictly increasing nonzero ID; reserve durably before applying output.
    pub id: u64,
    pub portions: u16,
    pub mode: BowlMode,
    pub direction: Direction,
    pub credit_next_event: bool,
    /// Required for DOUBLE_BOWL; neither reset nor reboot establishes phase.
    pub phase_known: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeedSample {
    pub power_healthy: bool,
    pub optical_valid: bool,
    /// Filtered outlet state; a brief grain-passage pulse is not a blockage.
    pub outlet_clear: bool,
    pub current_valid: bool,
    pub current_ma: u32,
    /// One captured edge. The adapter must not repeat it on successive polls.
    pub feedback_edge: bool,
    pub feedback_high: bool,
}

impl FeedSample {
    fn input_fault(&self) -> FeedReason {
        if !self.power_healthy {
            FeedReason::POWER
        } else if !self.current_valid || !self.optical_valid {
            FeedReason::SENSOR
        } else if !self.outlet_clear {
            FeedReason::OUTLET
        } else {
            FeedReason::NONE
        }
    }
}

/// Single serialized owner; inspect fields for reporting, mutate only via methods.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeedController {
    pub limits: FeedLimits,
    pub progress: FeedProgress,
    pub last_request_id: u64,

    pub started_at: u32,
    pub last_update_at: u32,
    pub last_feedback_at: u32,
    pub inhibit_at: u32,
    pub state_at: u32,

    pub reverse_attempts: u8,
    pub state: FeedState,
    pub reason: FeedReason,
    pub motor: MotorCommand,
}

impl FeedController {
    /// Boot initialization, after hardware stop. Restore the durable ID high-water mark.
    pub fn new(limits: &FeedLimits, last_request_id: u64) -> Option<Self> {
        limits.valid().then(|| Self {
            limits: *limits,
            last_request_id,
            state: FeedState::IDLE,
            motor: MotorCommand::STOP,
            ..Self::default()
        })
    }

    pub fn active(&self) -> bool {
        self.state == FeedState::RUNNING
            || self.state == FeedState::DIRECTION_PAUSE
            || self.state == FeedState::REVERSING
    }

    fn stop(&mut self, state: FeedState, reason: FeedReason) {
        self.motor = MotorCommand::STOP;
        self.state = state;
        self.reason = reason;
    }

    /// Fresh sensor snapshot required; any feedback edge in it is ignored.
    pub fn start(
        &mut self,
        request: &FeedRequest,
        now_ms: u32,
        sample: &FeedSample,
    ) -> FeedStartResult {
        if self.active() {
            return FeedStartResult::REJECT_BUSY;
        }
        if self.state == FeedState::FAULT || self.state == FeedState::CANCELLED {
            return FeedStartResult::REJECT_RESET_REQUIRED;
        }
        if request.id == 0 || request.id <= self.last_request_id {
            return FeedStartResult::REJECT_ID;
        }

        let progress = if request.portions > self.limits.max_portions
            || (request.mode == BowlMode::SINGLE && request.direction != Direction::D0)
            || (request.mode == BowlMode::DOUBLE && !request.phase_known)
        {
            None
        } else {
            FeedProgress::new(
                request.mode,
                request.direction,
                request.portions,
                request.credit_next_event,
            )
        };
        let Some(progress) = progress else {
            return FeedStartResult::REJECT_REQUEST;
        };

        if sample.input_fault() != FeedReason::NONE
            || sample.current_ma > self.limits.current_limit_ma
        {
            return FeedStartResult::REJECT_INPUT;
        }

        self.progress = progress;
        self.last_request_id = request.id;
        self.started_at = now_ms;
        self.last_update_at = now_ms;
        self.last_feedback_at = now_ms;
        self.inhibit_at = now_ms;
        self.state_at = now_ms;
        self.reverse_attempts = 0;
        self.state = FeedState::RUNNING;
        self.reason = FeedReason::NONE;
        self.motor = MotorCommand::drive(progress.direction);

        FeedStartResult::ACCEPTED
    }

    /// Process fresh samples in timestamp order, including polls without edges.
    /// Faults are evaluated before feedback and timer-driven motor transitions.
    /// `u32` clock wrap is supported; backward/ambiguous jumps fault the request.
    ///
    /// This only updates a DESIRED motor command. The adapter must apply stop
    /// promptly, enforce driver transition timing and use an independent
    /// watchdog/hardware stop if this stops running.
    pub fn update(&mut self, now_ms: u32, sample: Option<&FeedSample>) {
        self.update_captured(now_ms, sample, now_ms);
    }

    /// Process at `now_ms` but qualify one edge using its original capture time.
    /// Current faults/deadlines still take precedence. A future/ambiguous
    /// capture faults; an edge predating the current inhibit window is
    /// discarded. The caller must also bound queue age, overflow and actual
    /// drive continuity.
    pub fn update_captured(
        &mut self,
        now_ms: u32,
        sample: Option<&FeedSample>,
        captured_at_ms: u32,
    ) {
        if !self.active() {
            return;
        }

        let gap = now_ms.wrapping_sub(self.last_update_at);
        self.last_update_at = now_ms;
        let mut reason = if gap >= HALF_RANGE {
            FeedReason::CLOCK
        } else if gap > self.limits.max_observation_gap_ms {
            FeedReason::OBSERVATION_GAP
        } else if now_ms.wrapping_sub(self.started_at) >= self.limits.max_duration_ms {
            FeedReason::TOTAL_TIMEOUT
        } else {
            match sample {
                None => FeedReason::SENSOR,
                Some(sample) => sample.input_fault(),
            }
        };

        if reason == FeedReason::NONE
            && sample.is_some_and(|s| s.feedback_edge)
            && now_ms.wrapping_sub(captured_at_ms) >= HALF_RANGE
        {
            reason = FeedReason::CLOCK;
        }
        if reason == FeedReason::NONE
            && now_ms.wrapping_sub(self.last_feedback_at) >= self.limits.feedback_timeout_ms
        {
            reason = FeedReason::FEEDBACK_TIMEOUT;
        }

        let Some(sample) = sample.filter(|_| reason == FeedReason::NONE) else {
            self.stop(FeedState::FAULT, reason);
            return;
        };

        let overcurrent = sample.current_ma > self.limits.current_limit_ma;
        if self.state == FeedState::REVERSING {
            if now_ms.wrapping_sub(self.state_at) >= self.limits.reverse_ms {
                if overcurrent {
                    self.stop(FeedState::FAULT, FeedReason::OVERCURRENT);
                    return;
                }
                self.state = FeedState::RUNNING;
                self.motor = MotorCommand::drive(self.progress.direction);
                self.inhibit_at = now_ms;
            }
            return;
        }

        if self.state == FeedState::DIRECTION_PAUSE {
            if overcurrent {
                self.stop(FeedState::FAULT, FeedReason::OVERCURRENT);
                return;
            }
            if now_ms.wrapping_sub(self.state_at) >= self.limits.direction_pause_ms {
                self.state = FeedState::RUNNING;
                self.motor = MotorCommand::drive(self.progress.direction);
                // Edges captured while stopped or coasting must not count.
                self.inhibit_at = now_ms;
            }
            return;
        }

        if overcurrent {
            if self.reverse_attempts >= self.limits.max_reverse_attempts {
                self.stop(FeedState::FAULT, FeedReason::OVERCURRENT);
            } else {
                self.reverse_attempts = self.reverse_attempts.wrapping_add(1);
                self.state = FeedState::REVERSING;
                self.state_at = now_ms;
                self.motor = MotorCommand::drive(self.progress.direction.toggled());
            }
            return;
        }

        let inhibition = captured_at_ms.wrapping_sub(self.inhibit_at);
        if !sample.feedback_edge
            || inhibition >= HALF_RANGE
            || inhibition < self.limits.feedback_inhibit_ms
        {
            return;
        }

        let expected_high =
            self.progress.mode == BowlMode::SINGLE || self.progress.direction == Direction::D0;
        if sample.feedback_high != expected_high {
            return;
        }

        self.last_feedback_at = captured_at_ms;
        self.inhibit_at = captured_at_ms;
        let result = self.progress.accept();
        if result == ProgressResult::FINISHED {
            self.stop(FeedState::COMPLETE, FeedReason::DONE);
        } else if result == ProgressResult::PORTION && self.progress.mode == BowlMode::DOUBLE {
            if self.progress.direction == Direction::D0 {
                self.state = FeedState::DIRECTION_PAUSE;
                self.state_at = now_ms;
                self.motor = MotorCommand::STOP;
            } else {
                self.motor = MotorCommand::DRIVE_1;
            }
        }
    }

    /// External input/output failures latch a fault only for an active request.
    /// Rejects NONE, DONE, CANCELLED and invalid reason values. Does not apply GPIO.
    pub fn trip(&mut self, reason: FeedReason) -> bool {
        if !self.active() || reason.0 < FeedReason::CLOCK.0 || reason.0 > FeedReason::OUTPUT.0 {
            return false;
        }
        self.stop(FeedState::FAULT, reason);
        true
    }

    /// A stale cancellation cannot stop a different request. Terminal output stays STOP.
    pub fn cancel(&mut self, id: u64) -> bool {
        if !self.active() || id != self.last_request_id {
            return false;
        }
        self.stop(FeedState::CANCELLED, FeedReason::CANCELLED);
        true
    }

    /// Acknowledge a terminal result; never resets IDs or establishes mechanical phase.
    pub fn reset(&mut self) -> bool {
        if self.active() {
            return false;
        }
        self.stop(FeedState::IDLE, FeedReason::NONE);
        true
    }
}
