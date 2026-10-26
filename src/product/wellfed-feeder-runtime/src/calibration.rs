//! The portion calibration between the loop and the writer: what one
//! portion weighs.
//!
//! The device API queues a whole new calibration ([`change`]); the writer
//! saves it to NOR (storage sectors 34 and 35) and publishes what the
//! store holds ([`publish`]), which the loop reads for the portion
//! statistics. Nothing here writes flash, and nothing here reaches the
//! feeding owner: a weight changes what a portion is reported to weigh,
//! never how far the motor turns.

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use wellfed_core::store::slot::SlotIo;
use wellfed_device_api::mailbox::{Calibration, CalibrationResult, CalibrationView};
use wellfed_feeder::store::calibration::CalibrationStore;
use wellfed_hal::cell::TaskCell;
use wellfed_hal::sdk::IrqGuard;

/// The calibration's slots within the storage region.
pub const CALIBRATION_BASE: u32 = wellfed_hal::plaf103::storage::CALIBRATION.first;

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
static PENDING: TaskCell<Calibration> = TaskCell::new(Calibration { portion_mg: None });
static STATE: AtomicU8 = AtomicU8::new(Hand::Empty as u8);

/// What NOR holds, published by the writer, copied in and out with
/// interrupts masked.
static PUBLISHED: TaskCell<Calibration> = TaskCell::new(Calibration { portion_mg: None });
static LOADED: AtomicBool = AtomicBool::new(false);
static WRITABLE: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicU32 = AtomicU32::new(0);

fn view() -> CalibrationView {
    let calibration = {
        let _guard = IrqGuard::lock();
        // SAFETY: interrupts are masked, so this is the only reference.
        unsafe { *PUBLISHED.get() }
    };
    CalibrationView {
        calibration,
        generation: GENERATION.load(Ordering::Acquire),
        writable: WRITABLE.load(Ordering::Acquire),
    }
}

/// The calibration as the writer last published it, and whether a change
/// waits; `Busy` until the writer has opened the store.
pub fn current() -> CalibrationResult {
    if !LOADED.load(Ordering::Acquire) {
        return CalibrationResult::Busy;
    }
    CalibrationResult::Current {
        view: view(),
        pending: STATE.load(Ordering::Acquire) != Hand::Empty as u8,
    }
}

/// The device API's half: queue the whole new calibration for the writer.
pub fn change(calibration: Calibration) -> CalibrationResult {
    if !LOADED.load(Ordering::Acquire) {
        return CalibrationResult::Busy;
    }
    if !WRITABLE.load(Ordering::Acquire) {
        return CalibrationResult::ReadOnly;
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
        return CalibrationResult::Busy;
    }
    // SAFETY: `Filling` makes this the only accessor until `Queued`.
    unsafe { *PENDING.get() = calibration };
    STATE.store(Hand::Queued as u8, Ordering::Release);
    CalibrationResult::Current {
        view: view(),
        pending: true,
    }
}

/// The writer's half: the queued change, if any.
pub fn pending() -> Option<Calibration> {
    // SAFETY: `Queued` hands the value to the writer until `finish`.
    (STATE.load(Ordering::Acquire) == Hand::Queued as u8).then(|| unsafe { *PENDING.get() })
}

pub fn finish() {
    STATE.store(Hand::Empty as u8, Ordering::Release);
}

/// The writer publishes what NOR holds; writable only once it is `armed`.
pub fn publish<I: SlotIo>(store: &CalibrationStore<I>, armed: bool) {
    {
        let _guard = IrqGuard::lock();
        // SAFETY: interrupts are masked, so this is the only reference.
        unsafe { *PUBLISHED.get() = store.calibration };
    }
    // Reported as 32 bits; saturate rather than wrap to an old value.
    let generation = u32::try_from(store.generation).unwrap_or(u32::MAX);
    GENERATION.store(generation, Ordering::Release);
    WRITABLE.store(armed && store.writable(), Ordering::Release);
    LOADED.store(true, Ordering::Release);
}

/// Forget everything (host tests).
pub fn reset() {
    {
        let _guard = IrqGuard::lock();
        // SAFETY: interrupts are masked, so this is the only reference.
        unsafe { *PUBLISHED.get() = Calibration::default() };
    }
    LOADED.store(false, Ordering::Release);
    WRITABLE.store(false, Ordering::Release);
    GENERATION.store(0, Ordering::Release);
    STATE.store(Hand::Empty as u8, Ordering::Release);
}
