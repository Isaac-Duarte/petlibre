//! The firmware's start hook and its one task.

use wellfed_sdk::hal::sdk;
use wellfed_sdk::{LogRecord as _, Slot, Startup};

use crate::blink::Blinker;

#[wellfed_sdk::start(runtime = wellfed_feeder_runtime)]
fn start() -> Startup {
    let created = wellfed_sdk::spawn(blink_task);
    sdk::log(if created {
        c"PLBLK1 start created=1\r\n"
    } else {
        c"PLBLK1 start created=0\r\n"
    });

    // Nothing here holds the KM0 flash handshake, so the runtime may confirm
    // once it is otherwise healthy.
    Startup::Done
}

/// The step and one formatted record; the build's stack audit checks the
/// bound.
const BLINK_STACK_WORDS: u16 = 1024;

/// Touched only by the blink task.
static BLINKER: Slot<Blinker> = Slot::new();

#[wellfed_sdk::task(name = "wellfed-blink", stack = BLINK_STACK_WORDS, priority = 2)]
fn blink_task() -> u32 {
    let Some(mut blinker) = BLINKER.enter_or_init(Blinker::default) else {
        return 0;
    };

    let step = blinker.step();
    if let Some(record) = step.record {
        record.log();
    }
    step.delay_ms
}
