//! KM4 supervision: the TIM2 tick and feedback capture, the feeding owner's
//! beat, the watchdog and when it is fed, the breadcrumb a silent reset
//! leaves, and the bench-derived constants they run with.
//!
//! **No store write starts into a reset.** Every store write sets the flash
//! attempt marker for its duration, and a boot that finds the marker set
//! skips arming the flash hooks (once per power cycle), so a write that
//! hangs cannot reset the board over and over. The marker cannot tell a
//! hang inside the write from a reset that merely landed during it, so a
//! reset the runtime decided must not land in one: while the watchdog
//! feeder withholds the watchdog ([`writes_held`]), from the first pass it
//! does not feed, no write turn is granted, no arming starts and an install
//! goes no further. A write already running when the feeding stops has the
//! rest of the watchdog's expiry (at least 2.4 s less one feed period) to
//! finish; writes take tens of milliseconds. So the marker stays set only
//! after a reset the runtime did not decide (a hang, a fault, a lease that
//! halted), which is what it exists for, and a withheld reset never needs
//! to clear it.

pub(crate) mod active;
pub mod beat;
pub mod config;
pub mod crumb;
pub mod events;
pub mod supervise;
pub mod watchdog;

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// The watchdog feeder stops feeding for good (an intentional restart, a
/// refused checkpoint, or a supervision stop that outlasted the progress
/// gap: [`supervised`]), so a reset follows within the watchdog's expiry.
/// The breadcrumb records that the reset is the feeder's own, and the Wi-Fi task leaves the network
/// meanwhile, so the access point does not reject the next boot's
/// association with status 30. Idempotent; nothing here waits.
pub fn stopping() {
    STOPPING.store(true, Ordering::Release);
    crumb::stopping();
    crate::link::leave_for_restart();
}

/// Whether a stop that began at `since_ms` is certain to end
/// in a reset at `now_ms`. `supervise::feed` is no latch: feeding resumes
/// if it turns true again before the watchdog's progress gap passes, so a
/// sub-second false (a beat just past its gap, one pass of unverified
/// outputs) costs nothing. Once the stop has lasted the whole gap, the next
/// checkpoint is refused whatever `feed` says, and the reset comes.
pub fn stop_certain(since_ms: u32, now_ms: u32, gap_ms: u32) -> bool {
    now_ms.wrapping_sub(since_ms) >= gap_ms
}

/// The feeder task's open supervision stop: when it began, while one runs.
static STOP_SINCE: AtomicU32 = AtomicU32::new(0);
static STOP_OPEN: AtomicBool = AtomicBool::new(false);
/// [`stopping`] ran: the reset is certain.
static STOPPING: AtomicBool = AtomicBool::new(false);

/// One feeder pass's `supervise::feed` answer at uptime `now_ms`. A stop
/// that has lasted the watchdog's progress gap is certain to reset, and
/// only then calls [`stopping`]: a transient false must not take the
/// network down for a board that stays up. Feeder task only.
pub fn supervised(feeding: bool, now_ms: u32) {
    if feeding {
        STOP_OPEN.store(false, Ordering::Release);
        return;
    }
    if !STOP_OPEN.swap(true, Ordering::AcqRel) {
        STOP_SINCE.store(now_ms, Ordering::Relaxed);
    }
    let since = STOP_SINCE.load(Ordering::Relaxed);
    if stop_certain(since, now_ms, config::WATCHDOG.max_progress_gap_ms) {
        stopping();
    }
}

/// Whether store writes are held back: the watchdog feeder is not feeding,
/// or a reset is certain. A transient stop holds them for under a second;
/// a certain one until the reset. See the module documentation.
pub fn writes_held() -> bool {
    STOP_OPEN.load(Ordering::Acquire) || STOPPING.load(Ordering::Acquire)
}

/// Host tests only: no stop open, none certain.
#[cfg(not(target_os = "none"))]
pub fn reset() {
    STOP_OPEN.store(false, Ordering::Relaxed);
    STOPPING.store(false, Ordering::Relaxed);
}
