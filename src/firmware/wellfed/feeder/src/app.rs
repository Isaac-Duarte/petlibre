//! The firmware's start hook and its loop task; the button service is the
//! other of its two tasks.

use core::ffi::c_void;

use wellfed_feeder::panel::BUTTONS;
use wellfed_sdk::Slot;
use wellfed_sdk::hal::{panel, sdk, settings, startup};

use crate::owner::{self, Inputs, Owner};

wellfed_sdk::firmware!(start);

fn start() {
    // Nothing here holds the KM0 flash handshake, so the runtime may confirm
    // once it is otherwise healthy.
    wellfed_sdk::startup_done(true);

    // SAFETY: the step ignores its context and keeps its state in statics.
    let created = unsafe { wellfed_sdk::spawn(c"wellfed-loop", LOOP_STACK_WORDS, 2, loop_task) };
    sdk::log(if created {
        c"PLPANEL1 task created=1\r\n"
    } else {
        c"PLPANEL1 task created=0\r\n"
    });
    wellfed_sdk::buttons::start(BUTTONS);
}

/// The step and one formatted record; the build's stack audit checks the
/// bound.
const LOOP_STACK_WORDS: u16 = 1024;

/// Touched only by the loop task; see `loop_task`.
static OWNER: Slot<Owner> = Slot::new();

struct Line(owner::Record);

impl core::fmt::Display for Line {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        owner::describe(self.0, f)
    }
}

unsafe extern "C" fn loop_task(_: *mut c_void) -> u32 {
    // SAFETY: only this task touches the slot.
    let owner = unsafe { OWNER.get_or_init(Owner::default) };

    let inputs = Inputs {
        now_ms: sdk::uptime_ms(),
        gestures: core::iter::from_fn(wellfed_sdk::buttons::next),
        remote: panel::take(),
        lights: settings::current().is_none_or(|view| view.lights),
    };
    let delay_ms = owner.step(inputs, |record| {
        let mut report = startup::Report::new();
        if let Ok(line) = report.record(format_args!("PLPANEL1 {}\r\n", Line(record))) {
            sdk::log(line);
        }
    });

    // Requests read what the owner holds after this step.
    if let Some(lock) = owner.lock() {
        panel::publish(lock.locked());
    }
    delay_ms
}
