//! Board driver against the fake GPIO/ADC model. Outputs need an attached
//! flash authority; a failed write, readback or ADC conversion faults the
//! board with the motor stopped and optics off; and a flash hold cancels
//! pending or active output and never resumes it.

mod common;

use common::*;
use wellfed_core::feed::controller::MotorCommand;
use wellfed_core::feed::sensor_inputs::SensorReading;
use wellfed_core::flash_coord::{FlashFault, FlashParty, FlashReply};
use wellfed_hal::board::{Board, MOTOR_MASK};
use wellfed_hal::sdk::{self, fake};

#[test]
fn startup_capture_and_stop() {
    let mut board = open_board();
    let _authority = flash_attach(&mut board);

    assert_eq!(motor_pins(), MOTOR_MASK);
    assert!(board.optics(true));

    let mut reading = SensorReading::default();
    assert!(board.capture(&mut reading));
    assert!(reading.gpio_valid && reading.dc_present && reading.outlet_high && !reading.food_high);
    assert!(reading.optics_enabled && reading.current_adc_valid && reading.battery_adc_valid);
    assert!(hw(|h| h.reads) == 32 && reading.current[15] == 500 && reading.battery[15] == 2800);

    assert!(board.motor(MotorCommand::DRIVE_0, 3));
    assert_eq!(motor_pins(), 1 << 13);
    assert!(!board.motor_tick(13));
    assert!(motor_pins() == MOTOR_MASK && hw(|h| h.interrupts) == 0);

    board.close();
    assert!(!board.opened && !hw(|h| h.adc_enabled) && !hw(|h| h.trigger));
}

#[test]
fn adc_timeout_cancels_pending_drive() {
    let mut board = open_board();
    let _authority = flash_attach(&mut board);

    assert!(board.motor(MotorCommand::DRIVE_0, 0));
    hw(|h| h.stuck = true);
    let mut reading = SensorReading::default();
    assert!(!board.capture(&mut reading));

    assert!(board.faulted && !hw(|h| h.trigger) && hw(|h| h.reads) == 0);
    assert!(hw(|h| h.polls) == 8 && hw(|h| h.clears) == 2);
    assert!(!reading.current_adc_valid && !reading.battery_adc_valid);
    assert!(!board.motor_tick(3));
    assert!(!board.motor(MotorCommand::DRIVE_1, 4));
    assert_eq!(motor_pins(), MOTOR_MASK);
}

#[test]
fn failed_output_is_not_reported_stopped() {
    let mut board = open_board();
    let _authority = flash_attach(&mut board);

    assert!(board.motor(MotorCommand::DRIVE_0, 3));
    hw(|h| h.blocked_motor_write = true);
    assert!(!board.motor(MotorCommand::STOP, 4));
    assert!(board.faulted && !board.motor.applied_known);

    hw(|h| h.blocked_motor_write = false);
    assert!(!board.motor_tick(5));
    assert!(board.motor.applied_known && motor_pins() == MOTOR_MASK);
}

#[test]
fn calibration_failure_never_opens_the_port() {
    fake::reset_hardware();
    hw(|h| h.efuse_fail = true);
    let mut board = Box::<Board>::default();

    assert!(!board.open(3, 10, 8, 0));
    assert!(!board.opened && board.faulted && !hw(|h| h.adc_enabled));
    assert!(!board.motor(MotorCommand::DRIVE_0, 3));
    assert_eq!(motor_pins(), MOTOR_MASK);
}

#[test]
fn emitter_write_failure_stops_the_motor() {
    let mut board = open_board();
    let _authority = flash_attach(&mut board);

    assert!(board.motor(MotorCommand::DRIVE_0, 3));
    hw(|h| h.blocked_optics_write = true);
    assert!(!board.optics(true));

    assert!(board.faulted && !board.optics_enabled);
    assert_eq!(motor_pins(), MOTOR_MASK);
    assert!(optics_off());
}

#[test]
fn missing_authority_and_rebinding_never_enable_outputs() {
    let mut b = open_board();
    assert!(!b.motor(MotorCommand::DRIVE_0, 3));
    assert!(!b.optics(true));
    assert!(!b.faulted && motor_pins() == MOTOR_MASK);

    let mut c = flash_attach(&mut b);
    let mut other = flash_ready();
    // SAFETY: the fixture keeps the coordinator alive and in place.
    assert!(!unsafe { b.attach_flash(&mut *other) });
    assert_eq!(b.flash, &raw mut *c);

    assert_eq!(c.request(FlashParty::LOCAL, 17, 1, 0), FlashReply::ACCEPTED);
    assert!(!b.optics(true));
    assert!(!b.motor(MotorCommand::DRIVE_1, 3));
    assert!(b.flash_quiesce() && !b.faulted);
    assert!(optics_off());

    let t = c.token;
    assert_eq!(c.quiesced(&t, true, 1), FlashReply::ACCEPTED);
    assert_eq!(c.parked(&t, FlashParty::PEER, 2), FlashReply::ACCEPTED);
    assert_eq!(c.complete(&t, FlashParty::LOCAL, 3), FlashReply::ACCEPTED);

    assert!(!b.optics(true));
    assert_eq!(
        c.released(&t, FlashParty::PEER, true, 4),
        FlashReply::ACCEPTED
    );
    assert!(b.optics(true));
    b.close();
}

