//! The event history, carried between the loop task and the settings
//! writer, which alone writes the ring in storage sectors 13-24
//! (`wellfed_feeder::store::events`).
//!
//! - **Appends:** the loop queues a record with [`record`] and goes on; the
//!   writer drains the queue with [`pending`] and [`finish_pending`]. A full
//!   queue drops the record and counts it ([`take_lost`]), so a burst can
//!   never stall the loop; the loop records the count as an event of its own.
//!   The last slot is kept for a feed's outcome ([`record_outcome`]): appends
//!   wait while a feed holds the outputs, so the events of a long feed pile
//!   up here, and the feed's own result must not be the one lost.
//! - **Reads:** the device API asks for the record after a cursor with
//!   [`read_result`]; the writer takes the request from [`read_request`]
//!   and answers with [`finish_read`], and the loop collects it with
//!   [`read_result`] again. One read is outstanding at a time. An answer is
//!   a batch of up to [`READ_BATCH`] records, so a subscriber catching up
//!   walks through all of it in one pass, into one frame of events, and
//!   several subscribers within it share it. A subscriber at the newest
//!   record is told so without a read, which leaves the slot to those still
//!   catching up.
//! - **Turns:** a batch is fetched for one subscriber, by its session
//!   slot, and stays until that subscriber has taken from it. Others whose
//!   cursor falls inside it read from it meanwhile; one it cannot answer
//!   waits its turn, kept by its slot with the cursor it last asked for.
//!   Once the batch's subscriber has taken from it, the next read goes to
//!   the next waiting slot after the last one served, round robin. So
//!   every subscriber catching up gets a batch within one round of the
//!   others, whatever order the loop visits them in. A subscriber that
//!   ends gives its turn and its batch up at once ([`reader_left`]); one
//!   that stops asking without ending (its last frame still going out)
//!   keeps its batch for at most [`MISSES_MAX`] asks it turns away.
//! - **Epoch:** the writer publishes the ring's epoch once it has opened
//!   it ([`publish_open`]); the device API welcomes no session before then,
//!   since a welcome names the epoch its event numbers count in.
//!
//! Every access copies a few dozen bytes with the scheduler paused; both
//! sides are tasks, never interrupt handlers.

use wellfed_device_api::{Reader, SESSIONS};
use wellfed_feeder::store::events::{BODY_BYTES, Batch, EMPTY, Read, Record};

use wellfed_hal::cell::TaskCell;
use wellfed_hal::sdk;

/// Records the loop may queue before the writer takes them.
pub const QUEUE: usize = 16;

/// Records one read answers at most. A frame holds at least 31 events (most
/// encode to 10-25 bytes), so one round trip through the writer, which polls
/// every few milliseconds, fills most of the frame a subscriber catching up
/// sends a pass.
pub const READ_BATCH: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Queued {
    pub at: Option<u32>,
    pub body: [u8; BODY_BYTES],
}

/// A queue slot not yet filled. All zeros, as every initial field of
/// [`SHARED`] is, so the state goes to `.bss` rather than the image.
const UNQUEUED: Queued = Queued {
    at: None,
    body: [0; BODY_BYTES],
};

/// The owner of a read whose subscriber ended: nobody collects it.
const NOBODY: usize = SESSIONS;

// `repr(u8)` puts `Idle` at tag 0, so the initial state is all zeros.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum ReadSlot {
    Idle,
    /// A read for the subscriber in slot `reader`.
    Requested {
        reader: usize,
        after: Option<u64>,
    },
    /// The answer for slot `reader`'s cursor. Records stay valid; `End`
    /// only while the ring's newest record is still `last`.
    Ready {
        reader: usize,
        after: Option<u64>,
        batch: Batch,
        last: Option<u64>,
        /// Whether `reader` has taken anything from it.
        taken: bool,
        /// Asks from other slots it could not answer before then.
        misses: u8,
    },
}

/// A slot's place in the turns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Turn {
    /// Not waiting: answered last time, at the newest record, or no
    /// subscriber.
    Idle,
    /// Turned away while asking for the record after this cursor.
    Waiting(Option<u64>),
}

/// Asks from other slots a batch turns away before it gives way untaken:
/// its subscriber stopped asking without ending (its last frame is still
/// going out). Each waiting slot asks at most once a loop pass, so this is
/// 50 passes, about a second at a 20 ms pass, with one other subscriber
/// waiting, and about 17 with three. A subscriber whose batch gave way
/// asks again in its turn.
pub const MISSES_MAX: u8 = 50;

