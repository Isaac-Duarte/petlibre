//! The panel's child lock between the device API and the task that owns
//! the panel.
//!
//! The lock itself is firmware policy (`wellfed-feeder`); this module only
//! carries it across tasks. The owner publishes the lock it holds with
//! [`publish`] and takes queued changes with [`take`]. A signed `lock`
//! request reaches only [`request`], which reads the published lock or
//! queues one change in a one-slot mailbox, so a second change before the
//! owner takes the first is `busy`, never queued behind it. A firmware that
//! runs no panel owner never publishes, and every request is `unserved`.

use core::sync::atomic::{AtomicU8, Ordering};

use wellfed_feeder::mailbox::{LockFields, LockResult};

const UNSERVED: u8 = 0;
const LOCKED: u8 = 1;
const UNLOCKED: u8 = 2;

const EMPTY: u8 = 0;

/// `UNSERVED`, or the lock the owner last published.
static STATE: AtomicU8 = AtomicU8::new(UNSERVED);
/// `EMPTY`, or the queued change as `LOCKED` or `UNLOCKED`.
static QUEUED: AtomicU8 = AtomicU8::new(EMPTY);

const fn code(locked: bool) -> u8 {
    if locked { LOCKED } else { UNLOCKED }
}

/// The owner's half: the lock it now holds. The first call starts serving
/// requests.
pub fn publish(locked: bool) {
    STATE.store(code(locked), Ordering::Release);
}

/// The owner's half: the queued change, if any, which frees the slot.
pub fn take() -> Option<bool> {
    match QUEUED.swap(EMPTY, Ordering::AcqRel) {
        LOCKED => Some(true),
        UNLOCKED => Some(false),
        _ => None,
    }
}

/// The device API's half: read the lock, or queue a change for the owner.
pub fn request(fields: LockFields) -> LockResult {
    let locked = match STATE.load(Ordering::Acquire) {
        LOCKED => true,
        UNLOCKED => false,
        _ => return LockResult::Unserved,
    };
    let Some(want) = fields.locked else {
        return LockResult::Current {
            locked,
            pending: QUEUED.load(Ordering::Acquire) != EMPTY,
        };
    };

    match QUEUED.compare_exchange(EMPTY, code(want), Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => LockResult::Current {
            locked,
            pending: true,
        },
        Err(_) => LockResult::Busy,
    }
}

/// Forget everything (host tests).
pub fn reset() {
    STATE.store(UNSERVED, Ordering::Release);
    QUEUED.store(EMPTY, Ordering::Release);
}
