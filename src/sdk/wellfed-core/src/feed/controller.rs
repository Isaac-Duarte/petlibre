//! Feeding state machine: inhibit windows, pauses, reversal, deadlines and faults.
//!
//! The single serialized owner drives it with fresh input samples and captured
//! feedback edges. It only computes the desired motor command; the board
//! adapter applies it and backs it with the output gate and watchdog. Faults
//! and deadlines are checked before feedback, and every terminal state
//! commands STOP. No physical limits are built in: board validation must
//! supply [`FeedLimits`].
//!
//! A jam is current held over the limit past a drive's start-up surge: the
//! controller backs off (`reverse_ms` the other way, then a stop of
//! `direction_pause_ms`) and drives on, at most `max_reverse_attempts`
//! times, then ends the request `Overcurrent`. Each reversal lengthens the
//! request's deadline and its feedback window by what it costs (the time
//! back, the pause, and the time to win the ground back), so a jam that
//! clears does not then time out; the attempts bound that extension. A
//! reading over `current_max_ma`, or the limit held while backing off or
//! stopped, ends it at once.
//!
//! Missing feedback is no jam: without marks the controller cannot tell a
//! stopped rotor from a lost sensor, so it never reverses blind. The request
//! ends `FeedbackTimeout` (no edge of either level for `mark_gap_ms` of
//! drive, or no portion within the feedback timeout) or `Sensor` (the mark
//! input LOW for `mark_low_ms` of drive, longer than any mark lasts).

use crate::feed::progress::{BowlMode, Direction, FeedProgress, ProgressResult};
use crate::{HALF_RANGE, interval_valid};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum MotorCommand {
    #[default]
    Stop = 0,
    Drive0 = 1,
    Drive1 = 2,
}

impl MotorCommand {
    /// The variant's number, as records and reports print it.
    pub const fn code(self) -> u32 {
        self as u32
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum FeedState {
    #[default]
    Idle = 0,
    Running = 1,
    DirectionPause = 2,
    Reversing = 3,
    Complete = 4,
    Fault = 5,
    Cancelled = 6,
}

impl FeedState {
    /// The variant's number, as records and reports print it.
    pub const fn code(self) -> u32 {
        self as u32
    }
}

/// Persistent journal v1 encodes these values; never renumber without migration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum FeedReason {
    #[default]
    None = 0,
    Done = 1,
    Cancelled = 2,
    Clock = 3,
    ObservationGap = 4,
    TotalTimeout = 5,
    FeedbackTimeout = 6,
    Power = 7,
    Sensor = 8,
    Outlet = 9,
    Overcurrent = 10,
    FeedbackStream = 11,
    Output = 12,
}

impl FeedReason {
    /// The variant's number, as records and reports print it.
    pub const fn code(self) -> u32 {
        self as u32
    }

    /// The one-byte field a flash record stores.
    pub const fn byte(self) -> u8 {
        self as u8
    }

    /// The variant a record's byte names; `None` for any other byte.
    pub const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::None),
            1 => Some(Self::Done),
            2 => Some(Self::Cancelled),
            3 => Some(Self::Clock),
            4 => Some(Self::ObservationGap),
            5 => Some(Self::TotalTimeout),
            6 => Some(Self::FeedbackTimeout),
            7 => Some(Self::Power),
            8 => Some(Self::Sensor),
            9 => Some(Self::Outlet),
            10 => Some(Self::Overcurrent),
            11 => Some(Self::FeedbackStream),
            12 => Some(Self::Output),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum FeedStartResult {
    #[default]
    Accepted = 0,
    RejectBusy = 1,
    RejectResetRequired = 2,
    RejectId = 3,
    RejectRequest = 4,
    RejectInput = 5,
}

impl FeedStartResult {
    /// The variant's number, as records and reports print it.
    pub const fn code(self) -> u32 {
        self as u32
    }
}

impl MotorCommand {
    pub fn drive(direction: Direction) -> Self {
        if direction == Direction::D0 {
            Self::Drive0
        } else {
            Self::Drive1
        }
    }
}

/// All intervals are monotonic milliseconds, nonzero and below 2^31 (but
/// `surge_ms`, which may be zero). No physical defaults are supplied: board
/// validation must establish limits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeedLimits {
    pub feedback_inhibit_ms: u32,
    pub feedback_timeout_ms: u32,
    /// The stop between a double bowl's directions, and after each reversal.
    pub direction_pause_ms: u32,
    /// How long one reversal drives the other way.
    pub reverse_ms: u32,
    /// The longest any request may run, whatever its portions or reversals.
    pub max_duration_ms: u32,
    /// Each portion's share of a request's deadline: a request of `n`
    /// portions stops at `n` times this, or at `max_duration_ms` if sooner.
    pub portion_ms: u32,
    pub max_observation_gap_ms: u32,
    /// The longest forward drive with no feedback edge of either level: a
    /// turning rotor passes a mark's fall or rise well inside it.
    pub mark_gap_ms: u32,
    /// The longest forward drive with the feedback input LOW: longer than
    /// any mark reads LOW while the rotor turns.
    pub mark_low_ms: u32,

