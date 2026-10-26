//! What the statistics share between the loop and the settings writer:
//! the event ring's fill and the lifetime totals, which the writer
//! publishes ([`publish_fill`], [`publish_totals`]) and the loop reads
//! ([`fill`], [`totals`]), and this boot's counts, which the loop hands
//! over ([`hand_counts`]) for the writer's next save of the totals
//! ([`counts`]).
//!
//! Every access copies a few hundred bytes at most with the scheduler
//! paused; both sides are tasks, never interrupt handlers.

use wellfed_feeder::feeder::platform::EventsFill;
use wellfed_feeder::stats::Totals;
use wellfed_hal::cell::TaskCell;
use wellfed_hal::sdk;
use wellfed_proto::api::Counts;

/// The lifetime statistics' slots within the storage region.
pub const STATS_BASE: u32 = wellfed_hal::plaf103::storage::STATS.first;

// The totals count erases of every sector the region has; growing the
// region changes the stats record and takes a new version of it.
const _: () = assert!(
    wellfed_hal::plaf103::STORAGE_SECTORS as usize == wellfed_feeder::stats::TOTAL_SECTORS
);

struct Shared {
    fill: EventsFill,
    totals: Option<Totals>,
    counts: Option<Counts>,
}

static SHARED: TaskCell<Shared> = TaskCell::new(Shared {
    fill: EventsFill {
        kept: 0,
        capacity: 0,
    },
    totals: None,
    counts: None,
});

fn with<R>(f: impl FnOnce(&mut Shared) -> R) -> R {
    sdk::unscheduled(|| {
        // SAFETY: with the scheduler paused no other task runs, and no
        // interrupt handler reaches this state.
        f(unsafe { &mut *SHARED.get() })
    })
}

/// The writer's half: the event ring's fill now.
pub fn publish_fill(fill: EventsFill) {
    with(|shared| shared.fill = fill);
}

/// The loop's half: the event ring's fill as last published.
pub fn fill() -> EventsFill {
    with(|shared| shared.fill)
}

/// The writer's half: the totals the stats store holds, this boot's reset
/// counted, or `None` when it keeps none.
pub fn publish_totals(totals: Option<Totals>) {
    with(|shared| shared.totals = totals);
}

/// The loop's half: the totals as last published.
pub fn totals() -> Option<Totals> {
    with(|shared| shared.totals)
}

/// The loop's half: this boot's counts so far.
pub fn hand_counts(counts: Counts) {
    with(|shared| shared.counts = Some(counts));
}

/// The writer's half: this boot's counts as the loop last handed them,
/// `None` before it has.
pub fn counts() -> Option<Counts> {
    with(|shared| shared.counts)
}

/// Host tests only: back to the boot state.
#[cfg(not(target_os = "none"))]
pub fn reset() {
    with(|shared| {
        *shared = Shared {
            fill: EventsFill::default(),
            totals: None,
            counts: None,
        };
    });
}
