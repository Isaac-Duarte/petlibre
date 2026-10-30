//! The firmware's share of the SDK's eight task slots.
//!
//! On either bank the runtime holds three slots for the whole boot: the
//! watchdog feeder, the startup task and the service task, whose stages are
//! the power monitor, confirmation, the settings writer and the update
//! stage.
//! The firmware gets four, leaving one spare. When those stages were tasks
//! of their own, a firmware that took one of their slots left the build
//! unable to save settings or see an update offer (a physical run; see
//! RESUME), so a firmware's tasks come from this budget,
//! its SDK services' included, and the build's stack audit refuses an image
//! that would exceed it.
//!
//! The budget counts tasks started this boot; a firmware task that ends does
//! not give its share back.

use core::ffi::{CStr, c_void};
use core::sync::atomic::{AtomicU8, Ordering};

pub const FIRMWARE_TASKS: u8 = 4;

/// A task the firmware can start with `wellfed_sdk::spawn`, made by
/// [`#[task]`](crate::task). Its step is a safe function that ignores its
/// context, so the runtime may call it with none, again and again, until it
/// returns zero.
#[derive(Clone, Copy, Debug)]
pub struct Task {
    pub(crate) name: &'static CStr,
    pub(crate) stack_words: u16,
    pub(crate) priority: u32,
    pub(crate) step: extern "C" fn(*mut c_void) -> u32,
}

/// The watchdog feeder's FreeRTOS priority, above every other task the
/// runtime or a firmware starts (the runtime creates it with this value
/// written out, for the stack audit). Firmware tasks are refused at or
/// above it.
pub(crate) const FEEDER_PRIORITY: u32 = 7;

impl Task {
    /// Only `#[task]` calls this: the stack audit reads a task's stack from
    /// the attribute, and refuses a task made any other way.
    #[doc(hidden)]
    pub const fn __new(
        name: &'static CStr,
        stack_words: u16,
        priority: u32,
        step: extern "C" fn(*mut c_void) -> u32,
    ) -> Self {
        // A firmware task at the feeder's priority would share its time
        // slices, and one above it could starve it unseen: the feeder sits
        // above every other task so a hang shows as a missed beat.
        assert!(
            priority < FEEDER_PRIORITY,
            "a firmware task runs below the watchdog feeder's priority 7"
        );
        Self {
            name,
            stack_words,
            priority,
            step,
        }
    }

    pub fn name(&self) -> &'static CStr {
        self.name
    }

    pub fn stack_words(&self) -> u16 {
        self.stack_words
    }

    pub fn priority(&self) -> u32 {
        self.priority
    }

    /// Runs one step, as the task would, and returns its delay. For host
    /// tests: on the device only the task itself steps it.
    pub fn step(&self) -> u32 {
        (self.step)(core::ptr::null_mut())
    }
}

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

    /// The runtime creates the feeder with the priority written out, for
    /// the stack audit; it must stay the one the task cap compares with.
    #[test]
    fn the_feeder_is_created_at_the_capped_priority() {
        let runtime = include_str!("runtime.rs");
        let created = format!("task_create(c\"wellfed-wdg\", 512, {FEEDER_PRIORITY}, feeder_task");
        assert!(runtime.contains(&created), "runtime.rs: {created}");
    }
}
