//! The `adc` variant's start hook and its one task.

use wellfed_probes::adc;
use wellfed_sdk::hal::sdk;
use wellfed_sdk::{Slot, Startup};

#[wellfed_sdk::start(runtime = wellfed_feeder_runtime)]
fn start() -> Startup {
    // The probe keeps the ADC for as long as the build runs, so the
    // runtime's power monitor stays off it.
    wellfed_sdk::claim_adc();

    let created = wellfed_sdk::spawn(adc_task);
    // Only on failure: every PLADC1 line the audit sees must be a probe
    // record, so this one fails it.
    if !created {
        sdk::log(c"PLADC1 start created=0\r\n");
    }

    // The probe never binds the KM0 flash handshake, so confirmation need
    // not wait for it.
    Startup::Done
}

const ADC_STACK_WORDS: u16 = 2048;

/// Touched only by the ADC task; see `adc_task`.
static ADC: Slot<adc::Probe> = Slot::new();

#[wellfed_sdk::task(name = "wellfed-adc", stack = ADC_STACK_WORDS, priority = 3)]
fn adc_task() -> u32 {
    let Some(mut probe) = ADC.enter_or_init(Default::default) else {
        return 0;
    };

    let step = probe.step();
    if let Some(record) = step.record {
        adc::log(record);
    }
    step.delay_ms
}
