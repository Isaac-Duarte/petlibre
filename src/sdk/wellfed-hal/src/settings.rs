//! The device settings as the device API sees them. The settings writer
//! (`wellfed-sdk`'s `settings.rs`) publishes what NOR holds; a settings
//! write from an owner's session queues one change for it. Nothing here
//! touches flash, so the API's loop may call it.
//!
//! Both the view and the queued change are whole structs, copied in and out
//! with interrupts masked, so a reader never sees half a publish.

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

pub use wellfed_core::settings::{SettingsFields, SettingsResult, SettingsView, Window};

use crate::cell::TaskCell;
use crate::sdk::IrqGuard;

/// Who holds `PENDING`, as `STATE` stores it.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Hand {
    Empty = 0,
    Filling = 1,
    Queued = 2,
}

const NO_FIELDS: SettingsFields = SettingsFields {
    lights: None,
    sound: None,
    volume: None,
    lights_window: None,
    sound_window: None,
};
const NO_VIEW: SettingsView = SettingsView {
    lights: false,
    sound: false,
    volume: 0,
    lights_window: Window::Always,
    sound_window: Window::Always,
    generation: 0,
    writable: false,
};

/// The queued change; written only by the claimant of `STATE` (`Empty` to
/// `Filling`), read only by the writer after `Queued`.
static PENDING: TaskCell<SettingsFields> = TaskCell::new(NO_FIELDS);
static STATE: AtomicU8 = AtomicU8::new(Hand::Empty as u8);

/// What NOR holds, once the writer has loaded it.
static VIEW: TaskCell<SettingsView> = TaskCell::new(NO_VIEW);
static LOADED: AtomicBool = AtomicBool::new(false);

/// What NOR holds, once the writer has loaded it.
pub fn current() -> Option<SettingsView> {
    if !LOADED.load(Ordering::Acquire) {
        return None;
    }
    let _guard = IrqGuard::lock();
    // SAFETY: interrupts are masked, so this is the only reference.
    Some(unsafe { *VIEW.get() })
}

/// The device API's half: read, or queue one change for the writer.
pub fn request(fields: SettingsFields) -> SettingsResult {
    let Some(view) = current() else {
        return SettingsResult::Busy;
    };
    if !fields.changes() {
        let pending = STATE.load(Ordering::Acquire) != Hand::Empty as u8;
        return SettingsResult::Current { view, pending };
    }
    if !view.writable {
        return SettingsResult::ReadOnly;
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
        return SettingsResult::Busy;
    }
    // SAFETY: `Filling` makes this the only accessor until `Queued`.
    unsafe { *PENDING.get() = fields };
    STATE.store(Hand::Queued as u8, Ordering::Release);
    SettingsResult::Current {
        view,
        pending: true,
    }
}

/// The writer's half: publish what NOR holds.
pub fn publish(view: SettingsView) {
    {
        let _guard = IrqGuard::lock();
        // SAFETY: interrupts are masked, so this is the only reference.
        unsafe { *VIEW.get() = view };
    }
    LOADED.store(true, Ordering::Release);
}

/// The queued change, if any; it stays queued until [`finish`].
pub fn pending() -> Option<SettingsFields> {
    // SAFETY: `Queued` hands the value to the writer until `finish`.
    (STATE.load(Ordering::Acquire) == Hand::Queued as u8).then(|| unsafe { *PENDING.get() })
}

/// The writer applied (or gave up on) the queued change.
pub fn finish() {
    STATE.store(Hand::Empty as u8, Ordering::Release);
}

/// Forget everything (host tests).
pub fn reset() {
    LOADED.store(false, Ordering::Release);
    {
        let _guard = IrqGuard::lock();
        // SAFETY: interrupts are masked, so this is the only reference.
        unsafe { *VIEW.get() = NO_VIEW };
    }
    STATE.store(Hand::Empty as u8, Ordering::Release);
}
