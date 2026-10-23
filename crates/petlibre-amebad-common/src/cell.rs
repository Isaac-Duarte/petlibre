//! The one shared-static cell the ports use for state owned by a single task
//! or interrupt-serialized code.
//!
//! FreeRTOS tasks, the IPC handler and the C shim's callbacks reach their
//! state through statics, so the cell must be `Sync`. It adds no
//! synchronization of its own: each static's declaration names the task or
//! critical section that owns it, and every dereference of [`TaskCell::get`]
//! states which of those it relies on. `T: Send` is required because the
//! owner is generally not the context that initialized the static.

use core::cell::UnsafeCell;

#[repr(transparent)]
pub struct TaskCell<T>(UnsafeCell<T>);

// SAFETY: `TaskCell` only hands out raw pointers. Dereferencing one is
// `unsafe`, and its caller is the single owner named at the static (or holds
// the critical section that serializes it), so the value moves between
// contexts but is never shared; that requires `T: Send`.
unsafe impl<T: Send> Sync for TaskCell<T> {}

impl<T> TaskCell<T> {
    pub const fn new(value: T) -> Self {
        Self(UnsafeCell::new(value))
    }

    /// The value's address. Dereference it only from the owning context.
    #[inline(always)]
    pub const fn get(&self) -> *mut T {
        self.0.get()
    }
}
