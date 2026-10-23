//! Exports declared by `feed_controller.h`.

use petlibre_core::feed::controller::{
    FeedController, FeedLimits, FeedReason, FeedRequest, FeedSample, FeedStartResult,
};

use super::store;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_feed_controller_init(
    controller: *mut FeedController,
    limits: *const FeedLimits,
    last_request_id: u64,
) -> bool {
    // SAFETY: the C contract: `limits` is null or valid for the call.
    match unsafe { limits.as_ref() } {
        Some(limits) => store(
            // SAFETY: the C contract: `controller` is null or valid for the call.
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
        // SAFETY: the C contract: `controller` is null or valid for the call.
        unsafe { controller.as_mut() },
        // SAFETY: the C contract: `request` is null or valid for the call.
        unsafe { request.as_ref() },
        // SAFETY: the C contract: `sample` is null or valid for the call.
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
    // SAFETY: the C contract: `controller` is null or valid for the call.
    if let Some(controller) = unsafe { controller.as_mut() } {
        // SAFETY: the C contract: `sample` is null or valid for the call.
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
    // SAFETY: the C contract: `controller` is null or valid for the call.
    if let Some(controller) = unsafe { controller.as_mut() } {
        // SAFETY: the C contract: `sample` is null or valid for the call.
        controller.update_captured(now_ms, unsafe { sample.as_ref() }, captured_at_ms);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_feed_controller_trip(
    controller: *mut FeedController,
    reason: FeedReason,
) -> bool {
    // SAFETY: the C contract: `controller` is null or valid for the call.
    unsafe { controller.as_mut() }.is_some_and(|c| c.trip(reason))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_feed_controller_cancel(
    controller: *mut FeedController,
    id: u64,
) -> bool {
    // SAFETY: the C contract: `controller` is null or valid for the call.
    unsafe { controller.as_mut() }.is_some_and(|c| c.cancel(id))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_feed_controller_reset(controller: *mut FeedController) -> bool {
    // SAFETY: the C contract: `controller` is null or valid for the call.
    unsafe { controller.as_mut() }.is_some_and(FeedController::reset)
}
