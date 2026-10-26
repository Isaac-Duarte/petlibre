//! The event recorder: one durable event queued for the settings writer,
//! which alone appends the ring, stamped with the wall clock when it is
//! valid. The device API's events, the panel's lock changes, the boot and a
//! feed's end all come through here.

use wellfed_proto::api::EventBody;

use super::platform::{EventSlot, Platform};
use crate::store::events::BODY_BYTES;

/// Queue `body` at `now_ms`. A full queue counts it lost, and the loop
/// records the count as an event of its own.
pub fn record(platform: &mut impl Platform, now_ms: u32, body: EventBody) {
    queue(platform, now_ms, body, EventSlot::Shared);
}

/// As [`record`] for a feed's `Fed` event, which may take the queue's
/// reserved slot: appends wait while a feed holds the outputs, so the
/// events of a long feed pile up, and the feed's own result must not be the
/// one lost.
pub fn record_outcome(platform: &mut impl Platform, now_ms: u32, body: EventBody) {
    queue(platform, now_ms, body, EventSlot::Outcome);
}

fn queue(platform: &mut impl Platform, now_ms: u32, body: EventBody, slot: EventSlot) {
    let at = platform
        .wall_ms(now_ms)
        .and_then(|ms| u32::try_from(ms / 1000).ok());
    let mut bytes = [0u8; BODY_BYTES];
    // Every body fits the ring's record; one that did not would be a lost
    // event.
    if wellfed_proto::encode(&body, &mut bytes).is_ok() {
        let _ = platform.queue_event(at, bytes, slot);
    }
}
