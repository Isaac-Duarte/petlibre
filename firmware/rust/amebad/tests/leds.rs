use petlibro_amebad::leds::{self, Led};
use petlibro_amebad::sdk::fake;
use petlibro_amebad::stopped;

fn ready() {
    fake::reset_hardware();
    assert!(stopped::initialize().verified());
    assert!(leds::initialize());
}

#[test]
fn bounded_probe_visits_only_indicators_and_keeps_stopped_outputs() {
    ready();
    let mut duration = 0;
    let mut levels = Vec::new();
    for step in (0..).map_while(leds::probe_step) {
        assert!(leds::request(step.led, step.high));
        assert!(leds::observe(step.led).matches(step.high));
        levels.push((step.led.pin(), step.high));
        duration += step.delay_ms;
        assert!(stopped::observe().verified());
        fake::with(|h| assert!(!h.adc_enabled && !h.task_live));
    }
    assert_eq!(levels.len(), 16);
    assert_eq!(duration, 14_000);
    assert_eq!(
        levels,
        [0, 2, 4, 54]
            .into_iter()
            .flat_map(|p| [(p, true), (p, false), (p, true), (p, false)])
            .collect::<Vec<_>>()
    );
    assert!(leds::probe_step(u32::MAX).is_none());
    assert!(leds::all_low());
}

#[test]
fn preloads_before_enabling_indicators_and_preserves_other_pins() {
    fake::reset_hardware();
    assert!(stopped::initialize().verified());
    fake::with(|h| {
        h.output[0] |= leds::PA_MASK | (1 << 9);
        h.output[1] |= leds::PB_MASK | (1 << 10);
        h.gpio_init_hook = Some(|| {
            fake::with(|h| {
                assert_eq!(h.output[0] & leds::PA_MASK, 0);
                assert_eq!(h.output[1] & leds::PB_MASK, 0);
                assert_eq!(h.interrupts, 1);
            })
        });
    });
    assert!(leds::initialize());
    for led in Led::ALL {
        assert!(leds::request(led, true));
    }
    assert!(stopped::observe().verified());
    fake::with(|h| {
        assert_ne!(h.output[0] & (1 << 9), 0);
        assert_ne!(h.output[1] & (1 << 10), 0);
        assert_eq!(h.interrupts, 0);
    });
}

#[test]
fn blocked_write_and_direction_loss_abort_to_low() {
    for led in Led::ALL {
        ready();
        fake::with(|h| h.blocked_gpio_write[(led.pin() >> 5) as usize] = 1 << (led.pin() & 31));
        assert!(leds::request(led, true));
        assert!(!leds::observe(led).matches(true));
        assert!(leds::all_low());
        assert!(stopped::observe().verified());
    }
    ready();
    assert!(leds::request(Led::Connection, true));
    fake::with(|h| h.direction[1] &= !leds::PB_MASK);
    assert!(!leds::request(Led::Food, true));
    assert!(!leds::all_low());
    fake::with(|h| assert_eq!(h.output[0] & leds::PA_MASK, 0));
}

#[test]
fn missing_stopped_state_refuses_indicator_enable() {
    fake::reset_hardware();
    assert!(!leds::initialize());
    ready();
    fake::with(|h| {
        h.output[0] &= !(1 << 13);
        h.interrupts = 1;
    });
    assert!(!leds::request(Led::Fault, true));
    fake::with(|h| {
        assert_eq!(h.output[1] & leds::PB_MASK, 0);
        assert_eq!(h.interrupts, 1);
    });
}

#[test]
fn stale_input_is_distinguished_from_latch_failure_and_mux_mismatch() {
    ready();
    fake::with(|h| h.readback_override_mask[0] = 1);
    assert!(leds::request(Led::Connection, true));
    let stale = leds::observe(Led::Connection);
    assert!(stale.latch_high && !stale.input_high && stale.directions_ok && stale.stopped_ok);
    assert!(!stale.matches(true));
    fake::with(|h| {
        assert_eq!(h.interrupts, 0);
        h.readback_override_mask[0] = 0;
    });
    assert!(leds::observe(Led::Connection).matches(true));
    fake::with(|h| h.pad_control[0] = 11);
    assert!(!leds::observe(Led::Connection).matches(true));

    ready();
    fake::with(|h| {
        h.blocked_gpio_write[0] = 1;
        h.readback_override_mask[0] = 1;
        h.readback_override[0] = 1;
    });
    assert!(leds::request(Led::Connection, true));
    let blocked = leds::observe(Led::Connection);
    assert!(!blocked.latch_high && blocked.input_high && !blocked.matches(true));
}

#[test]
fn input_only_high_mismatch_can_complete_bounded_probe_but_never_passes() {
    use leds::Decision;
    ready();
    fake::with(|h| {
        h.readback_override_mask = [leds::PA_MASK, leds::PB_MASK];
        h.readback_override = [0; 2];
    });
    let mut mismatches = 0;
    let mut duration = 0;
    for step in (0..).map_while(leds::probe_step) {
        assert!(leds::request(step.led, step.high));
        let readback = leds::observe(step.led);
        if step.high {
            assert_eq!(readback.decision(true), Decision::InputMismatch);
            assert!(!readback.matches(true));
            mismatches += 1;
        } else {
            assert_eq!(readback.decision(false), Decision::Verified);
        }
        assert!(stopped::observe().verified());
        duration += step.delay_ms;
    }
    assert_eq!(mismatches, 8);
    assert_eq!(duration, 14_000);
    assert!(leds::all_low());
}

#[test]
fn high_mismatch_continuation_refuses_latch_direction_stop_and_pad_failures() {
    use leds::{Decision, Readback};
    let high = Readback {
        latch_high: true,
        input_high: false,
        directions_ok: true,
        stopped_ok: true,
        pad: 0,
    };
    for bad in [
        Readback {
            latch_high: false,
            ..high
        },
        Readback {
            directions_ok: false,
            ..high
        },
        Readback {
            stopped_ok: false,
            ..high
        },
        Readback { pad: 11, ..high },
        Readback {
            pad: 1 << 15,
            ..high
        },
        Readback {
            pad: 1 << 9,
            ..high
        },
        Readback {
            pad: 1 << 8,
            ..high
        },
    ] {
        assert_eq!(bad.decision(true), Decision::Abort);
    }
    let low = Readback {
        latch_high: false,
        input_high: true,
        ..high
    };
    assert_eq!(low.decision(false), Decision::Abort);
}

#[test]
fn indicator_pull_removal_preserves_unrelated_pad_flags_and_pins() {
    fake::reset_hardware();
    assert!(stopped::initialize().verified());
    fake::with(|h| h.pad_control = [0x5f00; 64]);
    assert!(leds::initialize());
    fake::with(|h| {
        for pin in 0..64 {
            let indicator = Led::ALL.into_iter().any(|led| led.pin() as usize == pin);
            assert_eq!(h.pad_control[pin], if indicator { 0x5c00 } else { 0x5f00 });
        }
    });
}

#[test]
fn blocked_low_write_cannot_hide_behind_a_low_input() {
    ready();
    assert!(leds::request(Led::Connection, true));
    fake::with(|h| {
        h.readback_override_mask[0] = 1;
        h.readback_override[0] = 0;
        h.blocked_gpio_write[0] = 1;
    });
    assert!(!leds::all_low());
}
