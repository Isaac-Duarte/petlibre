mod common;

use common::*;
use wellfed_amebad::board::{Board, MOTOR_MASK};
use wellfed_amebad::events::{Events, EventsConfig, EventsFault};
use wellfed_amebad::sdk::fake::{self, GPIOA_IRQ, TIMER2_IRQ};
use wellfed_amebad::sdk::{self, Irq, TIM_CR_CNT_STS};
use wellfed_core::feed_controller::{
    FeedController, FeedLimits, FeedReason, FeedRequest, FeedSample, FeedStartResult, FeedState,
    MotorCommand,
};
use wellfed_core::feed_progress::{BowlMode, Direction};
use wellfed_core::feedback_events::FeedbackFault;
use wellfed_core::flash_coord::FlashCoord;
use wellfed_core::motor_gate::MotorGateFault;

const CONFIG: EventsConfig = EventsConfig {
    ticks_per_second: 32_000,
    timer_period_ms: 2,
    max_feedback_age_ms: 2,
    feedback_capacity: 3,
};
const HEALTHY: FeedSample = FeedSample {
    power_healthy: true,
    optical_valid: true,
    outlet_clear: true,
    current_valid: true,
    current_ma: 100,
    feedback_edge: false,
    feedback_high: false,
};
const LIMITS: FeedLimits = FeedLimits {
    feedback_inhibit_ms: 5,
    feedback_timeout_ms: 30,
    direction_pause_ms: 3,
    reverse_ms: 3,
    max_duration_ms: 120,
    max_observation_gap_ms: 4,
    current_limit_ma: 300,
    max_portions: 1,
    max_reverse_attempts: 2,
};

#[derive(Default)]
struct Fixture {
    board: Board,
    events: Events,
    authority: Option<Box<FlashCoord>>,
}

impl Fixture {
    fn open_board(&mut self) {
        assert!(self.board.open(2, 12, 8, 0));
        self.authority = Some(flash_attach(&mut self.board));
    }

    fn open_events(&mut self, config: &EventsConfig) -> bool {
        let board = &raw mut self.board;
        unsafe { self.events.open(board, config) }
    }

    fn arm(&mut self) {
        assert!(!self.events.capture(true));
        fake::advance(64, true);
        assert!(self.events.timer_seen && self.events.timer_at == 2);
        let mut idle = FeedController::new(&LIMITS, 0).unwrap();
        assert!(self.events.process(&mut idle, Some(&HEALTHY)));
        assert!(self.events.capture(true));
    }

    fn drive(&mut self) {
        self.arm();
        assert!(self.events.optics(true, 120));
        assert!(self.events.motor(MotorCommand::DRIVE_0));
        assert_eq!(motor_pins(), 1 << 13);
    }

    fn finish(&mut self) {
        assert!(self.events.close());
        assert!(!self.events.opened && sdk::tim2_en() == 0 && sdk::tim2_dier() == 0);
        assert!(hw(
            |h| h.irq[TIMER2_IRQ].is_none() && h.irq[GPIOA_IRQ].is_none()
        ));
        assert!(sdk::gpioa_int_en() == 0 && hw(|h| h.interrupts) == 0);
        self.board.close();
    }
}

fn begin() -> Box<Fixture> {
    fake::reset_hardware();
    let mut f = Box::<Fixture>::default();
    f.open_board();
    assert!(f.open_events(&CONFIG));
    f
}

#[test]
fn timer_stops_an_owner_that_never_runs() {
    let mut f = begin();
    f.drive();
    fake::advance(12 * 32, true); // Only the independently registered ISR runs.
    assert!(f.events.fault == EventsFault::OUTPUT && f.board.faulted);
    assert_eq!(f.board.motor.fault, MotorGateFault::EXPIRED);
    assert!(motor_pins() == MOTOR_MASK && f.board.motor.applied_known);
    assert!(optics_off());
    assert!(!f.events.motor(MotorCommand::DRIVE_1));
    f.finish();
}

