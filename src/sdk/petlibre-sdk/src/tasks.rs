//! The firmware's share of the SDK's eight task slots.
//!
//! In the normal role the runtime holds six slots at its peak: the watchdog
//! feeder, the startup task, the transport and the power monitor run for the
//! whole boot, and confirmation, the settings writer and the offer check
//! overlap in pairs as each starts the next. A firmware that took one of
//! those slots left the build unable to save settings or see an update offer
//! (`fw44-simon-01`, physical), so a firmware's tasks come from this budget,
//! its SDK services' included, and the build's stack audit refuses an image
//! that would exceed it.
//!
//! The budget counts tasks started this boot; a firmware task that ends does
//! not give its share back.

use core::sync::atomic::{AtomicU8, Ordering};

pub const FIRMWARE_TASKS: u8 = 2;

static STARTED: AtomicU8 = AtomicU8::new(0);

/// Takes one share; false once the firmware has used its budget.
pub(crate) fn reserve() -> bool {
    STARTED
        .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
            (n < FIRMWARE_TASKS).then_some(n + 1)
        })
        .is_ok()
}

/// Returns a share whose task could not be created.
pub(crate) fn release() {
    let _ = STARTED.try_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_budget_runs_out_and_a_failed_creation_returns_its_share() {
        STARTED.store(0, Ordering::Release);

        assert!(reserve() && reserve());
        assert!(!reserve());
        release();
        assert!(reserve());
        assert!(!reserve());
    }
}
