//! The `optics` variant's start hook and its one task.

use wellfed_probes::optics;
use wellfed_sdk::hal::flash::park::Hardware;
use wellfed_sdk::hal::sdk;
use wellfed_sdk::{Slot, Startup, log};

#[wellfed_sdk::start(runtime = wellfed_feeder_runtime)]
fn start() -> Startup {
    let created = wellfed_sdk::spawn(optics_task);
    sdk::log(if created {
        c"PLOPT1 start created=1 plan=2\r\n"
    } else {
        c"PLOPT1 start created=0 plan=2\r\n"
    });
    const _: () = assert!(optics::PLAN == 2);

    // The probe holds the KM0 flash handshake; its task reports when it ends.
    Startup::Pending
}

/// Building the probe in place copies it through two constructor frames
/// (about 3.6 KiB), so 4 KiB overflowed once LTO inlined differently.
const OPTICS_STACK_WORDS: u16 = 2048;

/// Touched only by the optics task; see `optics_task`.
static OPTICS: Slot<optics::Probe> = Slot::new();

/// Above the diagnostic tasks, so their logging cannot hold
/// off the owner lease the supervisor enforces.
#[wellfed_sdk::task(name = "wellfed-optics", stack = OPTICS_STACK_WORDS, priority = 3)]
fn optics_task() -> u32 {
    let Some(mut probe) = OPTICS.enter_or_init(optics::Probe::default) else {
        return 0;
    };

    let step = probe.step::<Hardware>();
    if let Some(record) = step.record {
        log(format_args!("PLOPT1 {record}\r\n"));
    }
    // The probe holds the KM0 flash handshake until it ends.
    wellfed_sdk::startup_done(step.delay_ms == 0);
    step.delay_ms
}
