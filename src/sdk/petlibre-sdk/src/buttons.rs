//! FW20: the panel buttons for a firmware that wants them. The firmware
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
/// Filled by `start` before the task exists, then touched only by the
/// key task; see `key_task`.
static SERVICE: Slot<Service> = Slot::new();

struct Line(Record);

impl core::fmt::Display for Line {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        buttons::describe(self.0, f)
    }
}

/// Starts the button task once; later calls return false. Call from the
/// start hook. An invalid `config` is logged and the task ends at once.
pub fn start(config: Config) -> bool {
    if STARTED.swap(true, Ordering::AcqRel) {
        return false;
    }

    // SAFETY: the swap above lets only one caller here, and the task that
    // owns the slot does not exist yet; creating it orders this write
    // before the task's first read.
    unsafe { SERVICE.get_or_init(|| Service::new(config)) };
    // SAFETY: the step takes no context and keeps its state in the slot.
    let created = unsafe { sdk::task_create(c"petlibre-key", 512, 3, key_task, ptr::null_mut()) };
    sdk::log(if created {
        c"PLKEY1 start created=1\r\n"
    } else {
        c"PLKEY1 start created=0\r\n"
    });
    created
}

unsafe extern "C" fn key_task(_: *mut c_void) -> u32 {
    // SAFETY: only this task touches the slot once it runs.
    let Some(service) = (unsafe { SERVICE.get_or_try_init(|| None) }) else {
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
