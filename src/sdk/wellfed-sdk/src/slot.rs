//! Fixed-address task storage for the runtime's and firmwares' tasks.
//!
//! A task's step is called afresh each iteration, so its state lives in a
//! static. A [`Slot`] belongs to the first task that enters it: entry from
//! any other task, or a second entry while the first guard lives, is refused
//! with `None`. A step treats that as the end of its task, so a wiring
//! mistake stops the task (and the supervisor sees it) rather than aliasing
//! state across tasks.

use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use wellfed_hal::sdk;

/// Fixed-address state owned by one task.
///
/// Not a `cell::TaskCell`: several probes keep raw C context pointers (the
/// board, the flash coordinator, `JournalIo`), so they are not `Send`. A
/// slot needs no `Send` because its value never leaves the task that
/// claimed it, except through [`Slot::release`], which requires it.
pub struct Slot<T> {
    /// The owning task's identity; zero while unclaimed.
    owner: AtomicUsize,
    /// A guard is live.
    entered: AtomicBool,
    value: UnsafeCell<Option<T>>,
}

// SAFETY: only the owning task reaches the value, and only through a guard,
// of which at most one lives at a time; `release` hands the value to
// another task only for `T: Send`.
unsafe impl<T> Sync for Slot<T> {}

impl<T> Slot<T> {
    #[expect(
        clippy::new_without_default,
        reason = "slots are statics, built by the const constructor"
    )]
    pub const fn new() -> Self {
        Self {
            owner: AtomicUsize::new(0),
            entered: AtomicBool::new(false),
            value: UnsafeCell::new(None),
        }
    }

    /// A slot built at compile time, for a value too large to build on a
    /// task's stack even once: it starts in static RAM and is never moved.
    /// Enter it with [`Self::enter`], which links no constructor.
    pub const fn filled(value: T) -> Self {
        Self {
            owner: AtomicUsize::new(0),
            entered: AtomicBool::new(false),
            value: UnsafeCell::new(Some(value)),
        }
    }

    /// The value, for the owning task; `None` for another task, a second
    /// entry, or a slot never filled.
    pub fn enter(&self) -> Option<SlotGuard<'_, T>> {
        let guard = self.claim()?;
        // SAFETY: the guard is the only access to the value while it lives.
        unsafe { &*self.value.get() }.as_ref()?;
        Some(guard)
    }

    /// As [`Self::enter`], building the value on first use by `make` in a
    /// frame of its own. Built inline, a large value's temporary stays
    /// reserved in the task's frame beneath every later step: after the
    /// crate split, LTO inlined the optics probe's 3 KiB default and the
    /// task overflowed its 4 KiB stack. A `make` that returns `None` leaves
    /// the slot empty, to be tried again on a later entry.
    pub fn enter_or_try_init(&self, make: impl FnOnce() -> Option<T>) -> Option<SlotGuard<'_, T>> {
        let guard = self.claim()?;
        // SAFETY: the guard is the only access to the value while it lives.
        let value = unsafe { &mut *self.value.get() };
        if value.is_none() {
            fill(value, make);
        }
        value.as_ref()?;
        Some(guard)
    }

    /// As [`Self::enter_or_try_init`], for a constructor that cannot fail.
    pub fn enter_or_init(&self, make: impl FnOnce() -> T) -> Option<SlotGuard<'_, T>> {
        self.enter_or_try_init(|| Some(make()))
    }

    fn claim(&self) -> Option<SlotGuard<'_, T>> {
        let task = sdk::current_task();
        if task == 0 {
            return None;
        }
        match self
            .owner
            .compare_exchange(0, task, Ordering::Acquire, Ordering::Acquire)
        {
            Ok(_) => {}
            Err(owner) if owner == task => {}
            Err(_) => return None,
        }
        if self.entered.swap(true, Ordering::Acquire) {
            return None;
        }
        Some(SlotGuard {
            slot: self,
            _task: PhantomData,
        })
    }
}

impl<T: Send> Slot<T> {
    /// Hands the slot to whichever task enters it next: for state one task
    /// builds and another runs (a start hook filling a service's state, the
    /// watchdog opened by one task and fed by another). Refused, returning
    /// false, from a task that does not own the slot or while it is entered.
    pub fn release(&self) -> bool {
        let task = sdk::current_task();
        if task == 0 || self.owner.load(Ordering::Relaxed) != task {
            return false;
        }
        if self.entered.load(Ordering::Relaxed) {
            return false;
        }
        // Release: the next owner's claim (Acquire) sees every write made
        // through this task's guards.
        self.owner.store(0, Ordering::Release);
        true
    }
}

/// The owning task's access to a slot's value; the slot can be entered again
/// once it drops. Not `Send`, so it cannot carry the value to another task.
pub struct SlotGuard<'a, T> {
    slot: &'a Slot<T>,
    _task: PhantomData<*mut ()>,
}

impl<T> Deref for SlotGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: entry checked the value is present and nothing empties a
        // slot; the guard is the only access while it lives.
        unsafe { (*self.slot.value.get()).as_ref().unwrap_unchecked() }
    }
}

impl<T> DerefMut for SlotGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as for `deref`, and `&mut self` makes this borrow unique.
        unsafe { (*self.slot.value.get()).as_mut().unwrap_unchecked() }
    }
}

impl<T> Drop for SlotGuard<'_, T> {
    fn drop(&mut self) {
        self.slot.entered.store(false, Ordering::Release);
    }
}

#[inline(never)]
#[cold]
fn fill<T>(slot: &mut Option<T>, make: impl FnOnce() -> Option<T>) {
    *slot = make();
}
