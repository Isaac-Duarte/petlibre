//! Durable-storage callback interface used by the feeding owner.
//!
//! The owner reserves each request durably before applying output and
//! records its result afterwards; at boot `recover` returns the ID high-water
//! mark and the mechanical phase. [`crate::store::feed_journal::FeedJournal`]
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
}
