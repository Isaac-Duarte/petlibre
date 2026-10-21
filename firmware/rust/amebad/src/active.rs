//! Single-instance registration pointers read by interrupt handlers.
//!
//! Target builds use one global; host builds use one per thread so tests,
//! each with its own fake hardware, can run in parallel.

macro_rules! active_pointer {
    ($ty:ty) => {
        #[cfg(target_os = "none")]
        static ACTIVE: core::sync::atomic::AtomicPtr<$ty> = core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());

        #[cfg(target_os = "none")]
        fn active() -> *mut $ty {
            ACTIVE.load(core::sync::atomic::Ordering::Acquire)
        }

        #[cfg(target_os = "none")]
        fn set_active(value: *mut $ty) {
            ACTIVE.store(value, core::sync::atomic::Ordering::Release);
        }

        #[cfg(not(target_os = "none"))]
        std::thread_local! {
            static ACTIVE: core::cell::Cell<*mut $ty> = const { core::cell::Cell::new(core::ptr::null_mut()) };
        }

        #[cfg(not(target_os = "none"))]
        fn active() -> *mut $ty {
            ACTIVE.with(core::cell::Cell::get)
        }

        #[cfg(not(target_os = "none"))]
        fn set_active(value: *mut $ty) {
            ACTIVE.with(|cell| cell.set(value));
        }
    };
}
pub(crate) use active_pointer;
