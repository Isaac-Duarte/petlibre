//! The SDK surface used by the PLAF103 AmebaD ports.
//!
//! Target builds call the pinned AmebaD SDK through bindgen-generated
//! declarations; host builds use [`fake`], a deterministic model of the same
//! calls for tests and the virtual feeder. Both expose identical functions.
#![cfg_attr(target_os = "none", no_std)]
#![allow(clippy::missing_safety_doc)]

use core::ffi::c_void;

/// Interrupt service routine registered with the SDK vector table.
pub type Isr = unsafe extern "C" fn(context: *mut c_void) -> u32;

/// One task iteration: returns the delay in milliseconds before the next
/// iteration, or zero to delete the task.
pub type TaskStep = unsafe extern "C" fn(context: *mut c_void) -> u32;

/// Pinned GPIO enable bits, checked against SDK constants in target builds.
pub const GPIO_ENABLE_BIT: u32 = 1 << 20;

#[derive(Clone, Copy, Debug)]
pub struct GpioClock {
    pub function_before: u32,
    pub clock_before: u32,
    pub function_after: u32,
    pub clock_after: u32,
}

impl GpioClock {
    pub fn verified(self) -> bool {
        self.function_after & GPIO_ENABLE_BIT != 0
            && self.clock_after & GPIO_ENABLE_BIT != 0
            && (self.function_after & !GPIO_ENABLE_BIT) == (self.function_before & !GPIO_ENABLE_BIT)
            && (self.clock_after & !GPIO_ENABLE_BIT) == (self.clock_before & !GPIO_ENABLE_BIT)
    }
}

/// Read-only shared-pad enables and GPIO data-source selectors.
/// Values are observations; no mapping from group bits to physical pins is assumed.
#[derive(Clone, Copy, Debug, Default)]
pub struct GpioRouting {
    pub groups: u32,
    pub aon: u32,
    pub control_a: u32,
    pub control_b: u32,
    /// KM4's view of the KM0-owned shared-pad supply control.
    pub share_pad: u32,
    /// Raw Port A input word, including non-indicator pads.
    pub ext_a: u32,
}

/// One flash MMU entry's control, start and offset registers.
#[derive(Clone, Copy, Debug, Default)]
pub struct FlashMmuEntry {
    pub control: u32,
    pub start: u32,
    pub offset: u32,
}

/// Interrupt lines claimed by the ports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Irq {
    Timer2,
    GpioA,
}

#[cfg(all(target_os = "none", feature = "km4"))]
mod target;
#[cfg(all(target_os = "none", feature = "km4"))]
pub use target::*;

#[cfg(not(target_os = "none"))]
pub mod fake;
#[cfg(not(target_os = "none"))]
pub use fake::api::*;

#[cfg(any(not(target_os = "none"), feature = "km4"))]
mod guard {
    /// Interrupt mask held for the guard's lifetime, restoring the entry mask.
    pub struct IrqGuard(u32);

    impl IrqGuard {
        #[must_use]
        pub fn lock() -> Self {
            let saved = super::primask();
            super::disable_irq();
            Self(saved)
        }
    }

    impl Drop for IrqGuard {
        fn drop(&mut self) {
            super::set_primask(self.0);
        }
    }
}
#[cfg(any(not(target_os = "none"), feature = "km4"))]
pub use guard::IrqGuard;
