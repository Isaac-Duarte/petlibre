//! The KM0 archive and the firmware panic handler.
//!
//! Built with `km0`, this crate is the KM0 image's archive. Built with `km4`,
//! it supplies the panic handler every KM4 firmware links through
//! `petlibre-sdk`. The C ports call the HAL's `petlibre_*` exports, not this
//! crate's.
#![cfg_attr(target_os = "none", no_std)]

// The KM0 archive's C symbols (RAM park, SDK hooks, trial step) come from
// the common crate. A KM4 archive is the firmware crate, which links this
// one through petlibre-sdk for the panic handler.
#[cfg(feature = "km0")]
pub use petlibre_hal_common;

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    // KM4 owns the motor and infrared GPIO: mask interrupts and request motor
    // STOP and infrared OFF with raw register writes before halting. KM0
    // drives neither, so it only halts.
    #[cfg(feature = "km4")]
    petlibre_amebad_sys::panic_outputs_safe();

    // Halt here. Only a watchdog that was already armed resets the core (the
    // diagnostic build arms none); otherwise recovery needs an external reset.
    loop {
        core::hint::spin_loop();
    }
}
