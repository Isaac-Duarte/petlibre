//! The firmware's start hook and its one task.

use core::ffi::c_void;

use petlibre_probes::optics;
use petlibre_sdk::Slot;
use petlibre_sdk::hal::flash::park::Hardware;
use petlibre_sdk::hal::sdk;

petlibre_sdk::firmware!(start);

/// Above the diagnostic and transport tasks, so their logging cannot hold
/// off the owner lease the supervisor enforces.
fn start() {
    // SAFETY: the step ignores its context and keeps its state in statics.
    let created =
        unsafe { petlibre_sdk::spawn(c"petlibre-optics", PROBE_STACK_WORDS, 3, optics_task) };
    sdk::log(if created {
        c"PLOPT1 start created=1 plan=2\r\n"
    } else {
        c"PLOPT1 start created=0 plan=2\r\n"
    });
    const _: () = assert!(optics::PLAN == 2);
}

/// Building the probe in place copies it through two constructor frames
/// (about 3.6 KiB), so 4 KiB overflowed once LTO inlined differently.
const PROBE_STACK_WORDS: u16 = 2048;

/// Touched only by the optics task; see `optics_task`.
static OPTICS: Slot<optics::Probe> = Slot::new();

unsafe extern "C" fn optics_task(_: *mut c_void) -> u32 {
    // SAFETY: only this task touches the slot; the probe stays in place.
    let probe = unsafe { OPTICS.get_or_init(optics::Probe::default) };
    let step = probe.step::<Hardware>();
    if let Some(record) = step.record {
        petlibre_probes::log(format_args!("PLOPT1 {record}\r\n"));
    }
    // The probe holds the KM0 flash handshake until it ends.
    petlibre_sdk::startup_done(step.delay_ms == 0);
    step.delay_ms
}
