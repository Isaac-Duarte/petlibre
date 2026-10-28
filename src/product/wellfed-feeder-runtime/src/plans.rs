//! The feeding plans between the loop and the writer.
//!
//! Two kinds of change share one slot, both queued from the loop task: a
//! client's new table, through the device API, and the schedule's own
//! claims and passes ([`PlansChange`]). The writer applies each to what
//! NOR holds (`wellfed_feeder::store::plans::Plans::changed`), saves it to
//! storage sectors 11 and 12 and publishes the result ([`publish`]), which
//! the loop reads to decide what runs. Nothing here writes flash or
//! reaches the feeding owner.

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use wellfed_core::store::slot::SlotIo;
use wellfed_device_api::mailbox::{PlansChange, PlansResult, PlansView};
use wellfed_feeder::store::plans::PlansStore;
use wellfed_hal::cell::TaskCell;
use wellfed_hal::sdk::IrqGuard;

/// The plans' slots within the storage region.
pub const PLANS_BASE: u32 = wellfed_hal::plaf103::storage::PLANS.first;

/// Who holds `PENDING`, as `STATE` stores it.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Hand {
    Empty = 0,
    Filling = 1,
    Queued = 2,
}

/// The queued change; written only by the claimant of `STATE` (`Empty` to
/// `Filling`), read only by the writer after `Queued`.
static PENDING: TaskCell<PlansChange> = TaskCell::new(PlansChange::Replace {
    table: [None; wellfed_device_api::mailbox::PLANS],
    at_s: None,
});
static STATE: AtomicU8 = AtomicU8::new(Hand::Empty as u8);

/// What NOR holds, published by the writer, copied in and out with
/// interrupts masked.
static PUBLISHED: TaskCell<PlansView> = TaskCell::new(PlansView {
    table: [None; wellfed_device_api::mailbox::PLANS],
    marks: [0; wellfed_device_api::mailbox::PLANS],
    claim: None,
    revision: 0,
    replaced: 0,
    writable: false,
});
static LOADED: AtomicBool = AtomicBool::new(false);

fn view() -> PlansView {
    let _guard = IrqGuard::lock();
    // SAFETY: interrupts are masked, so this is the only reference.
    unsafe { *PUBLISHED.get() }
}

/// The plans as the writer last published them, and whether a change
/// waits; `Busy` until the writer has opened the store.
pub fn current() -> PlansResult {
    if !LOADED.load(Ordering::Acquire) {
        return PlansResult::Busy;
    }
    PlansResult::Current {
        view: view(),
        pending: STATE.load(Ordering::Acquire) != Hand::Empty as u8,
    }
}

/// Queue one change for the writer.
pub fn change(change: PlansChange) -> PlansResult {
    if !LOADED.load(Ordering::Acquire) {
        return PlansResult::Busy;
    }
    let view = view();
    if !view.writable {
        return PlansResult::ReadOnly;
    }
    if STATE
        .compare_exchange(
            Hand::Empty as u8,
            Hand::Filling as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_err()
    {
        return PlansResult::Busy;
    }
    // SAFETY: `Filling` makes this the only accessor until `Queued`.
    unsafe { *PENDING.get() = change };
    STATE.store(Hand::Queued as u8, Ordering::Release);
    PlansResult::Current {
        view,
        pending: true,
    }
}

/// The writer's half: the queued change, if any.
pub fn pending() -> Option<PlansChange> {
    // SAFETY: `Queued` hands the value to the writer until `finish`.
    (STATE.load(Ordering::Acquire) == Hand::Queued as u8).then(|| unsafe { *PENDING.get() })
}

pub fn finish() {
    STATE.store(Hand::Empty as u8, Ordering::Release);
}

/// The writer publishes what NOR holds; writable only once it is `armed`.
pub fn publish<I: SlotIo>(store: &PlansStore<I>, armed: bool) {
    {
        let _guard = IrqGuard::lock();
        // SAFETY: interrupts are masked, so this is the only reference.
        unsafe { *PUBLISHED.get() = store.view(armed) };
    }
    LOADED.store(true, Ordering::Release);
}

/// Forget everything (host tests).
pub fn reset() {
    {
        let _guard = IrqGuard::lock();
        // SAFETY: interrupts are masked, so this is the only reference.
        unsafe { *PUBLISHED.get() = PlansView::default() };
    }
    LOADED.store(false, Ordering::Release);
    STATE.store(Hand::Empty as u8, Ordering::Release);
}