#[test]
fn hold_cancels_pending_or_active_output_and_never_resumes_it() {
    for output in 0..3 {
        let mut b = open_board();
        let mut c = flash_attach(&mut b);
        if output < 2 {
            assert!(b.motor(MotorCommand::DRIVE_0, if output == 0 { 0 } else { 3 }));
        } else {
            assert!(b.optics(true));
        }

        assert_eq!(c.request(FlashParty::LOCAL, 17, 1, 0), FlashReply::ACCEPTED);
        hw(|h| h.interrupts = 1); // Nested ISR use must preserve the mask.
        assert!(!b.motor_tick(3));
        assert!(hw(|h| h.interrupts) == 1 && b.faulted);
        hw(|h| h.interrupts = 0);
        assert!(b.flash_quiesce());
        assert!(motor_pins() == MOTOR_MASK && optics_off());

        let t = c.token;
        assert_eq!(c.quiesced(&t, true, 1), FlashReply::ACCEPTED);
        assert_eq!(c.parked(&t, FlashParty::PEER, 2), FlashReply::ACCEPTED);
        assert_eq!(c.complete(&t, FlashParty::LOCAL, 3), FlashReply::ACCEPTED);
        assert_eq!(
            c.released(&t, FlashParty::PEER, true, 4),
            FlashReply::ACCEPTED
        );

        assert!(!b.motor(MotorCommand::DRIVE_1, 4));
        assert!(!b.optics(true));
        b.close();
    }
}

#[test]
fn failed_quiescence_cannot_acknowledge_flash_permission() {
    for output in 0..2 {
        let mut b = open_board();
        let mut c = flash_attach(&mut b);
        assert!(b.motor(MotorCommand::DRIVE_0, 3));
        assert!(b.optics(true));
        assert_eq!(c.request(FlashParty::PEER, 29, 1, 0), FlashReply::ACCEPTED);
        hw(|h| {
            h.blocked_motor_write = output == 0;
            h.blocked_optics_off = output == 1;
        });

        let inactive = b.flash_quiesce();
        assert!(!inactive && b.faulted);
        let t = c.token;
        assert_eq!(c.quiesced(&t, inactive, 1), FlashReply::FAILED);
        assert_eq!(c.fault, FlashFault::OUTPUT);
        assert!(!c.may_access(&t, FlashParty::PEER, 2));

        hw(|h| {
            h.blocked_motor_write = false;
            h.blocked_optics_off = false;
        });
        assert!(b.flash_quiesce());
        assert!(!b.optics(true));
        b.close();
    }
}

#[test]
fn infrared_off_checks_the_latch_behind_a_dead_input() {
    let mut b = open_board();
    let _authority = flash_attach(&mut b);

    assert!(b.optics(true));
    // Pads whose input reads LOW while the latch stays HIGH are not OFF.
    hw(|h| {
        h.blocked_optics_off = true;
        h.readback_override_mask[1] = 1 << 3;
        h.readback_override[1] = 0;
    });
    assert!(!b.optics(false));
    assert!(b.faulted && !optics_off() && !b.flash_quiesce());
    assert!(hw(|h| h.output[1] & (1 << 3) != 0));

    hw(|h| h.blocked_optics_off = false);
    assert!(b.optics(false) && b.flash_quiesce());
    b.close();
}

#[test]
fn outputs_only_open_leaves_sensors_and_adc_untouched() {
    fake::reset_hardware();
    let mut board = Box::<Board>::default();
    hw(|h| h.efuse_fail = true);

    assert!(board.open_outputs(3, 10, 0));
    assert!(board.opened && !board.sensors_ready && !board.faulted);
    assert_eq!(motor_pins(), MOTOR_MASK);
    assert!(optics_off());

    // A failing efuse would refuse a full open: neither it nor the ADC is touched.
    assert!(!hw(|h| h.adc_enabled));
    let mut reading = SensorReading::default();
    assert!(!board.capture(&mut reading) && !reading.gpio_valid && hw(|h| h.polls) == 0);
    assert!(board.motor_tick(3) && motor_pins() == MOTOR_MASK);

    board.close();
    assert!(!board.opened && !hw(|h| h.adc_enabled) && motor_pins() == MOTOR_MASK);
}

#[test]
fn optics_select_drives_each_emitter_alone_with_readback() {
    let mut b = open_board();
    let _authority = flash_attach(&mut b);

    assert!(b.optics_select(true, false));
    assert!(sdk::gpio_read_bit(15) && !sdk::gpio_read_bit(35) && b.optics_enabled);
    assert!(b.optics_select(false, true));
    assert!(!sdk::gpio_read_bit(15) && sdk::gpio_read_bit(35) && b.optics_enabled);

    let mut reading = SensorReading::default();
    hw(|h| h.input[0] |= 1 << 18 | 1 << 16);
    let pins = b.capture_gpio(&mut reading).unwrap();
    // One emitter is not "optics enabled" for qualification; no ADC read.
    assert!(reading.gpio_valid && !reading.optics_enabled && reading.outlet_high);
    assert!(!reading.current_adc_valid && !reading.battery_adc_valid);
    assert_eq!(pins & (1 << 18 | 1 << 16), 1 << 18 | 1 << 16);

    // A dead emitter pad fails readback: both off, board tripped.
    hw(|h| h.dead_input[0] |= 1 << 15);
    assert!(!b.optics_select(true, false));
    assert!(optics_off() && b.faulted);
    assert!(b.capture_gpio(&mut reading).is_none());
}

#[test]
fn open_refuses_while_swd_holds_pb3() {
    fake::reset_hardware();
    hw(|h| h.swd_stuck = true);
    let mut b = Box::<Board>::default();
    assert!(!b.open(3, 10, 8, 0));
    assert!(b.faulted && !b.opened && optics_off());
}
