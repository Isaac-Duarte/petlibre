//! The firmware's start hook.

wellfed_sdk::firmware!(start);

/// Stop feeding the watchdog, as a hung candidate would, so KM0 counts the
/// reset. Outputs stay stopped.
#[cfg(feature = "fw11-reset")]
fn start() {
    use wellfed_sdk::hal::sdk;

    wellfed_sdk::request_reset();
    sdk::log(if wellfed_sdk::watchdog_open() {
        c"PLTRL1 test=reset watchdog=1\r\n"
    } else {
        c"PLTRL1 test=reset watchdog=0\r\n"
    });
}

/// The normal image and `fw11-fail` (whose failure is the runtime's): no
/// task, so nothing holds the KM0 flash handshake.
#[cfg(not(feature = "fw11-reset"))]
fn start() {
    wellfed_sdk::startup_done(true);
}
