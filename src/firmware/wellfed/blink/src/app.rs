//! The firmware's start hook and its one task.

use core::ffi::c_void;

use wellfed_sdk::Slot;
use wellfed_sdk::hal::{sdk, startup};

use crate::blink::{self, Blinker};

wellfed_sdk::firmware!(start);

fn start() {
    // Nothing here holds the KM0 flash handshake, so the runtime may confirm
    // once it is otherwise healthy.
    wellfed_sdk::startup_done(true);

    // SAFETY: the step ignores its context and keeps its state in statics.
    let created =
        unsafe { wellfed_sdk::spawn(c"wellfed-blink", BLINK_STACK_WORDS, 2, blink_task) };
    sdk::log(if created {
        c"PLBLK1 start created=1\r\n"
    } else {
        c"PLBLK1 start created=0\r\n"
    });
}

/// The step and one formatted record; the build's stack audit checks the
/// bound.
const BLINK_STACK_WORDS: u16 = 1024;

/// Touched only by the blink task; see `blink_task`.
static BLINKER: Slot<Blinker> = Slot::new();

struct Line(blink::Record);

impl core::fmt::Display for Line {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        blink::describe(self.0, f)
    }
}

unsafe extern "C" fn blink_task(_: *mut c_void) -> u32 {
    let Some(mut blinker) = BLINKER.enter_or_init(Blinker::default) else {
        return 0;
    };
    let step = blinker.step();
    if let Some(record) = step.record {
        let mut report = startup::Report::new();
        if let Ok(line) = report.record(format_args!("PLBLK1 {}\r\n", Line(record))) {
            sdk::log(line);
        }
    }
    step.delay_ms
}
