//! The firmware's start hook.

petlibre_sdk::firmware!(start);

/// Stop feeding the watchdog, as a hung candidate would, so KM0 counts the
/// reset. Outputs stay stopped.
#[cfg(feature = "stall")]
fn start() {
    use petlibre_sdk::hal::sdk;

    petlibre_sdk::request_reset();
    sdk::log(if petlibre_sdk::watchdog_open() {
        c"PLTRL1 test=reset watchdog=1\r\n"
    } else {
        c"PLTRL1 test=reset watchdog=0\r\n"
    });
}

/// The normal image and `fail` (whose failure is the runtime's): no
/// task, so nothing holds the KM0 flash handshake.
#[cfg(not(feature = "stall"))]
fn start() {
    petlibre_sdk::startup_done(true);
}