#[test]
fn interrupt_overflow_stops_without_owner_processing() {
    let mut f = begin();
    f.drive();
    fake::advance(32, true);
    fake::edge(true);
    fake::edge(false);
    fake::edge(true);
    assert_eq!(f.events.feedback.count, 3);
    fake::edge(false);
    assert_eq!(f.events.fault, EventsFault::FEEDBACK);
    assert_eq!(f.events.feedback.fault, FeedbackFault::OVERFLOW);
    assert!(!f.events.capture_enabled && motor_pins() == MOTOR_MASK);
    assert!(optics_off());
    f.finish();
}

#[test]
fn missed_timer_heartbeat_blocks_renewal() {
    let mut f = begin();
    f.drive();
    fake::advance(6 * 32, false);
    assert!(!f.events.motor(MotorCommand::DRIVE_0));
    assert!(f.events.fault == EventsFault::TIMER && motor_pins() == MOTOR_MASK);
    f.finish();
}

#[test]
fn clock_failures_stop_in_irq_or_task_context() {
    let mut f = begin();
    f.drive();
    let timer_at = f.events.timer_at;
    // One interrupt without SYSTIMER advance is a coalesced pending update:
    // skipped without advancing, while the drive stays supervised.
    fake::irq(TIMER2_IRQ);
    assert!(f.events.fault == EventsFault::OK && f.events.timer_at == timer_at);
    assert_eq!(motor_pins(), 1 << 13);
    // A second consecutive one is a frozen reference clock.
    fake::irq(TIMER2_IRQ);
    assert!(f.events.fault == EventsFault::CLOCK && motor_pins() == MOTOR_MASK);
    f.finish();
    // An advancing reading between two repeats resets the tolerance.
    let mut f = begin();
    f.drive();
    for _ in 0..3 {
        fake::irq(TIMER2_IRQ);
        fake::advance(32, true);
    }
    assert!(f.events.fault == EventsFault::OK && motor_pins() == 1 << 13);
    assert!(f.events.motor(MotorCommand::DRIVE_0));
    f.finish();
    let mut f = begin();
    f.drive();
    hw(|h| h.raw_ticks -= 1);
    assert_eq!(f.events.now_ms(), None);
    assert_eq!(f.events.fault, EventsFault::CLOCK);
    assert_eq!(motor_pins(), MOTOR_MASK);
    f.finish();
}

#[test]
fn claims_and_failed_registration_leave_no_live_resources() {
    fake::reset_hardware();
    let mut f = Box::<Fixture>::default();
    f.open_board();
    hw(|h| h.gpio.int_en = 1);
    assert!(!f.open_events(&CONFIG));
    hw(|h| h.gpio.int_en = 0);
    hw(|h| h.timer.en = TIM_CR_CNT_STS);
    assert!(!f.open_events(&CONFIG));
    hw(|h| h.timer.en = 0);
    assert!(!f.open_events(&EventsConfig {
        timer_period_ms: 4,
        ..CONFIG
    }));
    assert!(!f.open_events(&EventsConfig {
        feedback_capacity: 33,
        ..CONFIG
    }));
    for which in 0..2 {
        hw(|h| {
            h.fail_timer_register = which == 0;
            h.fail_gpio_register = which == 1;
        });
        assert!(!f.open_events(&CONFIG));
        assert!(f.events.fault == EventsFault::SETUP && !f.events.opened);
        assert!(sdk::tim2_en() == 0 && sdk::tim2_dier() == 0 && sdk::gpioa_int_en() == 0);
        assert!(hw(
            |h| h.irq[TIMER2_IRQ].is_none() && h.irq[GPIOA_IRQ].is_none()
        ));
    }
    hw(|h| h.fail_gpio_register = false);
    assert!(f.open_events(&CONFIG));
    let mut another = Box::<Events>::default();
    assert!(!unsafe { another.open(&raw mut f.board, &CONFIG) });
    f.finish();
}

fn single_request() -> FeedRequest {
    FeedRequest {
        id: 1,
        portions: 1,
        mode: BowlMode::SINGLE,
        direction: Direction::D0,
        ..FeedRequest::default()
    }
}

