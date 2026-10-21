//! C ABI for `petlibro-core`, matching the original C firmware headers.

use core::ffi::c_void;

use petlibro_core::adc_scale::{AdcCalibration, SAMPLES};
use petlibro_core::feed_controller::{
    FeedController, FeedLimits, FeedReason, FeedRequest, FeedSample, FeedStartResult, MotorCommand,
};
use petlibro_core::feed_journal::{
    FeedJournal, JournalConfig, JournalError, JournalIo, JournalView,
};
use petlibro_core::feed_progress::{BowlMode, Direction, FeedProgress, ProgressResult};
use petlibro_core::feed_storage::OwnerStorage;
use petlibro_core::feedback_events::{
    FeedbackDrive, FeedbackEvent, FeedbackEvents, FeedbackFault, FeedbackTakeResult,
};
use petlibro_core::flash_coord::{
    FlashCoord, FlashFault, FlashLimits, FlashParty, FlashReply, FlashToken,
};
use petlibro_core::motor_gate::{MotorGate, MotorWritePair};
use petlibro_core::sensor_inputs::{SensorInputs, SensorLimits, SensorReading};
use petlibro_core::tick_clock::TickClock;

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
pub unsafe extern "C" fn petlibro_adc_calibration_valid(
    calibration: *const AdcCalibration,
) -> bool {
    unsafe { calibration.as_ref() }.is_some_and(AdcCalibration::valid)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibro_adc_calibration_decode(
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
pub unsafe extern "C" fn petlibro_adc_scale_16(
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
pub unsafe extern "C" fn petlibro_feed_progress_init(
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
pub unsafe extern "C" fn petlibro_feed_progress_accept(
    progress: *mut FeedProgress,
) -> ProgressResult {
    unsafe { &mut *progress }.accept()
}

// tick_clock.h

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibro_tick_clock_init(
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
pub unsafe extern "C" fn petlibro_tick_clock_read(
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
pub unsafe extern "C" fn petlibro_feed_controller_init(
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
pub unsafe extern "C" fn petlibro_feed_controller_start(
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
pub unsafe extern "C" fn petlibro_feed_controller_update(
    controller: *mut FeedController,
    now_ms: u32,
    sample: *const FeedSample,
) {
    if let Some(controller) = unsafe { controller.as_mut() } {
        controller.update(now_ms, unsafe { sample.as_ref() });
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibro_feed_controller_update_captured(
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
pub unsafe extern "C" fn petlibro_feed_controller_trip(
    controller: *mut FeedController,
    reason: FeedReason,
) -> bool {
    unsafe { controller.as_mut() }.is_some_and(|c| c.trip(reason))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibro_feed_controller_cancel(
    controller: *mut FeedController,
    id: u64,
) -> bool {
    unsafe { controller.as_mut() }.is_some_and(|c| c.cancel(id))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibro_feed_controller_reset(controller: *mut FeedController) -> bool {
    unsafe { controller.as_mut() }.is_some_and(FeedController::reset)
}

// motor_gate.h

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibro_motor_gate_init(
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
pub unsafe extern "C" fn petlibro_motor_gate_request(
    gate: *mut MotorGate,
    command: MotorCommand,
    now_ms: u32,
) -> bool {
    unsafe { &mut *gate }.request(command, now_ms)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibro_motor_gate_tick(gate: *mut MotorGate, now_ms: u32) -> bool {
    unsafe { &mut *gate }.tick(now_ms)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibro_motor_gate_trip(gate: *mut MotorGate, now_ms: u32) {
    unsafe { &mut *gate }.trip(now_ms);
}

// sensor_inputs.h

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibro_sensor_inputs_init(
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
pub unsafe extern "C" fn petlibro_sensor_inputs_update(
    inputs: *mut SensorInputs,
    now_ms: u32,
    reading: *const SensorReading,
) {
    unsafe { &mut *inputs }.update(now_ms, unsafe { reading.as_ref() });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibro_sensor_inputs_sample(
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
pub unsafe extern "C" fn petlibro_feedback_events_init(
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
pub unsafe extern "C" fn petlibro_feedback_events_push(
    events: *mut FeedbackEvents,
    captured_at_ms: u32,
    high: bool,
) -> bool {
    unsafe { events.as_mut() }.is_some_and(|e| e.push(captured_at_ms, high))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibro_feedback_events_take(
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
pub unsafe extern "C" fn petlibro_feedback_events_process(
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
pub unsafe extern "C" fn petlibro_feed_journal_open(
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
pub unsafe extern "C" fn petlibro_feed_journal_create(
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
pub unsafe extern "C" fn petlibro_feed_journal_read(
    journal: *const FeedJournal,
    view: *mut JournalView,
) -> bool {
    match unsafe { journal.as_ref() } {
        Some(journal) if !view.is_null() => store(unsafe { view.as_mut() }, journal.read()),
        _ => false,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibro_feed_journal_storage(journal: *mut FeedJournal) -> OwnerStorage {
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
pub unsafe extern "C" fn petlibro_flash_coord_init(
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
pub unsafe extern "C" fn petlibro_flash_coord_ready(
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
pub unsafe extern "C" fn petlibro_flash_coord_request(
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
pub unsafe extern "C" fn petlibro_flash_coord_quiesced(
    coord: *mut FlashCoord,
    token: *const FlashToken,
    inactive: bool,
    now_ms: u32,
) -> FlashReply {
    token_message(coord, token, |c, t| c.quiesced(t, inactive, now_ms))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibro_flash_coord_parked(
    coord: *mut FlashCoord,
    token: *const FlashToken,
    party: FlashParty,
    now_ms: u32,
) -> FlashReply {
    token_message(coord, token, |c, t| c.parked(t, party, now_ms))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibro_flash_coord_complete(
    coord: *mut FlashCoord,
    token: *const FlashToken,
    party: FlashParty,
    now_ms: u32,
) -> FlashReply {
    token_message(coord, token, |c, t| c.complete(t, party, now_ms))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibro_flash_coord_released(
    coord: *mut FlashCoord,
    token: *const FlashToken,
    party: FlashParty,
    inactive: bool,
    now_ms: u32,
) -> FlashReply {
    token_message(coord, token, |c, t| c.released(t, party, inactive, now_ms))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibro_flash_coord_poll(coord: *mut FlashCoord, now_ms: u32) -> bool {
    unsafe { coord.as_mut() }.is_some_and(|c| c.poll(now_ms))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibro_flash_coord_may_access(
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
pub unsafe extern "C" fn petlibro_flash_coord_may_actuate(
    coord: *mut FlashCoord,
    now_ms: u32,
) -> bool {
    unsafe { coord.as_mut() }.is_some_and(|c| c.may_actuate(now_ms))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibro_flash_coord_outputs_blocked(coord: *const FlashCoord) -> bool {
    unsafe { coord.as_ref() }.is_none_or(FlashCoord::outputs_blocked)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibro_flash_coord_close(
    coord: *mut FlashCoord,
    now_ms: u32,
) -> FlashReply {
    unsafe { coord.as_mut() }.map_or(FlashReply::FAILED, |c| c.close(now_ms))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibro_flash_coord_closed(
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
pub unsafe extern "C" fn petlibro_flash_coord_trip(coord: *mut FlashCoord, reason: FlashFault) {
    if let Some(c) = unsafe { coord.as_mut() } {
        c.trip(reason);
    }
}

// app/diagnostic.h

/// Synthetic observations and memory-only outputs; never accesses board I/O.
#[unsafe(no_mangle)]
pub extern "C" fn petlibro_diagnostic_run() -> bool {
    petlibro_core::diagnostic::run()
}
