//! C ABI for `petlibre-core`, matching the original C firmware headers.
//!
//! Enumeration ABI: every enumeration-typed field, argument and return value
//! (`MotorCommand`, `FeedState`, `FeedReason`, `BowlMode`, `Direction`,
//! `FlashPhase`, ...) is a 32-bit unsigned integer (`#[repr(transparent)]`
//! `u32`). C declarations MUST use `uint32_t` plus `#define`/`enum` constants
//! for the values, never an `enum` type: the pinned AmebaD GCC does not pass
//! `-fno-short-enums`, so a C `enum` would be one byte and silently change
//! struct layouts (for example `MotorGate` would shrink from 48 bytes). The
//! SDK's flags are fixed by its prebuilt libraries and are not changed here.
//! The `layout` assertions at the end of this file lock every exported
//! `#[repr(C)]` struct's size, alignment and enum-bearing field offsets for
//! both 32-bit firmware and 64-bit host builds.

use core::ffi::c_void;

use petlibre_core::adc_scale::{AdcCalibration, SAMPLES};
use petlibre_core::feed_controller::{
    FeedController, FeedLimits, FeedReason, FeedRequest, FeedSample, FeedStartResult, MotorCommand,
};
use petlibre_core::feed_journal::{
    FeedJournal, JournalConfig, JournalError, JournalIo, JournalView,
};
use petlibre_core::feed_progress::{BowlMode, Direction, FeedProgress, ProgressResult};
use petlibre_core::feed_storage::OwnerStorage;
use petlibre_core::feedback_events::{
    FeedbackDrive, FeedbackEvent, FeedbackEvents, FeedbackFault, FeedbackTakeResult,
};
use petlibre_core::flash_coord::{
    FlashCoord, FlashFault, FlashLimits, FlashParty, FlashReply, FlashToken,
};
use petlibre_core::motor_gate::{MotorGate, MotorWritePair};
use petlibre_core::sensor_inputs::{SensorInputs, SensorLimits, SensorReading};
use petlibre_core::tick_clock::TickClock;

/// Store `value` through `out` when both are present.
fn store<T>(out: Option<&mut T>, value: Option<T>) -> bool {
    match (out, value) {
        (Some(out), Some(value)) => {
            *out = value;
            true
        }
        _ => false,
    }
}

