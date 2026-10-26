//! Durable-storage callback interface used by the feeding owner.
//!
//! The owner reserves each request durably before applying output and
//! records its result afterwards; at boot `recover` returns the ID high-water
//! mark and the mechanical phase, and `resolve` records a reservation that
//! a reset interrupted as uncertain. [`crate::store::feed_journal::FeedJournal`]
//! provides the implementation.

use crate::feed::controller::{FeedReason, FeedRequest};
use crate::feed::progress::Direction;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OwnerRecovery {
    pub high_water_id: u64,
    pub direction: Direction,
    pub credit_next_event: bool,
    pub phase_known: bool,
    pub pending_request: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum OwnerOutcome {
    #[default]
    Pending = 0,
    Done = 1,
    Cancelled = 2,
    Expired = 3,
    Failed = 4,
    /// A reservation a reset interrupted, resolved at the next boot: the
    /// feed may have dispensed anything up to what it asked for, and is
    /// never retried.
    Uncertain = 5,
}

impl OwnerOutcome {
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
            0 => Some(Self::Pending),
            1 => Some(Self::Done),
            2 => Some(Self::Cancelled),
            3 => Some(Self::Expired),
            4 => Some(Self::Failed),
            5 => Some(Self::Uncertain),
            _ => None,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OwnerResult {
    pub id: u64,
    pub requested: u16,
    pub completed: u16,
    pub outcome: OwnerOutcome,
    pub reason: FeedReason,
    pub motion_possible: bool,
}

/// Where a single bowl's rotor rests between feeds. A drive that ends on a
/// credited portion stops a few milliseconds past a mark; any other drive
/// may stop anywhere between two. Never stored: the feed journal's last
/// result says which ([`Self::after`]), so the journal's format is
/// unchanged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum RotorPhase {
    /// Unknown until a drive ends at a mark: the next feed homes it.
    #[default]
    Unknown = 0,
    AtMark = 1,
}

impl RotorPhase {
    /// The variant's number, as records and reports print it.
    pub const fn code(self) -> u32 {
        self as u32
    }

    /// The phase after a feed that ended in `result`, the phase before it
    /// being `before`: a completed feed stopped at its last portion's mark,
    /// a feed that never moved left the rotor where it was, and any other
    /// ending (cancelled, failed or uncertain after motion) may have
    /// stopped it anywhere.
    pub const fn after(before: Self, result: &OwnerResult) -> Self {
        match result.outcome {
            OwnerOutcome::Done => Self::AtMark,
            _ if !result.motion_possible => before,
            _ => Self::Unknown,
        }
    }
}

/// Mandatory durable boundary, called with interrupts enabled and outputs off.
/// A true return means the transaction survives reset. No automatic retries.
/// Payloads are API values, never a native-struct on-flash format.
pub trait OwnerStorage {
    /// The ID high-water mark and mechanical phase at boot. It may write
    /// `recovery` even when it reports failure, and the owner keeps that
    /// value.
    fn recover(&mut self, recovery: &mut OwnerRecovery) -> bool;
    /// Make `request` durable before any output is applied.
    fn reserve(&mut self, request: &FeedRequest) -> bool;
    /// Record the reserved request's result and the phase after it.
    fn finish(&mut self, result: &OwnerResult, phase: &OwnerRecovery) -> bool;
    /// Record the reservation an earlier boot left pending as an
    /// [`OwnerOutcome::Uncertain`] result, keeping its ID as the high-water
    /// mark: the result, once durable. `recover` then reports the phase
    /// after it. Only a reservation recovered at boot can be resolved.
    fn resolve(&mut self) -> Option<OwnerResult>;
    /// The last result the journal holds, `None` before the first: what
    /// the rotor's phase at boot follows from ([`RotorPhase::after`]).
    fn last_result(&self) -> Option<OwnerResult>;
}
