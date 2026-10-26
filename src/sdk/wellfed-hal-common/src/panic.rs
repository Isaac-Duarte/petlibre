//! The panic handler every firmware archive links: the KM0 archive is this
//! crate, and every KM4 firmware reaches it through `wellfed-hal`.

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    // KM4 owns the motor and infrared GPIO: mask interrupts and request motor
    // STOP and infrared OFF with raw register writes before halting, then
    // say where. KM0 drives neither and has no print here, so it only halts.
    #[cfg(feature = "km4")]
    {
        wellfed_amebad_sys::panic_outputs_safe();
        if let Some(at) = info.location() {
            wellfed_amebad_sys::panic_record(at.file(), at.line(), at.column());
        }
    }
    #[cfg(not(feature = "km4"))]
    let _ = info;

    // Halt here. Only a watchdog that was already armed resets the core (the
    // diagnostic build arms none); otherwise recovery needs an external reset.
    loop {
        core::hint::spin_loop();
    }
}
