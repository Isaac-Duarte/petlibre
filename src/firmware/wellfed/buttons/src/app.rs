//! The firmware's start hook and its one task, which logs the raw lines and
//! reads the button service's gestures, as a firmware's owner task would.

use core::ffi::c_void;

use wellfed_probes::buttons;
use wellfed_sdk::Slot;
use wellfed_sdk::buttons::Config;
use wellfed_sdk::hal::sdk;

wellfed_sdk::firmware!(start);

fn start() {
    // Neither the probe nor the service binds the KM0 flash handshake, so the
    // runtime may confirm once it is otherwise healthy.
    wellfed_sdk::startup_done(true);

    // SAFETY: the step ignores its context and keeps its state in statics.
    let created = unsafe { wellfed_sdk::spawn(c"wellfed-btn", 1024, 3, button_task) };
    sdk::log(if created {
        c"PLBTN1 start created=1\r\n"
    } else {
        c"PLBTN1 start created=0\r\n"
    });
    wellfed_sdk::buttons::start(Config::BENCH);
}

/// Touched only by the button task; see `button_task`.
static BUTTONS: Slot<buttons::Probe> = Slot::new();

unsafe extern "C" fn button_task(_: *mut c_void) -> u32 {
    // SAFETY: only this task touches the slot.
    let probe = unsafe { BUTTONS.get_or_init(Default::default) };
    let step = probe.step();
    if let Some(record) = step.record {
        wellfed_probes::log(format_args!("PLBTN1 {record}\r\n"));
    }

    // The service logs each gesture as it makes it; this line shows the
    // reader got the same gesture through the queue.
    while let Some(gesture) = wellfed_sdk::buttons::next() {
        wellfed_probes::log(format_args!("PLKEY1 read {gesture}\r\n"));
    }
    step.delay_ms
}
