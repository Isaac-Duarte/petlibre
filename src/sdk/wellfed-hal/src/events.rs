//! The event history, carried between the loop task and the settings
//! writer, which alone writes the ring in storage sectors 13-24
//! (`wellfed_core::store::events`).
//!
//! - **Appends:** the loop queues a record with [`record`] and goes on; the
//!   writer drains the queue with [`pending`] and [`finish_pending`]. A full
//!   queue drops the record and counts it ([`take_lost`]), so a burst can
//!   never stall the loop; the loop records the count as an event of its own.
//! - **Reads:** the device API asks for the record after a cursor with
//!   [`request_read`]; the writer answers with [`finish_read`], and the loop
//!   collects it with [`read_result`]. One read is outstanding at a time,
//!   and an answer stays until the next request, so several subscribers at
//!   the same cursor share it.
//!
//! Every access copies a few dozen bytes with the scheduler paused; both
//! sides are tasks, never interrupt handlers.

use wellfed_core::store::events::{BODY_BYTES, Read};

use crate::cell::TaskCell;
use crate::sdk;

/// Records the loop may queue before the writer takes them.
pub const QUEUE: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Queued {
    pub at: Option<u32>,
    pub body: [u8; BODY_BYTES],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReadSlot {
    Idle,
    Requested(Option<u64>),
    /// The answer for a cursor, valid while the ring's newest record is
    /// still `last`.
    Ready {
        after: Option<u64>,
        read: Read,
        last: Option<u64>,
    },
}

struct Shared {
    queue: [Option<Queued>; QUEUE],
    head: usize,
    len: usize,
    lost: u32,
    last: Option<u64>,
    read: ReadSlot,
}

static SHARED: TaskCell<Shared> = TaskCell::new(Shared {
    queue: [None; QUEUE],
    head: 0,
    len: 0,
    lost: 0,
    last: None,
    read: ReadSlot::Idle,
});

fn with<R>(f: impl FnOnce(&mut Shared) -> R) -> R {
    sdk::unscheduled(|| {
        // SAFETY: with the scheduler paused no other task runs, and no
        // interrupt handler reaches this state.
        f(unsafe { &mut *SHARED.get() })
    })
}

/// The loop's half: queue one record; false (and counted) when full.
pub fn record(at: Option<u32>, body: [u8; BODY_BYTES]) -> bool {
    with(|shared| {
        if shared.len == QUEUE {
            shared.lost = shared.lost.saturating_add(1);
            return false;
        }
        let index = (shared.head + shared.len) % QUEUE;
        shared.queue[index] = Some(Queued { at, body });
        shared.len += 1;
        true
    })
}

/// Records dropped since the last call.
pub fn take_lost() -> u32 {
    with(|shared| core::mem::take(&mut shared.lost))
}

/// The writer's half: the oldest queued record, if any; it stays queued
/// until [`finish_pending`].
pub fn pending() -> Option<Queued> {
    with(|shared| {
        (shared.len > 0)
            .then(|| shared.queue[shared.head])
            .flatten()
    })
}

/// The writer appended (or gave up on) the oldest queued record; `last` is
/// the ring's newest record now.
pub fn finish_pending(last: Option<u64>) {
    with(|shared| {
        if shared.len > 0 {
            shared.queue[shared.head] = None;
            shared.head = (shared.head + 1) % QUEUE;
            shared.len -= 1;
        }
        shared.last = last;
    });
}

/// The writer's half: the ring's newest record, after opening it.
pub fn publish_last(last: Option<u64>) {
    with(|shared| shared.last = last);
}

/// The ring's newest record, as the writer last published it.
pub fn last_seq() -> Option<u64> {
    with(|shared| shared.last)
}

/// The loop's half: the answer for `after`, if the writer has one that is
/// still current. Otherwise this asks for it (when no other read waits)
/// and returns `None`.
pub fn read_result(after: Option<u64>) -> Option<Read> {
    with(|shared| match shared.read {
        ReadSlot::Ready {
            after: answered,
            read,
            last,
        } if answered == after && (last == shared.last || read != Read::End) => Some(read),
        ReadSlot::Requested(_) => None,
        _ => {
            shared.read = ReadSlot::Requested(after);
            None
        }
    })
}

/// The writer's half: the cursor a read waits on.
pub fn read_request() -> Option<Option<u64>> {
    with(|shared| match shared.read {
        ReadSlot::Requested(after) => Some(after),
        _ => None,
    })
}

/// The writer's half: the answer to [`read_request`].
pub fn finish_read(after: Option<u64>, read: Read) {
    with(|shared| {
        shared.read = ReadSlot::Ready {
            after,
            read,
            last: shared.last,
        };
    });
}

/// Forget everything (host tests).
#[cfg(not(target_os = "none"))]
pub fn reset() {
    with(|shared| {
        shared.queue = [None; QUEUE];
        shared.head = 0;
        shared.len = 0;
        shared.lost = 0;
        shared.last = None;
        shared.read = ReadSlot::Idle;
    });
}
