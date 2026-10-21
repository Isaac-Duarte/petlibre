//! C ABI exports with the original `petlibro_*` names and signatures.
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
pub use petlibro_amebad;

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    // Stop here; the independent hardware watchdog resets the core.
    loop {
        core::hint::spin_loop();
    }
}