// adc_scale.h

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_adc_calibration_valid(
    calibration: *const AdcCalibration,
) -> bool {
    unsafe { calibration.as_ref() }.is_some_and(AdcCalibration::valid)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_adc_calibration_decode(
    efuse: *const [u8; 4],
    calibration: *mut AdcCalibration,
) -> bool {
    match unsafe { efuse.as_ref() } {
        Some(efuse) if !calibration.is_null() => store(
            unsafe { calibration.as_mut() },
            AdcCalibration::decode(efuse),
        ),
        _ => false,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_adc_scale_16(
    calibration: *const AdcCalibration,
    samples: *const [u16; SAMPLES],
    value: *mut u32,
) -> bool {
    match (unsafe { calibration.as_ref() }, unsafe { samples.as_ref() }) {
        (Some(calibration), Some(samples)) if !value.is_null() => {
            store(unsafe { value.as_mut() }, calibration.scale_16(samples))
        }
        _ => false,
    }
}

// feed_progress.h

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_feed_progress_init(
    progress: *mut FeedProgress,
    mode: BowlMode,
    direction: Direction,
    portions: u16,
    credit_next_event: bool,
) -> bool {
    store(
        unsafe { progress.as_mut() },
        FeedProgress::new(mode, direction, portions, credit_next_event),
    )
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_feed_progress_accept(
    progress: *mut FeedProgress,
) -> ProgressResult {
    unsafe { &mut *progress }.accept()
}

// tick_clock.h

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_tick_clock_init(
    clock: *mut TickClock,
    ticks_per_second: u32,
    raw: u32,
    initial_ms: u32,
) -> bool {
    store(
        unsafe { clock.as_mut() },
        TickClock::new(ticks_per_second, raw, initial_ms),
    )
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_tick_clock_read(
    clock: *mut TickClock,
    raw: u32,
    now_ms: *mut u32,
) -> bool {
    let Some(clock) = (unsafe { clock.as_mut() }) else {
        return false;
    };
    match unsafe { now_ms.as_mut() } {
        Some(now) => store(Some(now), clock.read(raw)),
        None => {
            if clock.valid {
                clock.invalidate();
            }
            false
        }
    }
}

// feed_controller.h

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

// motor_gate.h

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_motor_gate_init(
    gate: *mut MotorGate,
    write_pair: Option<MotorWritePair>,
    context: *mut c_void,
    deadtime_ms: u32,
    lease_ms: u32,
    now_ms: u32,
) -> bool {
    let Some(gate) = (unsafe { gate.as_mut() }) else {
        return false;
    };
    // SAFETY: the C contract keeps the callback valid for `context`.
    unsafe { gate.init(write_pair, context, deadtime_ms, lease_ms, now_ms) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_motor_gate_request(
    gate: *mut MotorGate,
    command: MotorCommand,
    now_ms: u32,
) -> bool {
    unsafe { &mut *gate }.request(command, now_ms)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_motor_gate_tick(gate: *mut MotorGate, now_ms: u32) -> bool {
    unsafe { &mut *gate }.tick(now_ms)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_motor_gate_trip(gate: *mut MotorGate, now_ms: u32) {
    unsafe { &mut *gate }.trip(now_ms);
}

// sensor_inputs.h

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_sensor_inputs_init(
    inputs: *mut SensorInputs,
    limits: *const SensorLimits,
    calibration: *const AdcCalibration,
) -> bool {
    match (unsafe { limits.as_ref() }, unsafe { calibration.as_ref() }) {
        (Some(limits), Some(calibration)) => store(
            unsafe { inputs.as_mut() },
            SensorInputs::new(limits, calibration),
        ),
        _ => false,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_sensor_inputs_update(
    inputs: *mut SensorInputs,
    now_ms: u32,
    reading: *const SensorReading,
) {
    unsafe { &mut *inputs }.update(now_ms, unsafe { reading.as_ref() });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_sensor_inputs_sample(
    inputs: *const SensorInputs,
    now_ms: u32,
    feedback_edge: bool,
    feedback_high: bool,
    sample: *mut FeedSample,
) {
    unsafe { *sample = (*inputs).sample(now_ms, feedback_edge, feedback_high) };
}

// feedback_events.h

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_feedback_events_init(
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
pub unsafe extern "C" fn petlibre_feedback_events_push(
    events: *mut FeedbackEvents,
    captured_at_ms: u32,
    high: bool,
) -> bool {
    unsafe { events.as_mut() }.is_some_and(|e| e.push(captured_at_ms, high))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_feedback_events_take(
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
pub unsafe extern "C" fn petlibre_feedback_events_process(
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

// feed_journal.h

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_feed_journal_open(
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
pub unsafe extern "C" fn petlibre_feed_journal_create(
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
pub unsafe extern "C" fn petlibre_feed_journal_read(
    journal: *const FeedJournal,
    view: *mut JournalView,
) -> bool {
    match unsafe { journal.as_ref() } {
        Some(journal) if !view.is_null() => store(unsafe { view.as_mut() }, journal.read()),
        _ => false,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_feed_journal_storage(journal: *mut FeedJournal) -> OwnerStorage {
    match unsafe { journal.as_mut() } {
        Some(journal) => journal.storage(),
        None => OwnerStorage {
            context: core::ptr::null_mut(),
            ..FeedJournal::default().storage()
        },
    }
}

// flash_coord.h

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_flash_coord_init(
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
pub unsafe extern "C" fn petlibre_flash_coord_ready(
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
pub unsafe extern "C" fn petlibre_flash_coord_request(
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
pub unsafe extern "C" fn petlibre_flash_coord_quiesced(
    coord: *mut FlashCoord,
    token: *const FlashToken,
    inactive: bool,
    now_ms: u32,
) -> FlashReply {
    token_message(coord, token, |c, t| c.quiesced(t, inactive, now_ms))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_flash_coord_parked(
    coord: *mut FlashCoord,
    token: *const FlashToken,
    party: FlashParty,
    now_ms: u32,
) -> FlashReply {
    token_message(coord, token, |c, t| c.parked(t, party, now_ms))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_flash_coord_complete(
    coord: *mut FlashCoord,
    token: *const FlashToken,
    party: FlashParty,
    now_ms: u32,
) -> FlashReply {
    token_message(coord, token, |c, t| c.complete(t, party, now_ms))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_flash_coord_released(
    coord: *mut FlashCoord,
    token: *const FlashToken,
    party: FlashParty,
    inactive: bool,
    now_ms: u32,
) -> FlashReply {
    token_message(coord, token, |c, t| c.released(t, party, inactive, now_ms))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_flash_coord_poll(coord: *mut FlashCoord, now_ms: u32) -> bool {
    unsafe { coord.as_mut() }.is_some_and(|c| c.poll(now_ms))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_flash_coord_may_access(
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
pub unsafe extern "C" fn petlibre_flash_coord_may_actuate(
    coord: *mut FlashCoord,
    now_ms: u32,
) -> bool {
    unsafe { coord.as_mut() }.is_some_and(|c| c.may_actuate(now_ms))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_flash_coord_outputs_blocked(coord: *const FlashCoord) -> bool {
    unsafe { coord.as_ref() }.is_none_or(FlashCoord::outputs_blocked)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_flash_coord_close(
    coord: *mut FlashCoord,
    now_ms: u32,
) -> FlashReply {
    unsafe { coord.as_mut() }.map_or(FlashReply::FAILED, |c| c.close(now_ms))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_flash_coord_closed(
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
pub unsafe extern "C" fn petlibre_flash_coord_trip(coord: *mut FlashCoord, reason: FlashFault) {
    if let Some(c) = unsafe { coord.as_mut() } {
        c.trip(reason);
    }
}

// app/diagnostic.h

/// Synthetic observations and memory-only outputs; never accesses board I/O.
#[unsafe(no_mangle)]
pub extern "C" fn petlibre_diagnostic_run() -> bool {
    petlibre_core::diagnostic::run()
}

/// Compile-time lock of the exported `#[repr(C)]` layouts. These values were
/// derived from the existing Rust layout; a failure means the C ABI changed.
#[allow(dead_code)]
mod layout {
    use core::mem::{align_of, offset_of, size_of};

    use petlibre_core::adc_scale::AdcCalibration;
    use petlibre_core::feed_controller::{
        FeedController, FeedLimits, FeedReason, FeedRequest, FeedSample, FeedStartResult,
        FeedState, MotorCommand,
    };
    use petlibre_core::feed_journal::{
        FeedJournal, JournalConfig, JournalError, JournalIo, JournalView,
    };
    use petlibre_core::feed_progress::{BowlMode, Direction, FeedProgress, ProgressResult};
    use petlibre_core::feed_storage::{OwnerOutcome, OwnerRecovery, OwnerResult, OwnerStorage};
    use petlibre_core::feedback_events::{
        FeedbackDrive, FeedbackEvent, FeedbackEvents, FeedbackFault, FeedbackTakeResult,
    };
    use petlibre_core::flash_coord::{
        FlashCoord, FlashFault, FlashLimits, FlashParty, FlashPhase, FlashReply, FlashToken,
    };
    use petlibre_core::motor_gate::{MotorGate, MotorGateFault};
    use petlibre_core::sensor_inputs::{SensorFilter, SensorInputs, SensorLimits, SensorReading};
    use petlibre_core::tick_clock::TickClock;

    /// Size and alignment, then `field = offset` pairs.
    macro_rules! lock {
        ($t:ty, $size:expr, $align:expr $(, $field:ident = $offset:expr)* $(,)?) => {
            const _: () = assert!(size_of::<$t>() == $size && align_of::<$t>() == $align);
            $(const _: () = assert!(offset_of!($t, $field) == $offset);)*
        };
    }

    /// Every C enumeration is a 4-byte, 4-aligned `uint32_t`.
    macro_rules! enum32 {
        ($($t:ty),+ $(,)?) => {
            $(const _: () = assert!(size_of::<$t>() == 4 && align_of::<$t>() == 4);)+
        };
    }

    enum32!(
        MotorCommand,
        FeedState,
        FeedReason,
        FeedStartResult,
        BowlMode,
        Direction,
        ProgressResult,
        JournalError,
        OwnerOutcome,
        FeedbackFault,
        FeedbackTakeResult,
        FlashParty,
        FlashPhase,
        FlashFault,
        FlashReply,
        MotorGateFault,
    );

    lock!(AdcCalibration, 6, 2);
    lock!(TickClock, 20, 4);
    lock!(FeedLimits, 32, 4);
    lock!(FeedSample, 12, 4);
    lock!(FeedProgress, 16, 4, mode = 4, direction = 8);
    lock!(FeedRequest, 24, 8, mode = 12, direction = 16);
    lock!(
        FeedController,
        96,
        8,
        progress = 32,
        state = 80,
        reason = 84,
        motor = 88,
    );
    lock!(SensorLimits, 28, 4);
    lock!(SensorReading, 72, 2);
    lock!(SensorFilter, 8, 4);
    lock!(SensorInputs, 84, 4);
    lock!(FeedbackEvent, 8, 4);
    lock!(FeedbackDrive, 12, 4, applied = 4, applied_at = 8);
    lock!(FeedbackEvents, 272, 4, fault = 268);
    lock!(JournalConfig, 16, 8, mode = 8, max_portions = 12);
    lock!(OwnerRecovery, 16, 8, direction = 8);
    lock!(
        OwnerResult,
        24,
        8,
        outcome = 12,
        reason = 16,
        motion_possible = 20
    );
    lock!(JournalView, 56, 8, recovery = 0, result = 16, sequence = 40);
    lock!(FlashLimits, 20, 4);
    lock!(FlashToken, 32, 8, owner = 24);
    lock!(
        FlashCoord,
        96,
        8,
        token = 24,
        phase = 84,
        fault = 88,
        initialized = 92,
    );

    // Pointer-bearing structs differ between the 32-bit firmware targets and
    // 64-bit hosts; both are locked.
    #[cfg(target_pointer_width = "32")]
    mod pointers {
        use super::*;
        lock!(
            MotorGate,
            48,
            4,
            write_pair = 0,
            context = 4,
            desired = 32,
            applied = 36,
            fault = 40,
            applied_known = 44,
        );
        lock!(JournalIo, 20, 4);
        lock!(OwnerStorage, 16, 4);
        lock!(
            FeedJournal,
            112,
            8,
            io = 0,
            config = 24,
            view = 40,
            error = 96,
            slot = 100,
        );
    }

    #[cfg(target_pointer_width = "64")]
    mod pointers {
        use super::*;
        lock!(
            MotorGate,
            56,
            8,
            write_pair = 0,
            context = 8,
            desired = 40,
            applied = 44,
            fault = 48,
            applied_known = 52,
        );
        lock!(JournalIo, 40, 8);
        lock!(OwnerStorage, 32, 8);
        lock!(
            FeedJournal,
            128,
            8,
            io = 0,
            config = 40,
            view = 56,
            error = 112,
            slot = 116,
        );
    }
}
