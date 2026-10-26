//! The owners' update keys, as the store that holds them last published
//! them: the update stage admits an offer signed by one of them besides the
//! build's own key.
//!
//! The product's settings writer keeps the keys (with the rest of its
//! clients) and publishes them after it loads its store and after every
//! change; the update stage copies them for each offer. Written and read
//! with the scheduler paused, so a reader never sees half a table.

use crate::cell::TaskCell;
use crate::sdk;

/// The most update keys the stage holds; a store with more publishes the
/// first this many.
pub const SIGNERS: usize = 8;

struct Table {
    keys: [[u8; 32]; SIGNERS],
    count: usize,
}

static TABLE: TaskCell<Table> = TaskCell::new(Table {
    keys: [[0; 32]; SIGNERS],
    count: 0,
});

/// The writer's half: the update keys the store now holds.
pub fn publish(keys: impl Iterator<Item = [u8; 32]>) {
    let mut table = Table {
        keys: [[0; 32]; SIGNERS],
        count: 0,
    };
    for (slot, key) in table.keys.iter_mut().zip(keys) {
        *slot = key;
        table.count += 1;
    }
    sdk::unscheduled(|| {
        // SAFETY: with the scheduler paused no other task runs, and no
        // interrupt handler reaches the table.
        unsafe { *TABLE.get() = table };
    });
}

/// The update stage's half: the keys as last published, and how many.
pub fn read() -> ([[u8; 32]; SIGNERS], usize) {
    sdk::unscheduled(|| {
        // SAFETY: as in `publish`.
        let table = unsafe { &*TABLE.get() };
        (table.keys, table.count)
    })
}

/// Forget every key (host tests).
#[cfg(not(target_os = "none"))]
pub fn reset() {
    publish(core::iter::empty());
}
