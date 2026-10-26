//! What the statistics read from the settings writer: the event ring's
//! fill, which the writer publishes as it opens and appends ([`publish_fill`])
//! and the loop reads for the flash section ([`fill`]).
//!
//! Every access copies a few bytes with the scheduler paused; both sides are
//! tasks, never interrupt handlers.

use wellfed_feeder::feeder::platform::EventsFill;
use wellfed_hal::cell::TaskCell;
use wellfed_hal::sdk;

static FILL: TaskCell<EventsFill> = TaskCell::new(EventsFill {
    kept: 0,
    capacity: 0,
});

fn with<R>(f: impl FnOnce(&mut EventsFill) -> R) -> R {
    sdk::unscheduled(|| {
        // SAFETY: with the scheduler paused no other task runs, and no
        // interrupt handler reaches this state.
        f(unsafe { &mut *FILL.get() })
    })
}

/// The writer's half: the event ring's fill now.
pub fn publish_fill(fill: EventsFill) {
    with(|shared| *shared = fill);
}

/// The loop's half: the event ring's fill as last published.
pub fn fill() -> EventsFill {
    with(|shared| *shared)
}

/// Host tests only: back to the boot state.
#[cfg(not(target_os = "none"))]
pub fn reset() {
    with(|shared| *shared = EventsFill::default());
}
