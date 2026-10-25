//! The update's requests and chunks, carried between the loop task and
//! the service task.
//!
//! The loop serves the device API on an 8 KiB stack, too little for an
//! Ed25519 check, so it never verifies anything: it copies an offer, a key
//! statement or a cancel here ([`request`]) and a chunk ([`chunk`]), and
//! answers from what the service task's update stage publishes
//! ([`outcome`], [`progress`]). The stage takes them with [`pending`] and
//! [`take_chunk`], frees the request slot with [`finish`], and publishes
//! its state after every step with [`publish`].
//!
//! **The install lease.** Every request takes it as it is queued, in the
//! loop that is also the only task that starts feeds, so no feed can start
//! once the stage may write the update store or bank 1, and none runs while
//! it does: the loop refuses a request while a feed is queued or driven, and
//! a feed while the lease is held ([`installing`]). [`finish`] releases it
//! for every request but an accepted offer, whose install or give-way keeps
//! it: the install until it stops, a give-way or an activation until the
//! restart. A cancel during an install rides on the install's lease. The
//! stage's writes themselves wait for the write turn (`interlock`), which
//! keeps them clear of a feed that started before the lease.

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use wellfed_core::ota::MAX_CHUNK;
use wellfed_core::ota::install::{Progress, Refused};
use wellfed_core::ota::offer::{
    DOWNGRADE_BODY, KeyStatement, MANIFEST_LEN, SIGNATURE_LEN, STATEMENT_MAX,
};

use crate::cell::TaskCell;
use crate::sdk;

/// A downgrade statement's whole length.
pub const DOWNGRADE_LEN: usize = DOWNGRADE_BODY + SIGNATURE_LEN;
const CHUNK: usize = MAX_CHUNK as usize;
/// Chunks the mailbox holds while the stage writes a sector.
pub const CHUNKS: usize = 2;

/// An update request, copied out of the session's buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    Offer {
        manifest: [u8; MANIFEST_LEN],
        downgrade: Option<[u8; DOWNGRADE_LEN]>,
        /// The feeder's identity key, which a downgrade statement names.
        device: [u8; 32],
    },
    Statement {
        bytes: [u8; STATEMENT_MAX],
        len: u8,
    },
    Cancel,
}

impl Request {
    /// An offer's request, refused before it is queued when a part has the
    /// wrong length: no manifest the stage takes has another.
    pub fn offer(
        manifest: &[u8],
        downgrade: Option<&[u8]>,
        device: [u8; 32],
    ) -> Result<Self, Refused> {
        use wellfed_core::ota::offer::{OfferError, StatementError};
        let manifest = <[u8; MANIFEST_LEN]>::try_from(manifest)
            .map_err(|_| Refused::Offer(OfferError::Length))?;
        let downgrade = match downgrade {
            Some(bytes) => Some(
                <[u8; DOWNGRADE_LEN]>::try_from(bytes)
                    .map_err(|_| Refused::Offer(OfferError::Statement(StatementError::Length)))?,
            ),
            None => None,
        };
        Ok(Self::Offer {
            manifest,
            downgrade,
            device,
        })
    }

    pub fn statement(bytes: &[u8]) -> Result<Self, Refused> {
        use wellfed_core::ota::offer::StatementError;
        let len = u8::try_from(bytes.len())
            .ok()
            .filter(|&len| usize::from(len) <= STATEMENT_MAX)
            .ok_or(Refused::Statement(StatementError::Length))?;
        let mut out = [0; STATEMENT_MAX];
        out[..bytes.len()].copy_from_slice(bytes);
        Ok(Self::Statement { bytes: out, len })
    }
}

/// What a request came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Progress(Progress),
    /// A key statement took effect.
    Key(KeyStatement),
}

/// Who holds a slot, as its state byte stores it.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hand {
    Empty = 0,
    /// The claimant is writing it.
    Filling = 1,
    /// Handed to the stage.
    Queued = 2,
}

