//! The device settings as the transport sees them. The settings writer
//! (firstboot `settings.rs`) publishes what NOR holds; a signed `settings`
//! request queues one change for it. Nothing here touches flash, so the
//! transport may call it (see `cargo xtask check-repository`).

use core::sync::atomic::{AtomicU32, Ordering};

use wellfed_core::transport::{SettingsFields, SettingsResult, SettingsView};

const LOADED: u32 = 1 << 31;
const WRITABLE: u32 = 1 << 30;
const LIGHTS: u32 = 1 << 8;
const SOUND: u32 = 1 << 9;

const QUEUED: u32 = 1 << 31;
const SET_LIGHTS: u32 = 1 << 16;
const SET_SOUND: u32 = 1 << 17;
const SET_VOLUME: u32 = 1 << 18;

/// `LOADED | WRITABLE | LIGHTS | SOUND | volume`.
static VIEW: AtomicU32 = AtomicU32::new(0);
static GENERATION: AtomicU32 = AtomicU32::new(0);
/// One queued change: `QUEUED`, which fields, and their values.
static PENDING: AtomicU32 = AtomicU32::new(0);

/// What NOR holds, once the writer has loaded it.
pub fn current() -> Option<SettingsView> {
    let packed = VIEW.load(Ordering::Acquire);
    (packed & LOADED != 0).then(|| SettingsView {
        lights: packed & LIGHTS != 0,
        sound: packed & SOUND != 0,
        // The volume is the packed word's low byte.
        volume: packed.to_le_bytes()[0],
        generation: GENERATION.load(Ordering::Acquire),
        writable: packed & WRITABLE != 0,
    })
}

/// The transport's half: read, or queue one change for the writer.
pub fn request(fields: SettingsFields) -> SettingsResult {
    let Some(view) = current() else {
        return SettingsResult::Busy;
    };
    if !fields.changes() {
        let pending = PENDING.load(Ordering::Acquire) != 0;
        return SettingsResult::Current { view, pending };
    }
    if !view.writable {
        return SettingsResult::ReadOnly;
    }

    let mut packed = QUEUED;
    if let Some(on) = fields.lights {
        packed |= SET_LIGHTS | if on { LIGHTS } else { 0 };
    }
    if let Some(on) = fields.sound {
        packed |= SET_SOUND | if on { SOUND } else { 0 };
    }
    if let Some(volume) = fields.volume {
        packed |= SET_VOLUME | u32::from(volume);
    }
    match PENDING.compare_exchange(0, packed, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => SettingsResult::Current {
            view,
            pending: true,
        },
        Err(_) => SettingsResult::Busy,
    }
}

/// The writer's half: publish what NOR holds.
pub fn publish(view: SettingsView) {
    GENERATION.store(view.generation, Ordering::Release);
    let packed = LOADED
        | if view.writable { WRITABLE } else { 0 }
        | if view.lights { LIGHTS } else { 0 }
        | if view.sound { SOUND } else { 0 }
        | u32::from(view.volume);
    VIEW.store(packed, Ordering::Release);
}

/// The queued change, if any; it stays queued until [`finish`].
pub fn pending() -> Option<SettingsFields> {
    let packed = PENDING.load(Ordering::Acquire);
    (packed & QUEUED != 0).then(|| SettingsFields {
        lights: (packed & SET_LIGHTS != 0).then_some(packed & LIGHTS != 0),
        sound: (packed & SET_SOUND != 0).then_some(packed & SOUND != 0),
        volume: (packed & SET_VOLUME != 0).then_some(packed.to_le_bytes()[0]),
    })
}

/// The writer applied (or gave up on) the queued change.
pub fn finish() {
    PENDING.store(0, Ordering::Release);
}

/// Forget everything (host tests).
pub fn reset() {
    VIEW.store(0, Ordering::Release);
    GENERATION.store(0, Ordering::Release);
    PENDING.store(0, Ordering::Release);
}
