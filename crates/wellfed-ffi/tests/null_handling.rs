//! The C headers permit null pointers in specific places; these cases keep the
//! original rejections that native Rust signatures cannot express.

use core::ptr::{null, null_mut};

use wellfed_core::adc_scale::AdcCalibration;
use wellfed_core::feed_controller::{
    FeedController, FeedLimits, FeedReason, FeedRequest, FeedSample, FeedStartResult,
};
use wellfed_core::feed_journal::{FeedJournal, JournalConfig, JournalError, JournalView};
use wellfed_core::feed_progress::{BowlMode, Direction, FeedProgress};
use wellfed_core::feedback_events::{
    FeedbackDrive, FeedbackEvent, FeedbackEvents, FeedbackFault, FeedbackTakeResult,
};
use wellfed_core::flash_coord::{FlashCoord, FlashFault, FlashLimits, FlashReply};
use wellfed_core::tick_clock::TickClock;
use wellfed_ffi::core_abi::*;
use wellfed_testkit::journal_flash::FakeJournalFlash;

#[test]
fn constructors_reject_null_destinations() {
    // SAFETY: each pointer is null or a live local, as the C contract allows.
    unsafe {
        assert!(!wellfed_feed_progress_init(
            null_mut(),
            BowlMode::SINGLE,
            Direction::D0,
            1,
            true
        ));
        assert!(!wellfed_tick_clock_init(null_mut(), 32_768, 0, 0));

        assert!(!wellfed_adc_calibration_valid(null()));
        let mut calibration = AdcCalibration::default();
        assert!(!wellfed_adc_calibration_decode(null(), &mut calibration));

        assert!(!wellfed_feed_controller_init(null_mut(), null(), 0));
        assert!(!wellfed_flash_coord_init(
            null_mut(),
            &FlashLimits::default(),
            1,
            0
        ));
    }
}

#[test]
fn invalid_progress_leaves_destination_unchanged() {
    let mut progress = FeedProgress::new(BowlMode::SINGLE, Direction::D0, 1, true).unwrap();
    let before = progress;
    // SAFETY: each pointer is null or a live local, as the C contract allows.
    unsafe {
        assert!(!wellfed_feed_progress_init(
            &mut progress,
            BowlMode(2),
            Direction::D0,
            1,
            true
        ));
    }

    assert_eq!(progress, before);
}

#[test]
fn tick_clock_null_output_invalidates() {
    let mut clock = TickClock::new(u32::MAX, 0, 0).unwrap();
    let mut now = 0;
    // SAFETY: each pointer is null or a live local, as the C contract allows.
    unsafe {
        assert!(!wellfed_tick_clock_read(null_mut(), 1, null_mut()));
        assert!(wellfed_tick_clock_read(&mut clock, 0x7fff_ffff, &mut now));
        assert_eq!(now, 499);
        assert!(!wellfed_tick_clock_read(
            &mut clock,
            0x7fff_ffff,
            null_mut()
        ));
    }

    assert!(!clock.valid);
}

#[test]
fn controller_rejects_null_arguments() {
    let mut controller = FeedController::default();
    // SAFETY: each pointer is null or a live local, as the C contract allows.
    unsafe {
        assert_eq!(
            wellfed_feed_controller_start(&mut controller, null(), 0, &FeedSample::default()),
            FeedStartResult::REJECT_REQUEST
        );
        assert_eq!(
            wellfed_feed_controller_start(null_mut(), &FeedRequest::default(), 0, null()),
            FeedStartResult::REJECT_REQUEST
        );

        assert!(!wellfed_feed_controller_trip(
            null_mut(),
            FeedReason::OUTPUT
        ));

        assert!(!wellfed_feed_controller_init(&mut controller, null(), 0));
        assert!(!wellfed_feed_controller_init(
            &mut controller,
            &FeedLimits::default(),
            0
        ));
    }
}

#[test]
fn feedback_take_without_output_latches_argument() {
    let mut q = FeedbackEvents::new(1, 2, 0).unwrap();
    // SAFETY: each pointer is null or a live local, as the C contract allows.
    unsafe {
        assert_eq!(
            wellfed_feedback_events_take(&mut q, 0, null_mut()),
            FeedbackTakeResult::FAILED
        );
        assert_eq!(
            wellfed_feedback_events_take(null_mut(), 0, &mut FeedbackEvent::default()),
            FeedbackTakeResult::FAILED
        );
        assert!(!wellfed_feedback_events_push(null_mut(), 0, true));
    }

    assert_eq!(q.fault, FeedbackFault::ARGUMENT);
}

#[test]
fn feedback_process_with_missing_sample_fails_both_sides() {
    let mut q = FeedbackEvents::new(1, 2, 0).unwrap();
    let mut controller = FeedController::default();
    // SAFETY: each pointer is null or a live local, as the C contract allows.
    unsafe {
        assert!(!wellfed_feedback_events_process(
            &mut q,
            &mut controller,
            0,
            null(),
            FeedbackDrive::default()
        ));
        assert!(!wellfed_feedback_events_process(
            null_mut(),
            null_mut(),
            0,
            null(),
            FeedbackDrive::default()
        ));
    }

    assert_eq!(q.fault, FeedbackFault::ARGUMENT);
}

#[test]
fn journal_rejects_null_arguments() {
    let mut f = FakeJournalFlash::default();
    let io = f.io();
    let config = JournalConfig {
        binding: 1,
        mode: BowlMode::SINGLE,
        max_portions: 3,
    };
    let mut j = FeedJournal::default();

    // SAFETY: each pointer is null or a live local, as the C contract allows.
    unsafe {
        assert_eq!(
            wellfed_feed_journal_open(null_mut(), &io, &config),
            JournalError::ARGUMENT
        );
        assert_eq!(
            wellfed_feed_journal_open(&mut j, null(), &config),
            JournalError::ARGUMENT
        );
        assert_eq!(
            wellfed_feed_journal_create(&mut j, &io, &config, Direction::D0, true, true),
            JournalError::OK
        );

        assert!(!wellfed_feed_journal_read(&j, null_mut()));
        assert!(wellfed_feed_journal_read(&j, &mut JournalView::default()));

        let storage = wellfed_feed_journal_storage(&mut j);
        assert!(!storage.reserve.unwrap()(storage.context, null()));
        assert!(!storage.recover.unwrap()(storage.context, null_mut()));
        assert!(!storage.finish.unwrap()(storage.context, null(), null()));

        // Reopen from the journal's own saved configuration.
        let saved_io = &raw const j.io;
        let saved_config = &raw const j.config;
        assert_eq!(
            wellfed_feed_journal_open(&mut j, saved_io, saved_config),
            JournalError::OK
        );
    }
}

#[test]
fn flash_coord_null_handling() {
    let mut c = FlashCoord::default();
    // SAFETY: each pointer is null or a live local, as the C contract allows.
    unsafe {
        assert_eq!(
            wellfed_flash_coord_ready(null_mut(), 1, 1, true, 0),
            FlashReply::INVALID
        );
        assert_eq!(
            wellfed_flash_coord_quiesced(&mut c, null(), true, 0),
            FlashReply::INVALID
        );
        assert_eq!(
            wellfed_flash_coord_close(null_mut(), 0),
            FlashReply::FAILED
        );

        assert!(wellfed_flash_coord_outputs_blocked(null()));
        assert!(!wellfed_flash_coord_may_access(
            &mut c,
            null(),
            Default::default(),
            0
        ));
        wellfed_flash_coord_trip(null_mut(), FlashFault::ORDER);
    }
}
