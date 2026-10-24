//! Bounded queue of captured feedback edges, drained and validated per batch.
//!
//! Capture contexts push edges with their capture time; the owner drains them
//! in [`FeedbackEvents::process`], which credits only edges captured while
//! the motor gate's applied drive matched the controller's command. Every
//! call must be serialized by the caller, and a queue fault is sticky.

use crate::HALF_RANGE;
use crate::feed::controller::{FeedController, FeedReason, FeedSample, FeedState, MotorCommand};

pub const CAPACITY: usize = 32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum FeedbackFault {
    #[default]
    Ok = 0,
    Overflow = 1,
    Clock = 2,
    Stale = 3,
    Argument = 4,
}

impl FeedbackFault {
    /// The variant's number, as records and reports print it.
    pub const fn code(self) -> u32 {
        self as u32
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum FeedbackTakeResult {
    #[default]
    Empty = 0,
    Event = 1,
    Failed = 2,
}

impl FeedbackTakeResult {
    /// The variant's number, as records and reports print it.
    pub const fn code(self) -> u32 {
        self as u32
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeedbackEvent {
    pub captured_at: u32,
    pub high: bool,
}

/// A coherent motor-gate snapshot copied under the board's critical section.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeedbackDrive {
    pub healthy: bool,
    pub applied_known: bool,
    pub applied: MotorCommand,
    pub applied_at: u32,
}

/// Serialize EVERY call across ISR/task contexts. No internal locks/atomics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeedbackEvents {
    pub events: [FeedbackEvent; CAPACITY],
    pub max_age_ms: u32,
    pub observed_at: u32,
    pub head: u8,
    pub count: u8,
    pub capacity: u8,
    pub fault: FeedbackFault,
}

impl FeedbackEvents {
    /// Initialize only while stopped, with capture disabled. Reinit discards faults.
    pub fn new(capacity: u8, max_age_ms: u32, now_ms: u32) -> Option<Self> {
        let valid = capacity != 0
            && usize::from(capacity) <= CAPACITY
            && max_age_ms != 0
            && max_age_ms < HALF_RANGE;
        valid.then(|| Self {
            capacity,
            max_age_ms,
            observed_at: now_ms,
            ..Self::default()
        })
    }

    pub fn fail(&mut self, fault: FeedbackFault) -> bool {
        if self.fault == FeedbackFault::Ok {
            self.fault = fault;
        }
        false
    }

    fn observe(&mut self, now: u32) -> bool {
        if self.fault != FeedbackFault::Ok {
            return false;
        }
        if self.capacity == 0
            || usize::from(self.capacity) > CAPACITY
            || self.head >= self.capacity
            || self.count > self.capacity
            || self.max_age_ms == 0
            || self.max_age_ms >= HALF_RANGE
        {
            return self.fail(FeedbackFault::Argument);
        }

        if now.wrapping_sub(self.observed_at) >= HALF_RANGE {
            return self.fail(FeedbackFault::Clock);
        }
        self.observed_at = now;
        true
    }

    /// Capture timestamp must be read at ingestion, never restored from cached
    /// data. Failure latches. ISR callers must also trip physical outputs.
    pub fn push(&mut self, captured_at_ms: u32, high: bool) -> bool {
        if !self.observe(captured_at_ms) {
            return false;
        }
        if self.count == self.capacity {
            return self.fail(FeedbackFault::Overflow);
        }

        let index = (usize::from(self.head) + usize::from(self.count)) % usize::from(self.capacity);
        self.events[index] = FeedbackEvent {
            captured_at: captured_at_ms,
            high,
        };
        self.count += 1;
        true
    }

    /// Each event is returned once; EMPTY/FAILED carry no event.
    pub fn take(&mut self, now_ms: u32) -> (FeedbackTakeResult, Option<FeedbackEvent>) {
        if !self.observe(now_ms) {
            return (FeedbackTakeResult::Failed, None);
        }
        if self.count == 0 {
            return (FeedbackTakeResult::Empty, None);
        }

        let front = self.events[usize::from(self.head)];
        if now_ms.wrapping_sub(front.captured_at) > self.max_age_ms {
            self.fail(FeedbackFault::Stale);
            return (FeedbackTakeResult::Failed, None);
        }

        // `head < capacity <= u8::MAX`, so the increment cannot overflow.
        self.head = (self.head + 1) % self.capacity;
        self.count -= 1;
        (FeedbackTakeResult::Event, Some(front))
    }

    /// Bounded owner-side drain. Validates the ENTIRE batch before crediting
    /// feedback. Current health/deadlines are checked first; capture time is
    /// used for inhibition/counts. `max_age_ms` MUST be less than
    /// `feedback_inhibit_ms`, so a batch cannot contain two independently
    /// countable events. Pending/reversed/unverified drive edges are discarded;
    /// gate failures trip the controller.
    ///
    /// `true` means the stream/snapshot was usable, not that a request
    /// succeeded. A sticky queue fault must block NEW requests too.
    pub fn process(
        &mut self,
        controller: &mut FeedController,
        now_ms: u32,
        sample: &FeedSample,
        drive: FeedbackDrive,
    ) -> bool {
        if self.max_age_ms >= controller.limits.feedback_inhibit_ms {
            self.fail(FeedbackFault::Argument);
            controller.trip(FeedReason::FeedbackStream);
            return false;
        }

        let mut batch = [FeedbackEvent::default(); CAPACITY];
        let mut count = 0;
        loop {
            match self.take(now_ms) {
                (_, Some(event)) => {
                    // Bounded by the validated queue capacity.
                    batch[count] = event;
                    count += 1;
                }
                (FeedbackTakeResult::Empty, None) => break,
                _ => {
                    controller.trip(FeedReason::FeedbackStream);
                    return false;
                }
            }
        }

        let previous = controller.state;
        let command = controller.motor;
        let mut current = *sample;
        current.feedback_edge = false;
        controller.update(now_ms, Some(&current));

        let valid_drive = drive.healthy && drive.applied_known;
        if !valid_drive
            || (previous == FeedState::Running
                && drive.applied != MotorCommand::Stop
                && drive.applied != command)
        {
            controller.trip(FeedReason::Output);
            return false;
        }

        // Credit edges only while the gate applies exactly the commanded drive.
        if previous != FeedState::Running
            || drive.applied != command
            || drive.applied == MotorCommand::Stop
        {
            return true;
        }

        if now_ms.wrapping_sub(drive.applied_at) >= HALF_RANGE {
            controller.trip(FeedReason::Clock);
            return false;
        }

        for event in &batch[..count] {
            if controller.state != FeedState::Running || controller.motor != command {
                break;
            }
            // An edge captured before this drive was applied belongs to an earlier output.
            if event.captured_at.wrapping_sub(drive.applied_at) >= HALF_RANGE {
                continue;
            }
            current.feedback_edge = true;
            current.feedback_high = event.high;
            controller.update_captured(now_ms, Some(&current), event.captured_at);
        }
        true
    }
}
