use wellfed_amebad::board::MOTOR_MASK;
use wellfed_amebad::sdk::fake;
use wellfed_amebad::stopped::{self, PA_MASK, PB_MASK};

fn check_preloaded() {
    fake::with(|h| {
        assert_ne!(h.lp_function & wellfed_amebad::sdk::GPIO_ENABLE_BIT, 0);
        assert_ne!(h.lp_clock & wellfed_amebad::sdk::GPIO_ENABLE_BIT, 0);
        assert_eq!(h.output[0] & PA_MASK, MOTOR_MASK);
        assert_eq!(h.output[1] & PB_MASK, 0);
        assert_eq!(h.interrupts, 1);
    });
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
        assert_eq!(h.lp_function, 0x41 | wellfed_amebad::sdk::GPIO_ENABLE_BIT);
        assert_eq!(h.lp_clock, 0x82 | wellfed_amebad::sdk::GPIO_ENABLE_BIT);
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
        assert_eq!(wellfed_amebad::sdk::gpio_port_read(port as u32, mask), 0);
        assert!(!stopped::observe().verified());
        fake::with(|h| h.output[port] &= !mask);
        assert!(stopped::observe().verified());
    }
}

#[test]
fn panic_stop_requests_stop_and_infrared_off_only_when_gpio_is_clocked() {
    fake::reset_hardware();
    fake::with(|h| h.output = [1 << 15 | 1 << 2, 1 << 3 | 1 << 22]);
    wellfed_amebad::sdk::panic_outputs_safe();
    fake::with(|h| assert!(h.interrupts == 1 && h.output == [1 << 15 | 1 << 2, 1 << 3 | 1 << 22]));
    fake::reset_hardware();
    assert!(stopped::initialize().verified());
    fake::with(|h| h.output = [1 << 13 | 1 << 15 | 1 << 2, 1 << 3 | 1 << 22]);
    wellfed_amebad::sdk::panic_outputs_safe();
    fake::with(|h| {
        assert_eq!(h.interrupts, 1);
        assert_eq!(h.output, [MOTOR_MASK | 1 << 2, 1 << 22]);
    });
}
