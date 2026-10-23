//! The firmware's start hook and its one task.

use core::ffi::c_void;

use wellfed_probes::ble::Setup;
use wellfed_sdk::Slot;
use wellfed_sdk::hal::ble::BleSetup;
use wellfed_sdk::hal::sdk;

wellfed_sdk::firmware!(start);

fn start() {
    // Nothing here holds the KM0 flash handshake, so the runtime may confirm
    // once it is otherwise healthy. Credentials go to the runtime's settings
    // writer, which saves them through the flash authority.
    wellfed_sdk::startup_done(true);

    // SAFETY: the step ignores its context and keeps its state in statics.
    let created = unsafe { wellfed_sdk::spawn(c"wellfed-ble", BLE_STACK_WORDS, 2, ble_task) };
    sdk::log(if created {
        c"PLBLE1 start created=1\r\n"
    } else {
        c"PLBLE1 start created=0\r\n"
    });
}

/// The step (checking a request's HMAC and writing its reply is the
/// deepest path) and one formatted record; the build's stack audit checks
/// the bound.
const BLE_STACK_WORDS: u16 = 1024;

/// Touched only by the BLE task; see `ble_task`.
static SETUP: Slot<BleSetup> = Slot::new();

/// Brings up the BLE peripheral, then serves setup requests while offline.
unsafe extern "C" fn ble_task(_: *mut c_void) -> u32 {
    // SAFETY: only this task touches the slot.
    let setup = unsafe { SETUP.get_or_init(BleSetup::default) };
    let step = setup.step();
    if let Some(record) = step.record {
        wellfed_probes::log(format_args!("PLBLE1 {}\r\n", Setup(record)));
    }
    step.delay_ms
}
