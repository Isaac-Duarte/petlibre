//! Exports declared by `flash_coord.h`.

use wellfed_core::flash_coord::{
    FlashCoord, FlashFault, FlashLimits, FlashParty, FlashReply, FlashToken,
};

use super::store;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_flash_coord_init(
    coord: *mut FlashCoord,
    limits: *const FlashLimits,
    authority_session: u64,
    now_ms: u32,
) -> bool {
    match unsafe { limits.as_ref() } {
        Some(limits) => store(
            unsafe { coord.as_mut() },
            FlashCoord::new(limits, authority_session, now_ms),
        ),
        None => false,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_flash_coord_ready(
    coord: *mut FlashCoord,
    authority_session: u64,
    peer_session: u64,
    inactive: bool,
    now_ms: u32,
) -> FlashReply {
    unsafe { coord.as_mut() }.map_or(FlashReply::INVALID, |c| {
        c.ready(authority_session, peer_session, inactive, now_ms)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_flash_coord_request(
    coord: *mut FlashCoord,
    requester: FlashParty,
    session: u64,
    request_id: u64,
    now_ms: u32,
) -> FlashReply {
    unsafe { coord.as_mut() }.map_or(FlashReply::INVALID, |c| {
        c.request(requester, session, request_id, now_ms)
    })
}

/// Token messages: a null coordinator or token is INVALID.
fn token_message(
    coord: *mut FlashCoord,
    token: *const FlashToken,
    f: impl FnOnce(&mut FlashCoord, &FlashToken) -> FlashReply,
) -> FlashReply {
    // SAFETY: C contract — null or valid pointers.
    match (unsafe { coord.as_mut() }, unsafe { token.as_ref() }) {
        (Some(c), Some(t)) => f(c, t),
        _ => FlashReply::INVALID,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_flash_coord_quiesced(
    coord: *mut FlashCoord,
    token: *const FlashToken,
    inactive: bool,
    now_ms: u32,
) -> FlashReply {
    token_message(coord, token, |c, t| c.quiesced(t, inactive, now_ms))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_flash_coord_parked(
    coord: *mut FlashCoord,
    token: *const FlashToken,
    party: FlashParty,
    now_ms: u32,
) -> FlashReply {
    token_message(coord, token, |c, t| c.parked(t, party, now_ms))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_flash_coord_complete(
    coord: *mut FlashCoord,
    token: *const FlashToken,
    party: FlashParty,
    now_ms: u32,
) -> FlashReply {
    token_message(coord, token, |c, t| c.complete(t, party, now_ms))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_flash_coord_released(
    coord: *mut FlashCoord,
    token: *const FlashToken,
    party: FlashParty,
    inactive: bool,
    now_ms: u32,
) -> FlashReply {
    token_message(coord, token, |c, t| c.released(t, party, inactive, now_ms))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_flash_coord_poll(coord: *mut FlashCoord, now_ms: u32) -> bool {
    unsafe { coord.as_mut() }.is_some_and(|c| c.poll(now_ms))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_flash_coord_may_access(
    coord: *mut FlashCoord,
    token: *const FlashToken,
    party: FlashParty,
    now_ms: u32,
) -> bool {
    match (unsafe { coord.as_mut() }, unsafe { token.as_ref() }) {
        (Some(c), Some(t)) => c.may_access(t, party, now_ms),
        _ => false,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_flash_coord_may_actuate(
    coord: *mut FlashCoord,
    now_ms: u32,
) -> bool {
    unsafe { coord.as_mut() }.is_some_and(|c| c.may_actuate(now_ms))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_flash_coord_outputs_blocked(coord: *const FlashCoord) -> bool {
    unsafe { coord.as_ref() }.is_none_or(FlashCoord::outputs_blocked)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_flash_coord_close(
    coord: *mut FlashCoord,
    now_ms: u32,
) -> FlashReply {
    unsafe { coord.as_mut() }.map_or(FlashReply::FAILED, |c| c.close(now_ms))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_flash_coord_closed(
    coord: *mut FlashCoord,
    token: *const FlashToken,
    peer_disabled: bool,
    inactive: bool,
    now_ms: u32,
) -> FlashReply {
    token_message(coord, token, |c, t| {
        c.closed(t, peer_disabled, inactive, now_ms)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_flash_coord_trip(coord: *mut FlashCoord, reason: FlashFault) {
    if let Some(c) = unsafe { coord.as_mut() } {
        c.trip(reason);
    }
}
