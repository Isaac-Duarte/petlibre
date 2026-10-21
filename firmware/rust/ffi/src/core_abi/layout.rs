//! Compile-time lock of the exported `#[repr(C)]` layouts. These values were
//! derived from the existing Rust layout; a failure means the C ABI changed.

use core::mem::{align_of, offset_of, size_of};

use wellfed_core::adc_scale::AdcCalibration;
use wellfed_core::feed_controller::{
    FeedController, FeedLimits, FeedReason, FeedRequest, FeedSample, FeedStartResult, FeedState,
    MotorCommand,
};
use wellfed_core::feed_journal::{FeedJournal, JournalConfig, JournalError, JournalView};
use wellfed_core::feed_progress::{BowlMode, Direction, FeedProgress, ProgressResult};
use wellfed_core::feed_storage::{OwnerOutcome, OwnerRecovery, OwnerResult, OwnerStorage};
use wellfed_core::feedback_events::{
    FeedbackDrive, FeedbackEvent, FeedbackEvents, FeedbackFault, FeedbackTakeResult,
};
use wellfed_core::flash_coord::{
    FlashCoord, FlashFault, FlashLimits, FlashParty, FlashPhase, FlashReply, FlashToken,
};
use wellfed_core::motor_gate::{MotorGate, MotorGateFault};
use wellfed_core::sensor_inputs::{SensorFilter, SensorInputs, SensorLimits, SensorReading};
use wellfed_core::storage::JournalIo;
use wellfed_core::tick_clock::TickClock;

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