struct Shared {
    /// Records from `head`, `len` of them.
    queue: [Queued; QUEUE],
    head: usize,
    len: usize,
    lost: u32,
    last: Option<u64>,
    epoch: Option<u32>,
    read: ReadSlot,
    /// Each session slot's turn.
    turns: [Turn; SESSIONS],
    /// The slot the next read looks for a waiting subscriber from.
    turn: usize,
    /// The records of a `Ready` batch, at the front.
    records: [Record; READ_BATCH],
}

static SHARED: TaskCell<Shared> = TaskCell::new(Shared {
    queue: [UNQUEUED; QUEUE],
    head: 0,
    len: 0,
    lost: 0,
    last: None,
    epoch: None,
    read: ReadSlot::Idle,
    turns: [Turn::Idle; SESSIONS],
    turn: 0,
    records: [EMPTY; READ_BATCH],
});

fn with<R>(f: impl FnOnce(&mut Shared) -> R) -> R {
    sdk::unscheduled(|| {
        // SAFETY: with the scheduler paused no other task runs, and no
        // interrupt handler reaches this state.
        f(unsafe { &mut *SHARED.get() })
    })
}

/// Slots only [`record_outcome`] may fill.
pub const RESERVED: usize = 1;

/// The loop's half: queue one record; false (and counted) when only the
/// reserved slots are left.
pub fn record(at: Option<u32>, body: [u8; BODY_BYTES]) -> bool {
    push(at, body, QUEUE - RESERVED)
}

/// As [`record`] for a feed's outcome, which may take a reserved slot. One
/// feed runs at a time and its outcome is recorded once, before the writes
/// it held off resume, so one slot is enough.
pub fn record_outcome(at: Option<u32>, body: [u8; BODY_BYTES]) -> bool {
    push(at, body, QUEUE)
}