#[test]
fn owner_consumes_captured_feedback_and_applies_stop() {
    let mut f = begin();
    f.arm();
    let mut controller = FeedController::new(&LIMITS, 0).unwrap();
    assert_eq!(
        controller.start(&single_request(), 2, &HEALTHY),
        FeedStartResult::ACCEPTED
    );
    assert!(f.events.motor(controller.motor));
    for ms in 3..=7 {
        fake::advance(32, true);
        if ms == 7 {
            fake::edge(true);
        }
        assert!(f.events.process(&mut controller, Some(&HEALTHY)));
        assert!(f.events.motor(controller.motor));
    }
    assert!(controller.state == FeedState::COMPLETE && controller.progress.completed == 1);
    assert!(controller.last_feedback_at == 7 && f.events.feedback.count == 0);
    assert_eq!(motor_pins(), MOTOR_MASK);
    assert!(f.events.capture(false));
    f.finish();
}

#[test]
fn stale_feedback_fails_the_real_port_before_credit() {
    let mut f = begin();
    f.arm();
    let mut controller = FeedController::new(&LIMITS, 0).unwrap();
    assert_eq!(
        controller.start(&single_request(), 2, &HEALTHY),
        FeedStartResult::ACCEPTED
    );
    assert!(f.events.motor(controller.motor));
    fake::advance(32, true);
    fake::edge(true);
    fake::advance(3 * 32, true);
    assert!(!f.events.process(&mut controller, Some(&HEALTHY)));
    assert!(controller.state == FeedState::FAULT && controller.progress.completed == 0);
    assert_eq!(controller.reason, FeedReason::FEEDBACK_STREAM);
    assert_eq!(motor_pins(), MOTOR_MASK);
    f.finish();
}

#[test]
fn an_unverified_stop_retains_the_timer_for_retry() {
    let mut f = begin();
    f.drive();
    hw(|h| h.blocked_motor_write = true);
    assert!(!f.events.close());
    assert!(f.events.opened && !f.board.motor.applied_known && !f.events.capture_enabled);
    assert!(sdk::tim2_en() != 0 && hw(|h| h.irq_enabled[TIMER2_IRQ]));
    hw(|h| h.blocked_motor_write = false);
    fake::advance(64, true);
    assert!(f.board.motor.applied_known && motor_pins() == MOTOR_MASK);
    f.finish();
}

#[test]
fn ownership_and_timer_configuration_changes_trip() {
    let mut f = begin();
    f.drive();
    hw(|h| h.gpio.int_status = 1 << 4);
    fake::irq(GPIOA_IRQ);
    assert!(f.events.fault == EventsFault::RESOURCE && !hw(|h| h.irq_enabled[GPIOA_IRQ]));
    assert_eq!(motor_pins(), MOTOR_MASK);
    f.finish();
    for which in 0..3 {
        let mut f = begin();
        f.drive();
        hw(|h| match which {
            0 => h.timer.arr += 1,
            1 => h.timer.en = 0,
            _ => h.timer.dier = 0,
        });
        assert!(!f.events.motor(MotorCommand::DRIVE_0));
        assert!(f.events.fault == EventsFault::TIMER && motor_pins() == MOTOR_MASK);
        f.finish();
    }
}

#[test]
fn first_timer_is_required_and_raw_wrap_is_supported() {
    let mut f = begin();
    assert!(!f.events.motor(MotorCommand::DRIVE_0));
    assert!(f.events.fault == EventsFault::TIMER && motor_pins() == MOTOR_MASK);
    f.finish();
    fake::reset_hardware();
    hw(|h| h.raw_ticks = u32::MAX - 63);
    let mut f = Box::<Fixture>::default();
    f.open_board();
    assert!(f.open_events(&CONFIG));
    f.arm();
    assert_eq!(hw(|h| h.raw_ticks), 0);
    assert!(f.events.motor(MotorCommand::DRIVE_0));
    f.finish();
}

#[test]
fn stopped_warmup_cannot_outlive_its_owner() {
    let mut f = begin();
    f.arm();
    assert!(f.events.optics(true, 120));
    fake::advance(12 * 32, true);
    assert_eq!(f.events.fault, EventsFault::OWNER);
    assert!(optics_off());
    assert_eq!(motor_pins(), MOTOR_MASK);
    f.finish();
    let mut f = begin();
    f.arm();
    assert!(f.events.optics(true, 120));
    fake::advance(12 * 32, false);
    let mut idle = FeedController::new(&LIMITS, 0).unwrap();
    assert!(!f.events.process(&mut idle, Some(&HEALTHY)));
    assert!(f.events.fault == EventsFault::OWNER && !f.board.optics_enabled);
    f.finish();
}

