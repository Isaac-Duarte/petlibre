//! Exports declared by `feed_controller.h`.

use petlibre_core::feed_controller::{
    FeedController, FeedLimits, FeedReason, FeedRequest, FeedSample, FeedStartResult,
};

use super::store;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_feed_controller_init(
    controller: *mut FeedController,
    limits: *const FeedLimits,
    last_request_id: u64,
) -> bool {
    match unsafe { limits.as_ref() } {
        Some(limits) => store(
            unsafe { controller.as_mut() },
            FeedController::new(limits, last_request_id),
        ),
        None => false,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_feed_controller_start(
    controller: *mut FeedController,
    request: *const FeedRequest,
    now_ms: u32,
    sample: *const FeedSample,
) -> FeedStartResult {
    match (
        unsafe { controller.as_mut() },
        unsafe { request.as_ref() },
        unsafe { sample.as_ref() },
    ) {
        (Some(controller), Some(request), Some(sample)) => {
            controller.start(request, now_ms, sample)
        }
        _ => FeedStartResult::REJECT_REQUEST,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_feed_controller_update(
    controller: *mut FeedController,
    now_ms: u32,
    sample: *const FeedSample,
) {
    if let Some(controller) = unsafe { controller.as_mut() } {
        controller.update(now_ms, unsafe { sample.as_ref() });
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_feed_controller_update_captured(
    controller: *mut FeedController,
    now_ms: u32,
    sample: *const FeedSample,
    captured_at_ms: u32,
) {
    if let Some(controller) = unsafe { controller.as_mut() } {
        controller.update_captured(now_ms, unsafe { sample.as_ref() }, captured_at_ms);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_feed_controller_trip(
    controller: *mut FeedController,
    reason: FeedReason,
) -> bool {
    unsafe { controller.as_mut() }.is_some_and(|c| c.trip(reason))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_feed_controller_cancel(
    controller: *mut FeedController,
    id: u64,
) -> bool {
    unsafe { controller.as_mut() }.is_some_and(|c| c.cancel(id))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_feed_controller_reset(controller: *mut FeedController) -> bool {
    unsafe { controller.as_mut() }.is_some_and(FeedController::reset)
}
