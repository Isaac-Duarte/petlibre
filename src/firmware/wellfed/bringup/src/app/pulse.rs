//! The `pulse` variant's start hook and its one task.

use wellfed_probes::pulse;
use wellfed_sdk::hal::flash::park::Hardware;
use wellfed_sdk::hal::sdk;
use wellfed_sdk::{Slot, Startup, log};

#[wellfed_sdk::start(runtime = wellfed_feeder_runtime)]
fn start() -> Startup {
    let created = wellfed_sdk::spawn(pulse_task);
    sdk::log(if created {
        c"PLMOT1 start created=1\r\n"
    } else {
        c"PLMOT1 start created=0\r\n"
    });

    // The probe holds the KM0 flash handshake; its task reports when it ends.
    Startup::Pending
}

/// Building the probe in place copies it through two constructor frames
/// (about 3.6 KiB), so 4 KiB overflowed once LTO inlined differently.
const PULSE_STACK_WORDS: u16 = 2048;

/// Touched only by the pulse task; see `pulse_task`.
static PULSE: Slot<pulse::Probe> = Slot::new();

#[wellfed_sdk::task(name = "wellfed-pulse", stack = PULSE_STACK_WORDS, priority = 3)]
fn pulse_task() -> u32 {
    let Some(mut probe) = PULSE.enter_or_init(Default::default) else {
        return 0;
    };

    let step = probe.step::<Hardware>();
    if let Some(record) = step.record {
        log(format_args!("PLMOT1 {record}\r\n"));
    }
    // The probe holds the KM0 flash handshake until it ends.
    wellfed_sdk::startup_done(step.delay_ms == 0);
    step.delay_ms
}
