//! Exports declared by `feed_progress.h`.

use petlibre_core::feed::progress::{BowlMode, Direction, FeedProgress, ProgressResult};

use super::store;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_feed_progress_init(
    progress: *mut FeedProgress,
    mode: BowlMode,
    direction: Direction,
    portions: u16,
    credit_next_event: bool,
) -> bool {
    store(
        // SAFETY: the C contract: `progress` is null or valid for the call.
        unsafe { progress.as_mut() },
        FeedProgress::new(mode, direction, portions, credit_next_event),
    )
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_feed_progress_accept(
    progress: *mut FeedProgress,
) -> ProgressResult {
    // SAFETY: the C contract: `progress` must be valid: like the C original, this does not check for null.
    unsafe { &mut *progress }.accept()
}
