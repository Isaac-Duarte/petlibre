//! Exports declared by `feed_journal.h`.

use wellfed_core::feed_journal::{
    FeedJournal, JournalConfig, JournalError, JournalIo, JournalView,
};
use wellfed_core::feed_progress::Direction;
use wellfed_core::feed_storage::OwnerStorage;

use super::store;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_feed_journal_open(
    journal: *mut FeedJournal,
    io: *const JournalIo,
    config: *const JournalConfig,
) -> JournalError {
    if journal.is_null() {
        return JournalError::ARGUMENT;
    }
    // Copy first: the arguments may alias the journal's own saved values.
    let io = unsafe { io.as_ref() }.copied().unwrap_or_default();
    let config = unsafe { config.as_ref() }.copied().unwrap_or_default();
    // SAFETY: the C contract keeps the I/O callbacks valid for their context.
    unsafe { (*journal).open(io, config) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_feed_journal_create(
    journal: *mut FeedJournal,
    io: *const JournalIo,
    config: *const JournalConfig,
    direction: Direction,
    credit_next_event: bool,
    phase_known: bool,
) -> JournalError {
    if journal.is_null() {
        return JournalError::ARGUMENT;
    }
    let io = unsafe { io.as_ref() }.copied().unwrap_or_default();
    let config = unsafe { config.as_ref() }.copied().unwrap_or_default();
    // SAFETY: as for open.
    unsafe { (*journal).create(io, config, direction, credit_next_event, phase_known) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_feed_journal_read(
    journal: *const FeedJournal,
    view: *mut JournalView,
) -> bool {
    match unsafe { journal.as_ref() } {
        Some(journal) if !view.is_null() => store(unsafe { view.as_mut() }, journal.read()),
        _ => false,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_feed_journal_storage(journal: *mut FeedJournal) -> OwnerStorage {
    match unsafe { journal.as_mut() } {
        Some(journal) => journal.storage(),
        None => OwnerStorage {
            context: core::ptr::null_mut(),
            ..FeedJournal::default().storage()
        },
    }
}
