//! The panel buttons for a firmware that wants them. The firmware
//! starts the service from its start hook with its hold thresholds, then
//! reads gestures from the one task that owns what the buttons do:
//!
//! ```ignore
//! fn start() {
//!     petlibre_sdk::buttons::start(Config::BENCH);
//! }
//! // In the owning task's step:
//! while let Some(gesture) = petlibre_sdk::buttons::next() { … }
//! ```
//!
//! A firmware that never starts it links none of it, so the images that
//! don't use buttons keep their audited layout.

use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

use petlibre_hal::buttons::{self, Record, Service};
use petlibre_hal::{sdk, startup};

use crate::slot::Slot;

pub use petlibre_core::buttons::{Button, Buttons, Config, Gesture};
pub use petlibre_hal::buttons::next;

static STARTED: AtomicBool = AtomicBool::new(false);
/// Filled by `start` in the start hook's task, then handed to the key task.
static SERVICE: Slot<Service> = Slot::new();

struct Line(Record);

impl core::fmt::Display for Line {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        buttons::describe(self.0, f)
    }
}

/// Starts the button task once; later calls return false. Call from the
/// start hook. The task is one of the firmware's
/// [`FIRMWARE_TASKS`](crate::FIRMWARE_TASKS). An invalid `config` is logged
/// and the task ends at once.
pub fn start(config: Config) -> bool {
    if STARTED.swap(true, Ordering::AcqRel) {
        return false;
    }
    if !crate::tasks::reserve() {
        sdk::log(c"PLKEY1 start created=0 firmware_tasks=full\r\n");
        return false;
    }

    // Built here, before the task exists, then handed over: the key task's
    // first entry claims it.
    drop(SERVICE.enter_or_init(|| Service::new(config)));
    SERVICE.release();
    // SAFETY: the step takes no context and keeps its state in the slot.
    let created = unsafe { sdk::task_create(c"petlibre-key", 512, 3, key_task, ptr::null_mut()) };
    if !created {
        crate::tasks::release();
    }
    sdk::log(if created {
        c"PLKEY1 start created=1\r\n"
    } else {
        c"PLKEY1 start created=0\r\n"
    });
    created
}

unsafe extern "C" fn key_task(_: *mut c_void) -> u32 {
    let Some(mut service) = SERVICE.enter() else {
        return 0;
    };

    let step = service.step();
    if let Some(record) = step.record {
        let mut report = startup::Report::new();
        if let Ok(line) = report.record(format_args!("PLKEY1 {}\r\n", Line(record))) {
            sdk::log(line);
        }
    }
    step.delay_ms
}
