//! The time settings between the loop and the writer: the owner's zone and
//! the NTP server.
//!
//! The device API queues a whole new settings value ([`change`]); the
//! writer saves it to NOR (storage sectors 27 and 28) and publishes what the
//! store holds ([`publish`]), which the loop reads each pass to set the
//! clock's zone rule and the NTP exchange's server. Nothing here writes
//! flash.

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use wellfed_core::store::slot::SlotIo;
use wellfed_device_api::mailbox::{TimeSettings, TimeSettingsResult, TimeSettingsView};
use wellfed_feeder::store::time::TimeStore;
use wellfed_hal::cell::TaskCell;
use wellfed_hal::sdk::IrqGuard;

/// The time settings' slots within the storage region.
pub const TIME_BASE: u32 = wellfed_hal::plaf103::storage::TIME.first;

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
static PENDING: TaskCell<TimeSettings> = TaskCell::new(TimeSettings {
    zone: None,
    ntp: wellfed_device_api::mailbox::NtpServer::Off,
});
static STATE: AtomicU8 = AtomicU8::new(Hand::Empty as u8);

/// What NOR holds, published by the writer. The settings are larger than
/// an atomic, so they are copied in and out with interrupts masked.
static PUBLISHED: TaskCell<TimeSettings> = TaskCell::new(TimeSettings {
    zone: None,
    ntp: wellfed_device_api::mailbox::NtpServer::Off,
});
static LOADED: AtomicBool = AtomicBool::new(false);
static WRITABLE: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicU32 = AtomicU32::new(0);

fn view() -> TimeSettingsView {
    let settings = {
        let _guard = IrqGuard::lock();
        // SAFETY: interrupts are masked, so this is the only reference.
        unsafe { *PUBLISHED.get() }
    };
    TimeSettingsView {
        settings,
        generation: GENERATION.load(Ordering::Acquire),
        writable: WRITABLE.load(Ordering::Acquire),
    }
}

/// The settings as the writer last published them, and whether a change
/// waits; `Busy` until the writer has opened the store.
pub fn current() -> TimeSettingsResult {
    if !LOADED.load(Ordering::Acquire) {
        return TimeSettingsResult::Busy;
    }
    TimeSettingsResult::Current {
        view: view(),
        pending: STATE.load(Ordering::Acquire) != Hand::Empty as u8,
    }
}

/// The device API's half: queue the whole new settings for the writer.
pub fn change(settings: TimeSettings) -> TimeSettingsResult {
    if !LOADED.load(Ordering::Acquire) {
        return TimeSettingsResult::Busy;
    }
    if !WRITABLE.load(Ordering::Acquire) {
        return TimeSettingsResult::ReadOnly;
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
        return TimeSettingsResult::Busy;
    }
    // SAFETY: `Filling` makes this the only accessor until `Queued`.
    unsafe { *PENDING.get() = settings };
    STATE.store(Hand::Queued as u8, Ordering::Release);
    TimeSettingsResult::Current {
        view: view(),
        pending: true,
    }
}

/// The writer's half: the queued change, if any.
pub fn pending() -> Option<TimeSettings> {
    // SAFETY: `Queued` hands the value to the writer until `finish`.
    (STATE.load(Ordering::Acquire) == Hand::Queued as u8).then(|| unsafe { *PENDING.get() })
}

pub fn finish() {
    STATE.store(Hand::Empty as u8, Ordering::Release);
}

/// The writer publishes what NOR holds; writable only once it is `armed`.
pub fn publish<I: SlotIo>(store: &TimeStore<I>, armed: bool) {
    {
        let _guard = IrqGuard::lock();
        // SAFETY: interrupts are masked, so this is the only reference.
        unsafe { *PUBLISHED.get() = store.settings };
    }
    // Reported as 32 bits; saturate rather than wrap to an old value.
    let generation = u32::try_from(store.generation).unwrap_or(u32::MAX);
    GENERATION.store(generation, Ordering::Release);
    WRITABLE.store(armed && store.writable(), Ordering::Release);
    LOADED.store(true, Ordering::Release);
}

/// Forget everything (host tests).
pub fn reset() {
    LOADED.store(false, Ordering::Release);
    WRITABLE.store(false, Ordering::Release);
    GENERATION.store(0, Ordering::Release);
    STATE.store(Hand::Empty as u8, Ordering::Release);
}
