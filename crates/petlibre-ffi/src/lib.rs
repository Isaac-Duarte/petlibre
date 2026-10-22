//! C ABI exports with `petlibre_*` names and the original signatures.
//!
//! Each function accepts the nullable pointers the C headers allowed and
//! reproduces the original null handling. Callers keep the C contracts:
//! pointers are either null or valid, aligned and exclusively accessible for
//! the call, and callback/context pairs stay valid while installed.
// Host builds keep std so the library links into host C programs.
#![cfg_attr(target_os = "none", no_std)]
#![allow(clippy::missing_safety_doc)]

pub mod core_abi;

// The ports export their own C symbols (app entry, RAM park and SDK hooks).
#[cfg(any(feature = "km0", feature = "km4"))]
pub use petlibre_amebad;

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    // KM4 owns the motor and infrared GPIO: mask interrupts and request motor
    // STOP and infrared OFF with raw register writes before halting. KM0
    // drives neither, so it only halts.
    #[cfg(feature = "km4")]
    petlibre_amebad::sdk::panic_outputs_safe();
    // Halt here. Only a watchdog that was already armed resets the core (the
    // diagnostic build arms none); otherwise recovery needs an external reset.
    loop {
        core::hint::spin_loop();
    }
}