fn push(at: Option<u32>, body: [u8; BODY_BYTES], limit: usize) -> bool {
    with(|shared| {
        if shared.len >= limit {
            shared.lost = shared.lost.saturating_add(1);
            return false;
        }
        let index = (shared.head + shared.len) % QUEUE;
        shared.queue[index] = Queued { at, body };
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
    with(|shared| (shared.len > 0).then(|| shared.queue[shared.head]))
}

/// The writer appended (or gave up on) the oldest queued record; `last` is
/// the ring's newest record now.
pub fn finish_pending(last: Option<u64>) {
    with(|shared| {
        if shared.len > 0 {
            shared.queue[shared.head] = UNQUEUED;
            shared.head = (shared.head + 1) % QUEUE;
            shared.len -= 1;
        }
        shared.last = last;
    });
}

/// The writer's half: the ring's epoch and newest record, after opening it.
pub fn publish_open(epoch: u32, last: Option<u64>) {
    with(|shared| {
        shared.epoch = Some(epoch);
        shared.last = last;
    });
}

/// The ring's epoch, once the writer has opened it.
pub fn epoch() -> Option<u32> {
    with(|shared| shared.epoch)
}

/// The ring's newest record, as the writer last published it.
pub fn last_seq() -> Option<u64> {
    with(|shared| shared.last)
}

/// The loop's half: the answer for `after`, for the subscriber in slot
/// `reader`, if the writer has one that is still current. Otherwise this
/// waits the subscriber's turn, asks for the next read if the slot is
/// free, and returns `None`.
pub fn read_result(reader: Reader, after: Option<u64>) -> Option<Read> {
    with(|shared| {
        let me = reader.index();

        // A subscriber at the newest record needs no read. Asking would
        // replace the answer another subscriber waits on, and one caught
        // up asks on every pass, so the other would never collect it.
        if after.is_some() && after == shared.last {
            set_turn(shared, me, Turn::Idle);
            return Some(Read::End);
        }

        match &mut shared.read {
            ReadSlot::Ready {
                reader: owner,
                after: answered,
                batch,
                last,
                taken,
                misses,
            } => {
                if let Some(found) = answer(
                    *answered,
                    *batch,
                    *last,
                    &shared.records,
                    shared.last,
                    after,
                ) {
                    *taken |= *owner == me;
                    set_turn(shared, me, Turn::Idle);
                    return Some(found);
                }
                // Its own subscriber has moved past it, and an end the
                // ring has since moved past answers nobody.
                let stale = *batch == Batch::End && *last != shared.last;
                let held = !*taken && !stale && *owner != me && *owner != NOBODY;
                if held && *misses < MISSES_MAX {
                    *misses += 1;
                    set_turn(shared, me, Turn::Waiting(after));
                    return None;
                }
            }
            ReadSlot::Requested { reader: owner, .. } => {
                if *owner != me {
                    set_turn(shared, me, Turn::Waiting(after));
                }
                return None;
            }
            ReadSlot::Idle => {}
        }

        // The slot is free: the next waiting subscriber's read, this one
        // waiting among them.
        set_turn(shared, me, Turn::Waiting(after));
        let next = (0..SESSIONS)
            .map(|offset| (shared.turn + offset) % SESSIONS)
            .find_map(|slot| match shared.turns[slot] {
                Turn::Waiting(cursor) => Some((slot, cursor)),
                Turn::Idle => None,
            });
        // A reader outside the slots has no turn: it is served at once.
        let (slot, cursor) = next.unwrap_or((NOBODY, after));
        set_turn(shared, slot, Turn::Idle);
        shared.turn = (slot + 1) % SESSIONS;
        shared.read = ReadSlot::Requested {
            reader: slot,
            after: cursor,
        };
        None
    })
}

/// The loop's half: the subscriber in slot `reader` ended. Its turn is
/// given up, and a read fetched for it is nobody's, so the next ask may
/// replace it.
pub fn reader_left(reader: Reader) {
    with(|shared| {
        let me = reader.index();
        set_turn(shared, me, Turn::Idle);
        match &mut shared.read {
            ReadSlot::Requested { reader: owner, .. } | ReadSlot::Ready { reader: owner, .. }
                if *owner == me =>
            {
                *owner = NOBODY;
            }
            _ => {}
        }
    });
}

fn set_turn(shared: &mut Shared, slot: usize, turn: Turn) {
    if let Some(held) = shared.turns.get_mut(slot) {
        *held = turn;
    }
}

/// What a `Ready` batch answers for `after`: the asked cursor's answer, or
/// the record after any of its records but the last (which needs the next
/// batch); `None` when it holds no answer for it.
fn answer(
    answered: Option<u64>,
    batch: Batch,
    last: Option<u64>,
    records: &[Record],
    newest: Option<u64>,
    after: Option<u64>,
) -> Option<Read> {
    match batch {
        Batch::Records(len) => {
            let records = records.get(..len)?;
            if answered == after {
                return records.first().copied().map(Read::Record);
            }
            let held = records.iter().position(|r| Some(r.seq) == after)?;
            records.get(held + 1).copied().map(Read::Record)
        }
        Batch::Gap { from, to } => (answered == after).then_some(Read::Gap { from, to }),
        Batch::End => (answered == after && last == newest).then_some(Read::End),
    }
}

/// The writer's half: the cursor a read waits on.
pub fn read_request() -> Option<Option<u64>> {
    with(|shared| match shared.read {
        ReadSlot::Requested { after, .. } => Some(after),
        _ => None,
    })
}

/// The writer's half: the answer to [`read_request`], as
/// [`EventLog::read_batch`](wellfed_feeder::store::events::EventLog::read_batch)
/// gave it, with its records at the front of `records`.
pub fn finish_read(after: Option<u64>, batch: Batch, records: &[Record]) {
    with(|shared| {
        let batch = match batch {
            Batch::Records(len) => {
                let len = len.min(records.len()).min(READ_BATCH);
                shared.records[..len].copy_from_slice(&records[..len]);
                Batch::Records(len)
            }
            other => other,
        };
        let reader = match shared.read {
            ReadSlot::Requested { reader, .. } => reader,
            _ => NOBODY,
        };
        shared.read = ReadSlot::Ready {
            reader,
            after,
            batch,
            last: shared.last,
            taken: false,
            misses: 0,
        };
    });
}

/// Forget everything (host tests).
#[cfg(not(target_os = "none"))]
pub fn reset() {
    with(|shared| {
        shared.queue = [UNQUEUED; QUEUE];
        shared.head = 0;
        shared.len = 0;
        shared.lost = 0;
        shared.last = None;
        shared.epoch = None;
        shared.read = ReadSlot::Idle;
        shared.turns = [Turn::Idle; SESSIONS];
        shared.turn = 0;
    });
}
