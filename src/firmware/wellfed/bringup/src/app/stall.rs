//! The `stall` trial: no task, and the watchdog left unfed.

use wellfed_sdk::hal::sdk;

/// Stop feeding the watchdog, as a hung candidate would, so KM0 counts the
/// reset. Outputs stay stopped.
pub(super) fn start() {
    wellfed_sdk::request_reset();
    sdk::log(if wellfed_sdk::watchdog_open() {
        c"PLTRL1 test=reset watchdog=1\r\n"
    } else {
        c"PLTRL1 test=reset watchdog=0\r\n"
    });
}
