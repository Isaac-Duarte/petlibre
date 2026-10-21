//! Bounded queue of captured feedback edges, drained and validated per batch.

use crate::HALF_RANGE;
use crate::feed_controller::{FeedController, FeedReason, FeedSample, FeedState, MotorCommand};

pub const CAPACITY: usize = 32;

c_enum!(FeedbackFault { OK = 0, OVERFLOW = 1, CLOCK = 2, STALE = 3, ARGUMENT = 4 });
c_enum!(FeedbackTakeResult { EMPTY = 0, EVENT = 1, FAILED = 2 });

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeedbackEvent {
    pub captured_at: u32,
    pub high: bool,
}

/// A coherent motor-gate snapshot copied under the board's critical section.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeedbackDrive {
    pub healthy: bool,
    pub applied_known: bool,
    pub applied: MotorCommand,
    pub applied_at: u32,
}

/// Serialize EVERY call across ISR/task contexts. No internal locks/atomics.
#[repr(C)]
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
        if self.fault == FeedbackFault::OK {
            self.fault = fault;
        }
        false
    }

    fn observe(&mut self, now: u32) -> bool {
        if self.fault != FeedbackFault::OK {
            return false;
        }
        if self.capacity == 0
            || usize::from(self.capacity) > CAPACITY
            || self.head >= self.capacity
            || self.count > self.capacity
            || self.max_age_ms == 0
            || self.max_age_ms >= HALF_RANGE
        {
            return self.fail(FeedbackFault::ARGUMENT);
        }
        if now.wrapping_sub(self.observed_at) >= HALF_RANGE {
            return self.fail(FeedbackFault::CLOCK);
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
            return self.fail(FeedbackFault::OVERFLOW);
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
            return (FeedbackTakeResult::FAILED, None);
        }
        if self.count == 0 {
            return (FeedbackTakeResult::EMPTY, None);
        }
        let front = self.events[usize::from(self.head)];
        if now_ms.wrapping_sub(front.captured_at) > self.max_age_ms {
            self.fail(FeedbackFault::STALE);
            return (FeedbackTakeResult::FAILED, None);
        }
        self.head = ((usize::from(self.head) + 1) % usize::from(self.capacity)) as u8;
        self.count -= 1;
        (FeedbackTakeResult::EVENT, Some(front))
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
            self.fail(FeedbackFault::ARGUMENT);
            controller.trip(FeedReason::FEEDBACK_STREAM);
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
                (FeedbackTakeResult::EMPTY, None) => break,
                _ => {
                    controller.trip(FeedReason::FEEDBACK_STREAM);
                    return false;
                }
            }
        }
        let previous = controller.state;
        let command = controller.motor;
        let mut current = *sample;
        current.feedback_edge = false;
        controller.update(now_ms, Some(&current));
        let valid_drive = drive.healthy && drive.applied_known && drive.applied.valid();
        if !valid_drive
            || (previous == FeedState::RUNNING
                && drive.applied != MotorCommand::STOP
                && drive.applied != command)
        {
            controller.trip(FeedReason::OUTPUT);
            return false;
        }
        if previous != FeedState::RUNNING
            || drive.applied != command
            || drive.applied == MotorCommand::STOP
        {
            return true;
        }
        if now_ms.wrapping_sub(drive.applied_at) >= HALF_RANGE {
            controller.trip(FeedReason::CLOCK);
            return false;
        }
        for event in &batch[..count] {
            if controller.state != FeedState::RUNNING || controller.motor != command {
                break;
            }
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