#[test]
fn infrared_budget_cannot_be_extended_by_renewal() {
    let mut f = begin();
    f.arm();
    let mut idle = FeedController::new(&LIMITS, 0).unwrap();
    assert!(f.events.optics(true, 6));
    for _ in 3..=7 {
        fake::advance(32, true);
        assert!(f.events.process(&mut idle, Some(&HEALTHY)));
        assert!(f.events.optics(true, 120));
    }
    fake::advance(32, true);
    assert!(f.events.fault == EventsFault::OPTICS && !f.board.optics_enabled);
    f.finish();
}

#[test]
fn close_turns_infrared_off_before_removing_supervision() {
    let mut f = begin();
    f.arm();
    assert!(f.events.optics(true, 120) && f.board.optics_enabled);
    assert!(f.events.close());
    assert!(optics_off() && !f.board.optics_enabled && motor_pins() == MOTOR_MASK);
    assert!(!f.events.opened && sdk::tim2_en() == 0);
    f.board.close();

    // If OFF cannot be verified, close fails and keeps the timer for retries.
    let mut f = begin();
    f.arm();
    assert!(f.events.optics(true, 120));
    hw(|h| h.blocked_optics_off = true);
    assert!(!f.events.close());
    assert!(f.events.opened && f.events.fault == EventsFault::OUTPUT);
    assert!(sdk::tim2_en() != 0 && hw(|h| h.irq[TIMER2_IRQ].is_some()));
    hw(|h| h.blocked_optics_off = false);
    fake::advance(64, true); // The retained ISR retries the OFF request.
    assert!(optics_off());
    f.finish();
}

#[test]
fn masked_interrupts_stay_pending_until_unmasked() {
    let mut f = begin();
    f.arm();
    let (timer_irqs, timer_at) = (hw(|h| h.timer_irqs), f.events.timer_at);
    sdk::disable_irq();
    fake::advance(3 * 32, true); // Updates coalesce into one pending interrupt.
    fake::edge(true);
    assert!(hw(|h| h.timer_irqs) == timer_irqs && hw(|h| h.gpio_irqs) == 0);
    assert_eq!(f.events.feedback.count, 0);
    sdk::set_primask(0);
    assert_eq!(hw(|h| h.timer_irqs), timer_irqs + 1);
    assert!(hw(|h| h.gpio_irqs) == 1 && f.events.feedback.count == 1);
    assert!(f.events.timer_at > timer_at && f.events.fault == EventsFault::OK);
    // A line disabled while pending runs once it is re-enabled.
    sdk::irq_disable(Irq::Timer2);
    fake::advance(2 * 32, true);
    assert_eq!(hw(|h| h.timer_irqs), timer_irqs + 1);
    sdk::irq_enable(Irq::Timer2, 4);
    assert_eq!(hw(|h| h.timer_irqs), timer_irqs + 2);
    f.finish();
}

#[test]
fn port_reads_follow_the_pad_not_the_latch() {
    fake::reset_hardware();
    hw(|h| {
        h.direction = [1 | (1 << 13), 1 << 22];
        h.output = [1 | (1 << 13), 1 << 22];
    });
    // PA0-PA7 have a dead input path on the tested unit; PA13 and PB22 follow.
    assert!(!sdk::gpio_read_bit(0) && sdk::gpio_output_latch(0) & 1 != 0);
    assert!(sdk::gpio_read_bit(13) && sdk::gpio_read_bit(54));
    assert_eq!(sdk::gpioa_ext_port() & ((1 << 13) | 1), 1 << 13);
    hw(|h| {
        h.readback_override_mask[0] = 1 << 13;
        h.readback_override[0] = 0;
    });
    assert!(!sdk::gpio_read_bit(13) && sdk::gpioa_ext_port() & (1 << 13) == 0);
}
