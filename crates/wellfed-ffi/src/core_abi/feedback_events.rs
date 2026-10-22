//! Exports declared by `feedback_events.h`.

use wellfed_core::feed_controller::{FeedController, FeedReason, FeedSample};
use wellfed_core::feedback_events::{
    FeedbackDrive, FeedbackEvent, FeedbackEvents, FeedbackFault, FeedbackTakeResult,
};

use super::store;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_feedback_events_init(
    events: *mut FeedbackEvents,
    capacity: u8,
    max_age_ms: u32,
    now_ms: u32,
) -> bool {
    store(
        unsafe { events.as_mut() },
        FeedbackEvents::new(capacity, max_age_ms, now_ms),
    )
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_feedback_events_push(
    events: *mut FeedbackEvents,
    captured_at_ms: u32,
    high: bool,
) -> bool {
    unsafe { events.as_mut() }.is_some_and(|e| e.push(captured_at_ms, high))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_feedback_events_take(
    events: *mut FeedbackEvents,
    now_ms: u32,
    event: *mut FeedbackEvent,
) -> FeedbackTakeResult {
    let Some(events) = (unsafe { events.as_mut() }) else {
        return FeedbackTakeResult::FAILED;
    };
    let Some(out) = (unsafe { event.as_mut() }) else {
        events.fail(FeedbackFault::ARGUMENT);
        return FeedbackTakeResult::FAILED;
    };

    let (result, taken) = events.take(now_ms);
    if let Some(taken) = taken {
        *out = taken;
    }
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_feedback_events_process(
    events: *mut FeedbackEvents,
    controller: *mut FeedController,
    now_ms: u32,
    sample: *const FeedSample,
    drive: FeedbackDrive,
) -> bool {
    match (
        unsafe { events.as_mut() },
        unsafe { controller.as_mut() },
        unsafe { sample.as_ref() },
    ) {
        (Some(events), Some(controller), Some(sample)) => {
            events.process(controller, now_ms, sample, drive)
        }
        (events, controller, _) => {
            // A missing argument is a stream fault: fault whichever queue is present and
            // trip whichever controller is present.
            if let Some(events) = events {
                events.fail(FeedbackFault::ARGUMENT);
            }
            if let Some(controller) = controller {
                controller.trip(FeedReason::FEEDBACK_STREAM);
            }
            false
        }
    }
}
