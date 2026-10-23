//! The firmware's start hook and its one task.

use core::ffi::c_void;

use wellfed_probes::adc;
use wellfed_sdk::Slot;
use wellfed_sdk::hal::sdk;

wellfed_sdk::firmware!(start);

fn start() {
    // The probe keeps the ADC for as long as the build runs, so the
    // runtime's power monitor stays off it.
    wellfed_sdk::claim_adc();
    // The probe never binds the KM0 flash handshake, so confirmation need
    // not wait for it.
    wellfed_sdk::startup_done(true);

    // SAFETY: the step ignores its context and keeps its state in statics.
    let created = unsafe { wellfed_sdk::spawn(c"wellfed-adc", PROBE_STACK_WORDS, 3, adc_task) };
    // Only on failure: every PLADC1 line the audit sees must be a probe
    // record, so this one fails it.
    if !created {
        sdk::log(c"PLADC1 start created=0\r\n");
    }
}

const PROBE_STACK_WORDS: u16 = 2048;

/// Touched only by the ADC task; see `adc_task`.
static ADC: Slot<adc::Probe> = Slot::new();

unsafe extern "C" fn adc_task(_: *mut c_void) -> u32 {
    // SAFETY: only this task touches the slot; the probe stays in place.
    let probe = unsafe { ADC.get_or_init(Default::default) };
    let step = probe.step();
    if let Some(record) = step.record {
        adc::log(record);
    }
    step.delay_ms
}
