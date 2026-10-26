//! KM4 supervision: the TIM2 tick and feedback capture, the feeding owner's
//! beat, the watchdog and when it is fed, the breadcrumb a silent reset
//! leaves, and the bench-derived constants they run with.

pub(crate) mod active;
pub mod beat;
pub mod config;
pub mod crumb;
pub mod events;
pub mod supervise;
pub mod watchdog;

/// The watchdog feeder stops feeding for good (an intentional restart, a
/// refused checkpoint, or a supervision stop that outlasted the progress
/// gap: [`supervised`]), so a reset follows within the watchdog's expiry. The breadcrumb records that the
/// reset is the feeder's own, and the Wi-Fi task leaves the network
/// meanwhile, so the access point does not reject the next boot's
/// association with status 30. Idempotent; nothing here waits.
pub fn stopping() {
    crumb::stopping();
    crate::link::leave_for_restart();
}

/// Whether a supervision stop that began at `since_ms` is certain to end
/// in a reset at `now_ms`. `supervise::feed` is no latch: feeding resumes
/// if it turns true again before the watchdog's progress gap passes, so a
/// sub-second false (a beat just past its gap, one pass of unverified
/// outputs) costs nothing. Once the stop has lasted the whole gap, the next
/// checkpoint is refused whatever `feed` says, and the reset comes.
pub fn stop_certain(since_ms: u32, now_ms: u32, gap_ms: u32) -> bool {
    now_ms.wrapping_sub(since_ms) >= gap_ms
}

/// The feeder task's open supervision stop: when it began, while one runs.
static STOP_SINCE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static STOP_OPEN: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// One feeder pass's `supervise::feed` answer at uptime `now_ms`. A stop
/// that has lasted the watchdog's progress gap is certain to reset, and
/// only then calls [`stopping`]: a transient false must not take the
/// network down for a board that stays up. Feeder task only.
pub fn supervised(feeding: bool, now_ms: u32) {
    use core::sync::atomic::Ordering;

    if feeding {
        STOP_OPEN.store(false, Ordering::Relaxed);
        return;
    }
    if !STOP_OPEN.swap(true, Ordering::Relaxed) {
        STOP_SINCE.store(now_ms, Ordering::Relaxed);
    }
    let since = STOP_SINCE.load(Ordering::Relaxed);
    if stop_certain(since, now_ms, config::WATCHDOG.max_progress_gap_ms) {
        stopping();
    }
}

/// Host tests only: no stop open.
#[cfg(not(target_os = "none"))]
pub fn reset() {
    STOP_OPEN.store(false, core::sync::atomic::Ordering::Relaxed);
}
