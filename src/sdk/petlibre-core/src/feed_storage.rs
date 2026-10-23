//! Durable-storage callback interface used by the feeding owner.
//!
//! The owner reserves each request durably before applying output and
//! records its result afterwards; at boot `recover` returns the ID high-water
//! mark and the mechanical phase. [`crate::feed_journal::FeedJournal`]
//! provides the implementation.

use core::ffi::c_void;
use core::ptr;

use crate::feed_controller::{FeedReason, FeedRequest};
use crate::feed_progress::Direction;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OwnerRecovery {
    pub high_water_id: u64,
    pub direction: Direction,
    pub credit_next_event: bool,
    pub phase_known: bool,
    pub pending_request: bool,
}

c_enum!(OwnerOutcome { PENDING = 0, DONE = 1, CANCELLED = 2, EXPIRED = 3, FAILED = 4 });

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

pub type RecoverFn =
    unsafe extern "C" fn(context: *mut c_void, recovery: *mut OwnerRecovery) -> bool;
pub type ReserveFn =
    unsafe extern "C" fn(context: *mut c_void, request: *const FeedRequest) -> bool;
pub type FinishFn = unsafe extern "C" fn(
    context: *mut c_void,
    result: *const OwnerResult,
    phase: *const OwnerRecovery,
) -> bool;

/// Mandatory durable boundary, called with interrupts enabled and outputs off.
/// A true return means the transaction survives reset. No automatic retries.
/// Payloads are API values, never a native-struct on-flash format.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct OwnerStorage {
    pub context: *mut c_void,
    pub recover: Option<RecoverFn>,
    pub reserve: Option<ReserveFn>,
    pub finish: Option<FinishFn>,
}

impl Default for OwnerStorage {
    fn default() -> Self {
        Self {
            context: ptr::null_mut(),
            recover: None,
            reserve: None,
            finish: None,
        }
    }
}

#[allow(unsafe_code)]
impl OwnerStorage {
    pub fn complete(&self) -> bool {
        self.recover.is_some() && self.reserve.is_some() && self.finish.is_some()
    }

    /// # Safety
    /// The callbacks must be sound to call with `context`.
    pub unsafe fn recover(&self) -> Option<OwnerRecovery> {
        let mut recovery = OwnerRecovery::default();
        // SAFETY: the caller upholds the callback contract; the pointer is live.
        let ok = self
            .recover
            .is_some_and(|f| unsafe { f(self.context, &mut recovery) });
        ok.then_some(recovery)
    }

    /// Raw C-style recovery: the callback may write `recovery` even when it
    /// reports failure, and that value is kept.
    ///
    /// # Safety
    /// The callbacks must be sound to call with `context`.
    pub unsafe fn recover_into(&self, recovery: &mut OwnerRecovery) -> bool {
        // SAFETY: the caller upholds the callback contract; the pointer is live.
        self.recover
            .is_some_and(|f| unsafe { f(self.context, recovery) })
    }

    /// # Safety
    /// The callbacks must be sound to call with `context`.
    pub unsafe fn reserve(&self, request: &FeedRequest) -> bool {
        // SAFETY: the caller upholds the callback contract; the pointer is live.
        self.reserve
            .is_some_and(|f| unsafe { f(self.context, request) })
    }

    /// # Safety
    /// The callbacks must be sound to call with `context`.
    pub unsafe fn finish(&self, result: &OwnerResult, phase: &OwnerRecovery) -> bool {
        // SAFETY: the caller upholds the callback contract; the pointers are live.
        self.finish
            .is_some_and(|f| unsafe { f(self.context, result, phase) })
    }
}
