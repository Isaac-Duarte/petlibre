//! Stopped-state startup against the fake GPIO model: STOP and infrared-off
//! latches precede every output enable, other pins are preserved, and a
//! blocked write, lost direction, stuck SWD release or later pin drift never
//! reports stopped.

use wellfed_hal::board::MOTOR_MASK;
use wellfed_hal::sdk::{self, fake};
use wellfed_hal::stopped::{self, PA_MASK, PB_MASK};

fn check_preloaded() {
    fake::with(|h| {
        assert_ne!(h.lp_function & wellfed_hal::sdk::GPIO_ENABLE_BIT, 0);
        assert_ne!(h.lp_clock & wellfed_hal::sdk::GPIO_ENABLE_BIT, 0);
        assert_eq!(h.output[0] & sdk::IR_PA_BIT, 0);
        assert_eq!(h.output[1] & PB_MASK, 0);
        assert_eq!(h.interrupts, 1);
    });
    check_never_driven();
}

/// What the driver sees on each motor input: the latch while the pin is an
/// output, otherwise LOW through the driver's pull-down. Equal is standby or
/// brake; unequal is drive.
fn check_never_driven() {
    fake::with(|h| {
        let seen = h.output[0] & h.direction[0] & MOTOR_MASK;
        assert!(seen == 0 || seen == MOTOR_MASK, "motor driven: {seen:#x}");
    });
}

#[test]
fn startup_reaches_stop_through_standby_only() {
    for (latch, direction) in [
        (0, 0),
        (MOTOR_MASK, 0),
        (MOTOR_MASK, MOTOR_MASK),
        (0, MOTOR_MASK),
    ] {
        fake::reset_hardware();
        fake::with(|h| {
            h.output[0] = latch;
            h.direction[0] = direction;
            h.gpio_init_hook = Some(check_never_driven);
            h.output_hook = Some(check_never_driven);
        });

        assert!(stopped::initialize().verified());
    }
}

#[test]
fn clock_enable_preserves_other_flags_and_blocked_enable_touches_no_gpio() {
    fake::reset_hardware();
    fake::with(|h| {
        h.lp_function = 0x41;
        h.lp_clock = 0x82;
        h.interrupts = 1;
    });

    let snapshot = stopped::initialize();
    assert!(snapshot.verified());
    assert_eq!(snapshot.clock.function_before, 0x41);
    assert_eq!(snapshot.clock.clock_before, 0x82);
    fake::with(|h| {
        assert_eq!(h.lp_function, 0x41 | wellfed_hal::sdk::GPIO_ENABLE_BIT);
        assert_eq!(h.lp_clock, 0x82 | wellfed_hal::sdk::GPIO_ENABLE_BIT);
        assert_eq!(h.interrupts, 1);
    });

    fake::reset_hardware();
    fake::with(|h| h.blocked_gpio_clock = true);
    assert!(!stopped::initialize().verified());
    fake::with(|h| {
        assert_eq!(h.output, [0; 2]);
        assert_eq!(h.direction, [0; 2]);
    });
}

#[test]
fn latches_precede_every_output_enable_and_other_pins_are_preserved() {
    fake::reset_hardware();
    fake::with(|h| {
        h.output = [!MOTOR_MASK, u32::MAX];
        h.direction = [1 << 2, 1 << 22];
        h.gpio_init_hook = Some(check_preloaded);
    });

    let result = stopped::initialize();
    assert!(result.verified());

    fake::with(|h| {
        assert_eq!(h.output, [!(1u32 << 15), !PB_MASK]);
        assert_eq!(h.direction, [PA_MASK | (1 << 2), PB_MASK | (1 << 22)]);
        assert_eq!(h.interrupts, 0);
        assert!(!h.adc_enabled && !h.trigger && !h.task_live);
        assert_eq!(h.watchdog_init_calls, 0);
    });
}

#[test]
fn blocked_stop_or_infrared_write_cannot_report_success() {
    for (port, mask) in [(0, 1 << 13), (0, 1 << 14), (0, 1 << 15), (1, PB_MASK)] {
        fake::reset_hardware();
        fake::with(|h| {
            h.output = [1 << 15, PB_MASK];
            h.blocked_gpio_write[port] = mask;
            h.interrupts = 1;
        });
        assert!(!stopped::initialize().verified());
        fake::with(|h| assert_eq!(h.interrupts, 1));
    }
}

#[test]
fn lost_direction_or_later_pin_drift_is_not_stopped() {
    fake::reset_hardware();
    assert!(stopped::initialize().verified());

    fake::with(|h| h.direction[1] &= !PB_MASK);
    assert!(!stopped::observe().verified());

    fake::with(|h| {
        h.direction[1] |= PB_MASK;
        h.output[0] &= !(1 << 14);
    });
    assert!(!stopped::observe().verified());
}

#[test]
fn infrared_off_requires_a_low_latch_not_only_a_low_input() {
    // A dead input path reads LOW while the emitter latch is HIGH.
    for (port, mask) in [(0usize, 1u32 << 15), (1, PB_MASK)] {
        fake::reset_hardware();
        assert!(stopped::initialize().verified());
        fake::with(|h| {
            h.output[port] |= mask;
            h.readback_override_mask[port] = mask;
            h.readback_override[port] = 0;
        });
        assert_eq!(wellfed_hal::sdk::gpio_port_read(port as u32, mask), 0);
        assert!(!stopped::observe().verified());
        fake::with(|h| h.output[port] &= !mask);
        assert!(stopped::observe().verified());
    }
}

#[test]
fn panic_stop_requests_stop_and_infrared_off_only_when_gpio_is_clocked() {
    fake::reset_hardware();
    fake::with(|h| h.output = [1 << 15 | 1 << 2, 1 << 3 | 1 << 22]);
    wellfed_hal::sdk::panic_outputs_safe();
    fake::with(|h| assert!(h.interrupts == 1 && h.output == [1 << 15 | 1 << 2, 1 << 3 | 1 << 22]));

    fake::reset_hardware();
    assert!(stopped::initialize().verified());
    fake::with(|h| h.output = [1 << 13 | 1 << 15 | 1 << 2, 1 << 3 | 1 << 22]);
    wellfed_hal::sdk::panic_outputs_safe();
    fake::with(|h| {
        assert_eq!(h.interrupts, 1);
        assert_eq!(h.output, [MOTOR_MASK | 1 << 2, 1 << 22]);
    });
}

#[test]
fn pb3_is_driven_only_after_swd_is_released() {
    fake::reset_hardware();
    // Reset state: SWD holds PB3, so a HIGH latch never reaches the pad.
    fake::with(|h| {
        h.direction[1] |= 1 << 3;
        h.output[1] |= 1 << 3;
    });
    assert!(!sdk::gpio_read_bit(35));

    fake::with(|h| h.output[1] = 0);
    assert!(stopped::initialize().verified());
    assert_eq!(sdk::swd_pinmux() & sdk::SWD_PMUX_BIT, 0);
    fake::with(|h| h.output[1] |= 1 << 3);
    assert!(sdk::gpio_read_bit(35));
}

#[test]
fn a_stuck_swd_release_fails_stopped_startup() {
    fake::reset_hardware();
    fake::with(|h| h.swd_stuck = true);
    let snapshot = stopped::initialize();
    assert!(!snapshot.verified() && !snapshot.motor_verified());
}
