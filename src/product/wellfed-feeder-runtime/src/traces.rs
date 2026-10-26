//! The latest feed's trace in RAM, carried between the loop, which records
//! it from the feeding owner's observations, and the settings writer, which
//! copies it into the storage ring (sectors 29-31,
//! `wellfed_feeder::store::traces`) once it has the write turn.
//!
//! One trace, under a kilobyte, passes through these hands:
//! - **Recording:** the loop's alone, from the feed's submission
//!   ([`begin`]) through each poll ([`observe`]) to its result ([`seal`]).
//!   Nothing is written to flash meanwhile.
//! - **Sealed:** read-only from then on. The writer claims it for its
//!   append ([`write_sealed`]), after the feed, when the outputs are
//!   stopped and it has the write turn; the loop keeps answering reads from
//!   it ([`page`]) during the write and after.
//! - The next feed's [`begin`] takes it back. A sealed trace the writer
//!   never reached is lost then, and counted (`unsaved`): the next feed's
//!   claim waits for a queued trace like any queued write, so that takes a
//!   write that failed or could not start. A feed that begins while the
//!   writer is still appending is not traced at all, rather than wait.
//!
//! The hand-offs are one atomic state, so neither side waits for the other
//! or pauses the scheduler; both are tasks, never interrupt handlers.

use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use wellfed_core::feed::observe::Observation;
use wellfed_feeder::feeder::platform::{TraceRead, TracesFill};
use wellfed_feeder::trace::{DriveFigures, Recorder};
use wellfed_hal::cell::TaskCell;
use wellfed_proto::api::{EventBody, Refusal, TraceSummary};

/// Who holds `TRACE`, as `STATE` stores it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Hand {
    Empty = 0,
    /// The loop's, mutable.
    Recording = 1,
    /// Read-only, waiting for the writer.
    Sealed = 2,
    /// Read-only, the writer appending it.
    Writing = 3,
    /// Read-only, in the storage ring too.
    Stored = 4,
    /// Read-only, its append failed or could not start.
    Unsaved = 5,
}

impl Hand {
    const fn from_byte(byte: u8) -> Self {
        match byte {
            1 => Self::Recording,
            2 => Self::Sealed,
            3 => Self::Writing,
            4 => Self::Stored,
            5 => Self::Unsaved,
            _ => Self::Empty,
        }
    }
}

/// The trace; mutated only by the loop while `STATE` is `Recording`, read by
/// the loop and the writer otherwise.
static TRACE: TaskCell<Recorder> = TaskCell::new(Recorder::new());
static STATE: AtomicU8 = AtomicU8::new(Hand::Empty as u8);
static UNSAVED: AtomicU32 = AtomicU32::new(0);
/// The ring's fill, as the writer last published it.
static KEPT: AtomicU32 = AtomicU32::new(0);
static CAPACITY: AtomicU32 = AtomicU32::new(0);

fn state() -> Hand {
    Hand::from_byte(STATE.load(Ordering::Acquire))
}

fn hand_over(from: Hand, to: Hand) -> bool {
    STATE
        .compare_exchange(from as u8, to as u8, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

/// The loop's half: start recording feed `feed`; false when the writer is
/// still appending the last trace, and this feed goes untraced.
pub fn begin(feed: u32) -> bool {
    loop {
        let held = state();
        if held == Hand::Writing {
            return false;
        }
        if hand_over(held, Hand::Recording) {
            // A sealed trace the writer has not reached is lost here. One
            // still recording never got a result (its feed ended before
            // its reservation), so there was nothing to save.
            if held == Hand::Sealed {
                UNSAVED.fetch_add(1, Ordering::Relaxed);
            }
            // SAFETY: `Recording` makes the loop the trace's only user.
            unsafe { (*TRACE.get()).begin(feed) };
            return true;
        }
    }
}

/// The loop's half: one poll of the feed being recorded.
pub fn observe(seen: &Observation) {
    if state() == Hand::Recording {
        // SAFETY: as in `begin`.
        unsafe { (*TRACE.get()).observe(seen) };
    }
}

/// The loop's half: seal the trace with the feed's `Fed` event and the
/// owner's figures, for the writer; its summary, `None` when no trace of
/// that feed was recorded.
pub fn seal(fed: &EventBody, drive: &DriveFigures) -> Option<TraceSummary> {
    if state() != Hand::Recording {
        return None;
    }
    // SAFETY: as in `begin`.
    let summary = unsafe { (*TRACE.get()).seal(fed, drive) }?;
    STATE.store(Hand::Sealed as u8, Ordering::Release);
    Some(summary)
}

/// Whether a sealed trace waits for the writer: the next feed's claim lets
/// it go first.
pub fn queued() -> bool {
    state() == Hand::Sealed
}

/// The loop's half: one page of feed `feed`'s trace, if it is the one in
/// RAM: `Busy` while it is still recorded. `None` for any other feed.
pub fn page(feed: u32, offset: u16, out: &mut [u8]) -> Option<Result<TraceRead, Refusal>> {
    let held = state();
    // SAFETY: only the loop calls this, and only the loop mutates the trace
    // (while `Recording`), so nothing writes it during this read.
    let trace = unsafe { &*TRACE.get() };
    if held == Hand::Empty || trace.feed() != feed {
        return None;
    }
    if held == Hand::Recording {
        return Some(Err(Refusal::Busy));
    }
    Some(trace.page(offset, out).map(|read| TraceRead {
        stored: held == Hand::Stored,
        ..read
    }))
}

/// The writer's half: append the sealed trace with `append`, which says
/// whether it was stored; `None` when no trace waits.
pub fn write_sealed(append: impl FnOnce(&TraceSummary, &[u8]) -> bool) -> Option<(u32, bool)> {
    if !hand_over(Hand::Sealed, Hand::Writing) {
        return None;
    }
    // SAFETY: `Writing` keeps the loop from taking the trace back, and the
    // loop's own reads do not write it.
    let trace = unsafe { &*TRACE.get() };
    let feed = trace.feed();
    let stored = trace
        .summary()
        .is_some_and(|summary| append(summary, trace.body()));
    if !stored {
        UNSAVED.fetch_add(1, Ordering::Relaxed);
    }
    let next = if stored { Hand::Stored } else { Hand::Unsaved };
    STATE.store(next as u8, Ordering::Release);
    Some((feed, stored))
}

/// The writer's half: the ring's fill now.
pub fn publish_fill(kept: u32, capacity: u32) {
    KEPT.store(kept, Ordering::Relaxed);
    CAPACITY.store(capacity, Ordering::Relaxed);
}

/// The ring's fill as last published, and the traces lost before it.
pub fn fill() -> TracesFill {
    TracesFill {
        kept: KEPT.load(Ordering::Relaxed),
        capacity: CAPACITY.load(Ordering::Relaxed),
        unsaved: UNSAVED.load(Ordering::Relaxed),
    }
}

/// Host tests only: back to the boot state.
#[cfg(not(target_os = "none"))]
pub fn reset() {
    STATE.store(Hand::Empty as u8, Ordering::Release);
    UNSAVED.store(0, Ordering::Relaxed);
    publish_fill(0, 0);
    // SAFETY: tests reset between runs, with no other user of the trace.
    unsafe { (*TRACE.get()).begin(0) };
}
