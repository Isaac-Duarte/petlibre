//! The firmware's start hook and its one task.

use core::ffi::c_void;

use petlibre_probes::buttons;
use petlibre_sdk::Slot;
use petlibre_sdk::hal::sdk;

petlibre_sdk::firmware!(start);

fn start() {
    // The probe never binds the KM0 flash handshake, so the runtime may
    // confirm once it is otherwise healthy.
    petlibre_sdk::startup_done(true);

    // SAFETY: the step ignores its context and keeps its state in statics.
    let created = unsafe { petlibre_sdk::spawn(c"petlibre-btn", 1024, 3, button_task) };
    sdk::log(if created {
        c"PLBTN1 start created=1\r\n"
    } else {
        c"PLBTN1 start created=0\r\n"
    });
}

/// Touched only by the button task; see `button_task`.
static BUTTONS: Slot<buttons::Probe> = Slot::new();

unsafe extern "C" fn button_task(_: *mut c_void) -> u32 {
    // SAFETY: only this task touches the slot.
    let probe = unsafe { BUTTONS.get_or_init(Default::default) };
    let step = probe.step();
    if let Some(record) = step.record {
        petlibre_probes::log(format_args!("PLBTN1 {record}\r\n"));
    }
    step.delay_ms
}
