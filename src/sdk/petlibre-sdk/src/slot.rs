//! Fixed-address task storage for the runtime's and firmwares' tasks.

use core::cell::UnsafeCell;

/// Fixed-address task storage, built on first use; each static names the
/// task that owns it.
///
/// Not a `cell::TaskCell`: several probes keep raw C context pointers (the
/// board, the flash coordinator, `JournalIo`), so they are not `Send`.
/// Those pointers stay with the owning task like the rest of the value.
pub struct Slot<T>(pub(crate) UnsafeCell<Option<T>>);

// SAFETY: each slot's value is touched only by the task its static names
// (see `get_or_try_init`); the hooks reach the runtime's provider only
// through the context pointer its owner installed.
unsafe impl<T> Sync for Slot<T> {}

impl<T> Slot<T> {
    #[expect(
        clippy::new_without_default,
        reason = "slots are statics, built by the const constructor"
    )]
    pub const fn new() -> Self {
        Self(UnsafeCell::new(None))
    }

    /// The value, built on first use by `make` in a frame of its own. Built
    /// inline, a large value's temporary stays reserved in the task's frame
    /// beneath every later step: after the crate split, LTO inlined the
    /// optics probe's 3 KiB default and the task overflowed its 4 KiB stack.
    ///
    /// # Safety
    /// Only the slot's owning task calls this, with no other reference into
    /// the slot live.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn get_or_try_init(&self, make: impl FnOnce() -> Option<T>) -> Option<&mut T> {
        // SAFETY: the caller is the slot's only user.
        let slot = unsafe { &mut *self.0.get() };
        if slot.is_none() {
            fill(slot, make);
        }
        slot.as_mut()
    }

    /// As `get_or_try_init`, for a constructor that cannot fail.
    ///
    /// # Safety
    /// As for `get_or_try_init`.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn get_or_init(&self, make: impl FnOnce() -> T) -> &mut T {
        // SAFETY: the caller's contract.
        match unsafe { self.get_or_try_init(|| Some(make())) } {
            Some(value) => value,
            None => unreachable!(),
        }
    }
}

#[inline(never)]
#[cold]
fn fill<T>(slot: &mut Option<T>, make: impl FnOnce() -> Option<T>) {
    *slot = make();
}
