//! FW45: the client store's changes and the feeder's identity, carried
//! between the loop task and the settings writer.
//!
//! The settings writer alone writes the client store (storage sectors 9 and
//! 10), and the loop task alone runs handshakes and reads the panel. So the
//! loop queues a change here with [`request`] (an enrollment once a
//! handshake admits a client, a revocation, or the FP15 reset), and the
//! writer takes it with [`pending`] and frees the slot with [`finish`],
//! naming the [`KeyResult`]. A second change before the writer takes the
//! first is [`Requested::Busy`], never queued behind it. A caller that must
//! know the result (BLE setup answers its client) keeps the [`ticket`] of
//! its change and reads [`outcome`].
//!
//! The other way, the writer publishes the identity secret it holds with
//! [`publish_identity`], each time with a new generation, and the loop
//! copies it with [`identity_since`] to derive the public key. The secret
//! stays in RAM: no log, request or record carries it.

use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use petlibre_core::named_enum;
use petlibre_core::store::clients::{ClientError, Label, Role};

use crate::cell::TaskCell;
use crate::sdk;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Enroll {
        key: [u8; 32],
        label: Label,
        role: Role,
    },
    Revoke {
        key: [u8; 32],
    },
    /// FP15: forget every client and take a new identity.
    Reset,
}

named_enum! {
    pub enum Requested {
        Queued = "queued",
        /// A change is already waiting for the writer.
        Busy = "busy",
    }
}

named_enum! {
    /// What became of a queued client store change.
    pub enum KeyResult {
        Done = "done",
        Unarmed = "unarmed",
        /// A reset needs a new identity, and the generator never started.
        NoGenerator = "no-generator",
        NotWritable = "not-writable",
        Full = "full",
        Duplicate = "duplicate",
        NotFound = "not-found",
        Io = "io",
    }
}

impl From<ClientError> for KeyResult {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::NotWritable | ClientError::IdentitySet => Self::NotWritable,
            ClientError::Full => Self::Full,
            ClientError::Duplicate => Self::Duplicate,
            ClientError::NotFound => Self::NotFound,
            ClientError::Io => Self::Io,
        }
    }
}

/// Who holds `PENDING`, as `STATE` stores it.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Hand {
    Empty = 0,
    /// The claimant is writing the change.
    Filling = 1,
    /// Handed to the writer until `finish`.
    Queued = 2,
}

/// Written only by the claimant of `STATE` (`Empty` to `Filling`), read only
/// by the writer after `Queued`.
static PENDING: TaskCell<Option<Change>> = TaskCell::new(None);
static STATE: AtomicU8 = AtomicU8::new(Hand::Empty as u8);
/// The latest queued change's ticket; only the claimant writes it.
static TICKET: AtomicU32 = AtomicU32::new(0);
/// The last finished change: its ticket's low 24 bits, then its result's
/// index in `KeyResult::ALL` plus one (0 before any).
static FINISHED: AtomicU32 = AtomicU32::new(0);

/// Written by the writer and read by the loop, each with the scheduler
/// paused; `GENERATION` is 0 until the first publish.
static IDENTITY: TaskCell<[u8; 32]> = TaskCell::new([0; 32]);
static GENERATION: AtomicU32 = AtomicU32::new(0);

/// The loop's half: queue one change for the writer.
pub fn request(change: Change) -> Requested {
    if STATE
        .compare_exchange(
            Hand::Empty as u8,
            Hand::Filling as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_err()
    {
        return Requested::Busy;
    }
    // SAFETY: `Filling` makes this the only accessor until `Queued`.
    unsafe { *PENDING.get() = Some(change) };
    TICKET.store(
        TICKET.load(Ordering::Acquire).wrapping_add(1),
        Ordering::Release,
    );
    STATE.store(Hand::Queued as u8, Ordering::Release);
    Requested::Queued
}

/// The ticket of the change `request` last queued.
pub fn ticket() -> u32 {
    TICKET.load(Ordering::Acquire)
}

/// The result of the change with `ticket`, once the writer has finished it.
pub fn outcome(ticket: u32) -> Option<KeyResult> {
    let finished = FINISHED.load(Ordering::Acquire);
    if finished >> 8 != ticket & 0x00ff_ffff {
        return None;
    }
    let index = usize::try_from(finished & 0xff).ok()?.checked_sub(1)?;
    KeyResult::ALL.get(index).copied()
}

/// The writer's half: the queued change, if any.
pub fn pending() -> Option<Change> {
    if STATE.load(Ordering::Acquire) != Hand::Queued as u8 {
        return None;
    }
    // SAFETY: `Queued` hands the value to the writer until `finish`.
    unsafe { *PENDING.get() }
}

/// The writer's half: the change is applied or refused; free the slot.
pub fn finish(result: KeyResult) {
    let index = KeyResult::ALL
        .iter()
        .position(|&r| r == result)
        .and_then(|i| u32::try_from(i + 1).ok())
        .unwrap_or(0);
    FINISHED.store(
        (TICKET.load(Ordering::Acquire) & 0x00ff_ffff) << 8 | index,
        Ordering::Release,
    );
    STATE.store(Hand::Empty as u8, Ordering::Release);
}

/// The writer's half: the identity the store now holds.
pub fn publish_identity(secret: &[u8; 32]) {
    sdk::unscheduled(|| {
        // SAFETY: with the scheduler paused no other task runs, and no
        // interrupt handler reaches the identity.
        unsafe { *IDENTITY.get() = *secret };
        let next = GENERATION.load(Ordering::Acquire).wrapping_add(1).max(1);
        GENERATION.store(next, Ordering::Release);
    });
}

/// The loop's half: the identity and its generation, if published since
/// `seen` (0 before the first).
pub fn identity_since(seen: u32) -> Option<(u32, [u8; 32])> {
    sdk::unscheduled(|| {
        let generation = GENERATION.load(Ordering::Acquire);
        // SAFETY: as in `publish_identity`.
        (generation != 0 && generation != seen).then(|| (generation, unsafe { *IDENTITY.get() }))
    })
}

/// Forget everything (host tests).
#[cfg(not(target_os = "none"))]
pub fn reset() {
    STATE.store(Hand::Empty as u8, Ordering::Release);
    TICKET.store(0, Ordering::Release);
    FINISHED.store(0, Ordering::Release);
    GENERATION.store(0, Ordering::Release);
    // SAFETY: host tests serialize on this module's state.
    unsafe { *IDENTITY.get() = [0; 32] };
}