fn claim(state: &AtomicU8) -> bool {
    state
        .compare_exchange(
            Hand::Empty as u8,
            Hand::Filling as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok()
}

static REQUEST: TaskCell<Option<Request>> = TaskCell::new(None);
static REQUEST_STATE: AtomicU8 = AtomicU8::new(Hand::Empty as u8);
static TICKET: AtomicU32 = AtomicU32::new(0);
/// The last finished request's ticket and answer; written by the stage
/// with the scheduler paused.
static OUTCOME: TaskCell<Option<(u32, Result<Answer, Refused>)>> = TaskCell::new(None);
/// The loop read the last outcome: its answer is on its way out.
static TAKEN: AtomicBool = AtomicBool::new(false);

struct Chunk {
    offset: u32,
    len: u16,
    data: [u8; CHUNK],
}

static CHUNKS_CELL: TaskCell<[Chunk; CHUNKS]> = TaskCell::new(
    [const {
        Chunk {
            offset: 0,
            len: 0,
            data: [0; CHUNK],
        }
    }; CHUNKS],
);
static CHUNK_STATE: [AtomicU8; CHUNKS] = [const { AtomicU8::new(Hand::Empty as u8) }; CHUNKS];

/// The stage's state, published with the scheduler paused.
static PROGRESS: TaskCell<Progress> = TaskCell::new(Progress::idle(0, 0));
static PUBLISHED: AtomicBool = AtomicBool::new(false);
static LEASE: AtomicBool = AtomicBool::new(false);
/// The queued request took the lease (a cancel during an install did not).
static TOOK: AtomicBool = AtomicBool::new(false);

/// The loop's half: queue one request; its ticket. It takes the install
/// lease with it; only a cancel goes in while an install holds it.
pub fn request(request: Request) -> Result<u32, Refused> {
    if !claim(&REQUEST_STATE) {
        return Err(Refused::Busy);
    }
    let took = !LEASE.swap(true, Ordering::AcqRel);
    if !took && !matches!(request, Request::Cancel) {
        REQUEST_STATE.store(Hand::Empty as u8, Ordering::Release);
        return Err(Refused::Installing);
    }
    TOOK.store(took, Ordering::Release);
    // SAFETY: `Filling` makes this the only accessor until `Queued`.
    unsafe { *REQUEST.get() = Some(request) };
    let ticket = TICKET.load(Ordering::Acquire).wrapping_add(1).max(1);
    TICKET.store(ticket, Ordering::Release);
    REQUEST_STATE.store(Hand::Queued as u8, Ordering::Release);
    Ok(ticket)
}

/// The loop's half: the answer to the request with `ticket`, once done.
pub fn outcome(ticket: u32) -> Option<Result<Answer, Refused>> {
    let outcome = sdk::unscheduled(|| {
        // SAFETY: the stage writes it only with the scheduler paused.
        unsafe { *OUTCOME.get() }
    });
    let (_, answer) = outcome.filter(|&(done, _)| done == ticket)?;
    TAKEN.store(true, Ordering::Release);
    Some(answer)
}

/// The loop's half: hand one chunk to the stage, or `Busy` while both
/// slots wait to be written.
pub fn chunk(offset: u32, data: &[u8]) -> Result<(), Refused> {
    let Ok(len) = u16::try_from(data.len()) else {
        return Err(Refused::Busy);
    };
    if data.len() > CHUNK {
        return Err(Refused::Busy);
    }
    let Some(index) = CHUNK_STATE.iter().position(claim) else {
        return Err(Refused::Busy);
    };
    // SAFETY: `Filling` makes this slot the claimant's until `Queued`; the
    // other slot is not touched.
    let slot = unsafe { &mut (*CHUNKS_CELL.get())[index] };
    slot.offset = offset;
    slot.len = len;
    slot.data[..data.len()].copy_from_slice(data);
    CHUNK_STATE[index].store(Hand::Queued as u8, Ordering::Release);
    Ok(())
}

/// The loop's half: the stage's state, with the chunks still waiting in
/// the mailbox taken out of what it wants.
pub fn progress() -> Option<Progress> {
    if !PUBLISHED.load(Ordering::Acquire) {
        return None;
    }
    let mut progress = sdk::unscheduled(|| {
        // SAFETY: as in `publish`.
        unsafe { *PROGRESS.get() }
    });
    for (index, state) in CHUNK_STATE.iter().enumerate() {
        if state.load(Ordering::Acquire) == Hand::Queued as u8 {
            // SAFETY: a queued slot is only read until the stage frees it,
            // and only the stage frees it; reading the offset is a copy.
            let offset = unsafe { (*CHUNKS_CELL.get())[index].offset };
            for wanted in &mut progress.wanted {
                if *wanted == Some(offset) {
                    *wanted = None;
                }
            }
        }
    }
    Some(progress)
}

/// A request or an install holds the flash: no feed starts.
pub fn installing() -> bool {
    LEASE.load(Ordering::Acquire)
}

/// The stage's half: the queued request, if any.
pub fn pending() -> Option<Request> {
    if REQUEST_STATE.load(Ordering::Acquire) != Hand::Queued as u8 {
        return None;
    }
    // SAFETY: `Queued` hands the value to the stage until `finish`.
    unsafe { *REQUEST.get() }
}

/// The stage's half: the request is answered; free the slot. The lease it
/// took goes with it, unless it was an offer the stage accepted.
pub fn finish(answer: Result<Answer, Refused>) {
    // SAFETY: `Queued` hands the slot to the stage until here.
    let offer = matches!(unsafe { *REQUEST.get() }, Some(Request::Offer { .. }));
    let ticket = TICKET.load(Ordering::Acquire);
    sdk::unscheduled(|| {
        // Cleared with the outcome, so the loop never reads the new answer
        // as already taken.
        TAKEN.store(false, Ordering::Release);
        // SAFETY: with the scheduler paused no other task runs, and no
        // interrupt handler reaches the outcome.
        unsafe { *OUTCOME.get() = Some((ticket, answer)) };
    });
    if TOOK.swap(false, Ordering::AcqRel) && !(offer && answer.is_ok()) {
        release();
    }
    REQUEST_STATE.store(Hand::Empty as u8, Ordering::Release);
}

/// The stage's half: whether the loop has read the last outcome.
pub fn taken() -> bool {
    TAKEN.load(Ordering::Acquire)
}

/// The stage's half: the oldest waiting chunk, given to `take` in place;
/// its slot is free again afterwards. False when none waits.
pub fn take_chunk(take: impl FnOnce(u32, &[u8])) -> bool {
    let Some(index) = CHUNK_STATE
        .iter()
        .position(|state| state.load(Ordering::Acquire) == Hand::Queued as u8)
    else {
        return false;
    };
    // SAFETY: `Queued` hands this slot to the stage until it is freed below.
    let slot = unsafe { &(*CHUNKS_CELL.get())[index] };
    take(slot.offset, &slot.data[..usize::from(slot.len).min(CHUNK)]);
    CHUNK_STATE[index].store(Hand::Empty as u8, Ordering::Release);
    true
}

/// The stage's half: drop every waiting chunk (the install stopped).
pub fn drop_chunks() {
    for state in &CHUNK_STATE {
        let _ = state.compare_exchange(
            Hand::Queued as u8,
            Hand::Empty as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

/// The stage's half: its state now.
pub fn publish(progress: &Progress) {
    sdk::unscheduled(|| {
        // SAFETY: as in `finish`.
        unsafe { *PROGRESS.get() = *progress };
    });
    PUBLISHED.store(true, Ordering::Release);
}

/// The stage's half: the install no longer holds the flash.
pub fn release() {
    LEASE.store(false, Ordering::Release);
}

/// Forget everything (host tests).
#[cfg(not(target_os = "none"))]
pub fn reset() {
    REQUEST_STATE.store(Hand::Empty as u8, Ordering::Release);
    TICKET.store(0, Ordering::Release);
    TAKEN.store(false, Ordering::Release);
    PUBLISHED.store(false, Ordering::Release);
    LEASE.store(false, Ordering::Release);
    TOOK.store(false, Ordering::Release);
    for state in &CHUNK_STATE {
        state.store(Hand::Empty as u8, Ordering::Release);
    }
    // SAFETY: host tests serialize on this module's state.
    unsafe {
        *REQUEST.get() = None;
        *OUTCOME.get() = None;
        *PROGRESS.get() = Progress::idle(0, 0);
    }
}
