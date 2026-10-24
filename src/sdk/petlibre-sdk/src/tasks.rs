//! The firmware's share of the SDK's eight task slots.
//!
//! In the normal role the runtime holds three slots for the whole boot: the
//! watchdog feeder, the startup task and the service task, whose stages are
//! the power monitor, confirmation, the settings writer and the offer check.
//! The firmware gets four, leaving one spare. When those stages were tasks
//! of their own, a firmware that took one of their slots left the build
//! unable to save settings or see an update offer (a physical run; see
//! RESUME), so a firmware's tasks come from this budget,
//! its SDK services' included, and the build's stack audit refuses an image
//! that would exceed it.
//!
//! The budget counts tasks started this boot; a firmware task that ends does
//! not give its share back.

use core::sync::atomic::{AtomicU8, Ordering};

pub const FIRMWARE_TASKS: u8 = 4;

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

        assert!((0..FIRMWARE_TASKS).all(|_| reserve()));
        assert!(!reserve());
        release();
        assert!(reserve());
        assert!(!reserve());
    }
}