    /// Current held over this (two polls past the surge window) is a jam.
    pub current_limit_ma: u32,
    /// One reading over this stops the motor at once, surge or not, with
    /// no reversal.
    pub current_max_ma: u32,
    /// After every change of drive (a start, a reversal, a pause, a
    /// resume), polls this long count toward no jam: the driver's deadtime
    /// and the motor's start-up surge read high.
    pub surge_ms: u32,
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
            && interval_valid(self.portion_ms)
            && interval_valid(self.max_observation_gap_ms)
            && interval_valid(self.mark_gap_ms)
            && interval_valid(self.mark_low_ms)
            && self.feedback_timeout_ms > self.feedback_inhibit_ms
            && self.feedback_timeout_ms > self.direction_pause_ms
            && self.feedback_timeout_ms > self.reverse_ms
            && self.max_duration_ms >= self.feedback_timeout_ms
            && self.max_duration_ms >= self.portion_ms
            && self.max_observation_gap_ms <= self.feedback_timeout_ms
            // A jam while backing off or paused must still be seen.
            && self.surge_ms < self.reverse_ms
            && self.surge_ms < self.direction_pause_ms
            && self.current_limit_ma != 0
            && self.current_max_ma > self.current_limit_ma
            && self.max_portions != 0
    }

    /// What one reversal adds to a request's deadline and feedback window:
    /// the time back, the pause, and the time forward to win it back.
    pub const fn reversal_ms(&self) -> u32 {
        self.reverse_ms
            .saturating_mul(2)
            .saturating_add(self.direction_pause_ms)
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
    /// The feedback input read LOW in this acquisition (at a mark, or stuck).
    pub mark_low: bool,
}

impl FeedSample {
    fn input_fault(&self) -> FeedReason {
        if !self.power_healthy {
            FeedReason::Power
        } else if !self.current_valid || !self.optical_valid {
            FeedReason::Sensor
        } else if !self.outlet_clear {
            FeedReason::Outlet
        } else {
            FeedReason::None
        }
    }
}

/// Consecutive polls over `current_limit_ma` that count as overcurrent: a
/// motor's start-up surge can read over the limit for one poll (seen once at
/// 17 ms into a drive), while a jam stays over it.
pub const OVERCURRENT_POLLS: u8 = 2;

