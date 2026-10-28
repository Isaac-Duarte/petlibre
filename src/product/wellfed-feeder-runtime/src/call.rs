//! The meal call between the loop and the writer.
//!
//! One slot carries the device API's work to the writer: an upload's
//! start, a delete, or one chunk of the upload, whose bytes wait in
//! [`DATA`] so neither task holds them on its stack. The writer applies it
//! to storage sectors 40-47 (`wellfed_feeder::store::call`) and publishes
//! the result ([`publish`]), and tells the speaker which clip it may play.
//! Nothing here writes flash.

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use wellfed_core::clip::CHUNK_MAX;
use wellfed_core::store::slot::SlotIo;
use wellfed_device_api::mailbox::{CallChange, CallClip, CallResult, CallView, chunk_wanted};
use wellfed_feeder::store::call::CallStore;
use wellfed_hal::cell::TaskCell;
use wellfed_hal::sdk::IrqGuard;
use wellfed_proto::api::Refusal;

/// The call's sectors within the storage region.
pub const CALL_BASE: u32 = wellfed_hal::plaf103::storage::CALL.first;
pub const CALL_UNITS: u32 = wellfed_hal::plaf103::storage::CALL.count;

/// What waits for the writer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Queued {
    Change(CallChange),
    /// `len` bytes of [`DATA`] at stream offset `offset`.
    Chunk {
        offset: u32,
        len: u16,
    },
}

/// Who holds `PENDING` and `DATA`, as `STATE` stores it.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Hand {
    Empty = 0,
    Filling = 1,
    Queued = 2,
}

/// Written only by the claimant of `STATE` (`Empty` to `Filling`), read
/// only by the writer after `Queued`.
static PENDING: TaskCell<Queued> = TaskCell::new(Queued::Change(CallChange::Delete));
static DATA: TaskCell<[u8; CHUNK_MAX as usize]> = TaskCell::new([0; CHUNK_MAX as usize]);
static STATE: AtomicU8 = AtomicU8::new(Hand::Empty as u8);

/// What NOR holds, published by the writer, copied in and out with
/// interrupts masked.
static PUBLISHED: TaskCell<CallView> = TaskCell::new(CallView {
    writable: false,
    clip: CallClip::None,
});
static LOADED: AtomicBool = AtomicBool::new(false);

fn view() -> CallView {
    let _guard = IrqGuard::lock();
    // SAFETY: interrupts are masked, so this is the only reference.
    unsafe { *PUBLISHED.get() }
}

fn waiting() -> bool {
    STATE.load(Ordering::Acquire) != Hand::Empty as u8
}

/// The call as the writer last published it, and whether work waits;
/// `Busy` until the writer has opened the store.
pub fn current() -> CallResult {
    if !LOADED.load(Ordering::Acquire) {
        return CallResult::Busy;
    }
    CallResult::Current {
        view: view(),
        pending: waiting(),
    }
}

/// Claim the slot and fill it; false while it is taken.
fn queue(fill: impl FnOnce(&mut Queued, &mut [u8; CHUNK_MAX as usize])) -> bool {
    if STATE
        .compare_exchange(
            Hand::Empty as u8,
            Hand::Filling as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_err()
    {
        return false;
    }
    // SAFETY: `Filling` makes this the only accessor until `Queued`.
    unsafe { fill(&mut *PENDING.get(), &mut *DATA.get()) };
    STATE.store(Hand::Queued as u8, Ordering::Release);
    true
}

/// Queue an upload's start or a delete for the writer.
pub fn change(change: CallChange) -> CallResult {
    if !LOADED.load(Ordering::Acquire) {
        return CallResult::Busy;
    }
    let view = view();
    if !view.writable {
        return CallResult::ReadOnly;
    }
    // Refused rather than queued for the seconds a call plays: the writer
    // could not take the sectors until it ends.
    if wellfed_hal::audio::call_playing() {
        return CallResult::Busy;
    }
    if !queue(|pending, _| *pending = Queued::Change(change)) {
        return CallResult::Busy;
    }
    CallResult::Current {
        view,
        pending: true,
    }
}

/// Queue one chunk of the upload under way: `Busy` while the last one is
/// still being written (the upload stands where it was until then).
pub fn chunk(offset: u32, data: &[u8]) -> Result<(), Refusal> {
    if !LOADED.load(Ordering::Acquire) {
        return Err(Refusal::Busy);
    }
    let view = view();
    if !view.writable {
        return Err(Refusal::NotWritable);
    }
    match chunk_wanted(view.clip, offset, data.len()) {
        Ok(false) => Ok(()),
        Ok(true) if wellfed_hal::audio::call_playing() => Err(Refusal::Busy),
        Ok(true) => {
            let len = u16::try_from(data.len()).map_err(|_| Refusal::Invalid)?;
            let queued = queue(|pending, bytes| {
                *pending = Queued::Chunk { offset, len };
                if let Some(head) = bytes.get_mut(..data.len()) {
                    head.copy_from_slice(data);
                }
            });
            if queued { Ok(()) } else { Err(Refusal::Busy) }
        }
        // The next chunk, sent while the one before is still queued.
        Err(_) if waiting() => Err(Refusal::Busy),
        Err(refusal) => Err(refusal),
    }
}

/// The writer's half: run `write` on the queued work and its bytes, if
/// any work waits; its result.
pub fn with_pending<R>(write: impl FnOnce(Queued, &[u8]) -> R) -> Option<R> {
    if STATE.load(Ordering::Acquire) != Hand::Queued as u8 {
        return None;
    }
    // SAFETY: `Queued` hands both to the writer until `finish`.
    let (queued, data) = unsafe { (*PENDING.get(), &*DATA.get()) };
    let bytes = match queued {
        Queued::Chunk { len, .. } => data.get(..usize::from(len)).unwrap_or(&[]),
        Queued::Change(_) => &[],
    };
    Some(write(queued, bytes))
}

pub fn finish() {
    STATE.store(Hand::Empty as u8, Ordering::Release);
}

/// The writer publishes what NOR holds, writable only once it is `armed`,
/// and the clip the speaker may play.
pub fn publish<I: SlotIo>(store: &CallStore<I>, armed: bool) {
    {
        let _guard = IrqGuard::lock();
        // SAFETY: interrupts are masked, so this is the only reference.
        unsafe { *PUBLISHED.get() = store.view(armed) };
    }
    wellfed_hal::audio::publish_call(store.stored());
    LOADED.store(true, Ordering::Release);
}

/// Forget everything (host tests).
pub fn reset() {
    {
        let _guard = IrqGuard::lock();
        // SAFETY: interrupts are masked, so this is the only reference.
        unsafe {
            *PUBLISHED.get() = CallView {
                writable: false,
                clip: CallClip::None,
            };
        }
    }
    LOADED.store(false, Ordering::Release);
    STATE.store(Hand::Empty as u8, Ordering::Release);
}
