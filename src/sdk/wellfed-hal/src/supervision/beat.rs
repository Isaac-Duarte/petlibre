//! The feeding owner's liveness, as the runtime's watchdog feeder reads it.
//!
//! The runtime alone owns the hardware watchdog. The owner, stepped from a
//! firmware task, publishes a beat each time it would have refreshed it: a
//! timestamp, the gap it promises not to exceed, and whether it is
//! supervising outputs it drives. The feeder withholds the refresh once the
//! beat is older than its gap, so a stalled owner still ends in a reset,
//! and while the owner supervises a feed, the feeder trusts that supervisor
//! instead of requiring the outputs to read stopped.

/// One published beat, in uptime milliseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Beat {
    pub at_ms: u32,
    /// The owner's longest promised gap between beats.
    pub gap_ms: u32,
    /// The owner held its supervisor open and healthy over an active feed
    /// phase at this beat.
    pub supervised: bool,
    /// Beats since the owner opened; wraps. Diagnostic only.
    pub count: u32,
}

impl Beat {
    /// Within its promised gap at `now_ms`.
    pub fn fresh(&self, now_ms: u32) -> bool {
        now_ms.wrapping_sub(self.at_ms) < self.gap_ms
    }
}

#[cfg(target_os = "none")]
fn with<R>(f: impl FnOnce(&mut Option<Beat>) -> R) -> R {
    use crate::cell::TaskCell;
    use crate::sdk::IrqGuard;

    /// Written by the owner's task, read by the watchdog feeder's.
    static BEAT: TaskCell<Option<Beat>> = TaskCell::new(None);

    let _guard = IrqGuard::lock();
    // SAFETY: every access is inside the guard, which on the single KM4 core
    // excludes every other task, and none outlives `f`.
    f(unsafe { &mut *BEAT.get() })
}

/// One per host thread, like the fake hardware each test owns.
#[cfg(not(target_os = "none"))]
fn with<R>(f: impl FnOnce(&mut Option<Beat>) -> R) -> R {
    use core::cell::RefCell;

    std::thread_local! {
        static BEAT: RefCell<Option<Beat>> = const { RefCell::new(None) };
    }
    BEAT.with_borrow_mut(f)
}

/// An owner opened: it counts as fresh until `gap_ms` passes without a beat.
pub(crate) fn open(now_ms: u32, gap_ms: u32) {
    with(|beat| {
        *beat = Some(Beat {
            at_ms: now_ms,
            gap_ms,
            supervised: false,
            count: 0,
        });
    });
}

pub(crate) fn beat(now_ms: u32, supervised: bool) {
    with(|beat| {
        if let Some(beat) = beat {
            beat.at_ms = now_ms;
            beat.supervised = supervised;
            beat.count = beat.count.wrapping_add(1);
        }
    });
}

/// The owner stopped: the feeder goes back to its own output check.
pub(crate) fn close() {
    with(|beat| *beat = None);
}

/// The current beat; `None` while no owner runs.
pub fn read() -> Option<Beat> {
    with(|beat| *beat)
}