/// Single serialized owner; inspect fields for reporting, mutate only via methods.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeedController {
    pub limits: FeedLimits,
    pub progress: FeedProgress,
    pub last_request_id: u64,

    pub started_at: u32,
    /// From `started_at`: this request's deadline.
    pub deadline_ms: u32,
    pub last_update_at: u32,
    pub last_feedback_at: u32,
    pub inhibit_at: u32,
    pub state_at: u32,

    /// When the commanded drive last changed (start, reversal, pause,
    /// resume): the surge window counts from here.
    pub drive_at: u32,
    /// The last feedback edge of either level, or the last start or resume.
    pub edge_at: u32,
    /// Since when the feedback input has read LOW while running forward.
    pub low_since: Option<u32>,
    /// What the reversals since the last portion add to the feedback
    /// timeout.
    pub feedback_grace_ms: u32,

    pub reverse_attempts: u8,
    /// Consecutive polls over the current limit past the surge window; a
    /// feedback edge reuses its poll's sample and leaves this unchanged.
    pub over_limit: u8,
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
            state: FeedState::Idle,
            motor: MotorCommand::Stop,
            ..Self::default()
        })
    }

    pub fn active(&self) -> bool {
        self.state == FeedState::Running
            || self.state == FeedState::DirectionPause
            || self.state == FeedState::Reversing
    }

    fn stop(&mut self, state: FeedState, reason: FeedReason) {
        self.motor = MotorCommand::Stop;
        self.state = state;
        self.reason = reason;
    }

    /// Why `sample` keeps a request from starting, or `None`: an input
    /// fault, or current over the limit with the motor stopped, which only
    /// a faulty reading can show.
    pub fn input_refusal(&self, sample: &FeedSample) -> FeedReason {
        match sample.input_fault() {
            FeedReason::None if sample.current_ma > self.limits.current_limit_ma => {
                FeedReason::Sensor
            }
            reason => reason,
        }
    }

    /// The commanded drive changes at `now_ms`: the surge window and the
    /// jam count start again, and a LOW run is no longer timed.
    fn drive_changed(&mut self, now_ms: u32) {
        self.drive_at = now_ms;
        self.over_limit = 0;
        self.low_since = None;
    }

    /// Forward again after a pause. Edges captured while stopped or
    /// coasting must not count, and the mark gap counts afresh.
    fn resume(&mut self, now_ms: u32) {
        self.state = FeedState::Running;
        self.motor = MotorCommand::drive(self.progress.direction);
        self.inhibit_at = now_ms;
        self.edge_at = now_ms;
        self.drive_changed(now_ms);
    }

    /// Back off a jam: the other way for `reverse_ms`, then a pause, then
    /// forward again. The deadline and the feedback window grow by what it
    /// costs, the deadline never past `max_duration_ms`.
    fn reverse(&mut self, now_ms: u32) {
        let cost = self.limits.reversal_ms();
        self.reverse_attempts = self.reverse_attempts.saturating_add(1);
        self.deadline_ms = self
            .deadline_ms
            .saturating_add(cost)
            .min(self.limits.max_duration_ms);
        self.feedback_grace_ms = self.feedback_grace_ms.saturating_add(cost);

        self.state = FeedState::Reversing;
        self.state_at = now_ms;
        self.motor = MotorCommand::drive(self.progress.direction.toggled());
        self.drive_changed(now_ms);
    }

    /// Stop between a double bowl's directions, or after a reversal.
    fn pause(&mut self, now_ms: u32) {
        self.state = FeedState::DirectionPause;
        self.state_at = now_ms;
        self.motor = MotorCommand::Stop;
        self.drive_changed(now_ms);
    }

    /// Whether the feedback input has read LOW for `mark_low_ms` of forward
    /// drive: no mark lasts that long while the rotor turns, and a stalled
    /// rotor draws the current a jam shows first.
    fn stuck_low(&mut self, now_ms: u32, low: bool) -> bool {
        if !low {
            self.low_since = None;
            return false;
        }
        let since = *self.low_since.get_or_insert(now_ms);
        now_ms.wrapping_sub(since) >= self.limits.mark_low_ms
    }

    /// Fresh sensor snapshot required; any feedback edge in it is ignored.
    pub fn start(
        &mut self,
        request: &FeedRequest,
        now_ms: u32,
        sample: &FeedSample,
    ) -> FeedStartResult {
        if self.active() {
            return FeedStartResult::RejectBusy;
        }
        if self.state == FeedState::Fault || self.state == FeedState::Cancelled {
            return FeedStartResult::RejectResetRequired;
        }
        if request.id == 0 || request.id <= self.last_request_id {
            return FeedStartResult::RejectId;
        }

        let progress = if request.portions > self.limits.max_portions
            || (request.mode == BowlMode::Single && request.direction != Direction::D0)
            || (request.mode == BowlMode::Double && !request.phase_known)
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
            return FeedStartResult::RejectRequest;
        };

        if self.input_refusal(sample) != FeedReason::None {
            return FeedStartResult::RejectInput;
        }

        self.progress = progress;
        self.last_request_id = request.id;
        self.started_at = now_ms;
        self.deadline_ms = u32::from(request.portions)
            .saturating_mul(self.limits.portion_ms)
            .min(self.limits.max_duration_ms);
        self.last_update_at = now_ms;
        self.last_feedback_at = now_ms;
        self.inhibit_at = now_ms;
        self.state_at = now_ms;
        self.edge_at = now_ms;
        self.feedback_grace_ms = 0;
        self.reverse_attempts = 0;
        self.drive_changed(now_ms);
        self.state = FeedState::Running;
        self.reason = FeedReason::None;
        self.motor = MotorCommand::drive(progress.direction);

        FeedStartResult::Accepted
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
            FeedReason::Clock
        } else if gap > self.limits.max_observation_gap_ms {
            FeedReason::ObservationGap
        } else if now_ms.wrapping_sub(self.started_at) >= self.deadline_ms {
            FeedReason::TotalTimeout
        } else {
            match sample {
                None => FeedReason::Sensor,
                Some(sample) => sample.input_fault(),
            }
        };

        if reason == FeedReason::None
            && sample.is_some_and(|s| s.feedback_edge)
            && now_ms.wrapping_sub(captured_at_ms) >= HALF_RANGE
        {
            reason = FeedReason::Clock;
        }
        let feedback_window = self
            .limits
            .feedback_timeout_ms
            .saturating_add(self.feedback_grace_ms);
        if reason == FeedReason::None
            && (now_ms.wrapping_sub(self.last_feedback_at) >= feedback_window
                || (self.state == FeedState::Running
                    && now_ms.wrapping_sub(self.edge_at) >= self.limits.mark_gap_ms))
        {
            reason = FeedReason::FeedbackTimeout;
        }

        let Some(sample) = sample.filter(|_| reason == FeedReason::None) else {
            self.stop(FeedState::Fault, reason);
            return;
        };

        if !sample.feedback_edge {
            if sample.current_ma > self.limits.current_max_ma {
                self.stop(FeedState::Fault, FeedReason::Overcurrent);
                return;
            }
            let surge = now_ms.wrapping_sub(self.drive_at) < self.limits.surge_ms;
            self.over_limit = if !surge && sample.current_ma > self.limits.current_limit_ma {
                self.over_limit.saturating_add(1)
            } else {
                0
            };
            if self.state == FeedState::Running && self.stuck_low(now_ms, sample.mark_low) {
                self.stop(FeedState::Fault, FeedReason::Sensor);
                return;
            }
        }
        let overcurrent = self.over_limit >= OVERCURRENT_POLLS;

        match self.state {
            // The limit held while backing off or stopped: the jam holds
            // both ways, or the reading is wrong. No more motion either way.
            FeedState::Reversing | FeedState::DirectionPause if overcurrent => {
                self.stop(FeedState::Fault, FeedReason::Overcurrent);
                return;
            }
            FeedState::Reversing => {
                if now_ms.wrapping_sub(self.state_at) >= self.limits.reverse_ms {
                    self.pause(now_ms);
                }
                return;
            }
            FeedState::DirectionPause => {
                if now_ms.wrapping_sub(self.state_at) >= self.limits.direction_pause_ms {
                    self.resume(now_ms);
                }
                return;
            }
            _ => {}
        }

        if overcurrent {
            if self.reverse_attempts >= self.limits.max_reverse_attempts {
                self.stop(FeedState::Fault, FeedReason::Overcurrent);
            } else {
                self.reverse(now_ms);
            }
            return;
        }

        if !sample.feedback_edge {
            return;
        }
        // Any edge shows the rotor turning, qualified or not; one captured
        // before the last resume belongs to the drive before it.
        if captured_at_ms.wrapping_sub(self.edge_at) < HALF_RANGE {
            self.edge_at = captured_at_ms;
        }
        let inhibition = captured_at_ms.wrapping_sub(self.inhibit_at);
        if inhibition >= HALF_RANGE || inhibition < self.limits.feedback_inhibit_ms {
            return;
        }

        let expected_high =
            self.progress.mode == BowlMode::Single || self.progress.direction == Direction::D0;
        if sample.feedback_high != expected_high {
            return;
        }

        self.last_feedback_at = captured_at_ms;
        self.feedback_grace_ms = 0;
        self.inhibit_at = captured_at_ms;
        let result = self.progress.accept();
        if result == ProgressResult::Finished {
            self.stop(FeedState::Complete, FeedReason::Done);
        } else if result == ProgressResult::Portion && self.progress.mode == BowlMode::Double {
            if self.progress.direction == Direction::D0 {
                self.pause(now_ms);
            } else {
                self.motor = MotorCommand::Drive1;
                self.drive_changed(now_ms);
            }
        }
    }

    /// External input/output failures latch a fault only for an active request.
    /// Rejects the non-fault reasons (none, done, cancelled). Does not apply GPIO.
    pub fn trip(&mut self, reason: FeedReason) -> bool {
        if !self.active()
            || matches!(
                reason,
                FeedReason::None | FeedReason::Done | FeedReason::Cancelled
            )
        {
            return false;
        }
        self.stop(FeedState::Fault, reason);
        true
    }

    /// A stale cancellation cannot stop a different request. Terminal output stays STOP.
    pub fn cancel(&mut self, id: u64) -> bool {
        if !self.active() || id != self.last_request_id {
            return false;
        }
        self.stop(FeedState::Cancelled, FeedReason::Cancelled);
        true
    }

    /// Acknowledge a terminal result; never resets IDs or establishes mechanical phase.
    pub fn reset(&mut self) -> bool {
        if self.active() {
            return false;
        }
        self.stop(FeedState::Idle, FeedReason::None);
        true
    }
}
