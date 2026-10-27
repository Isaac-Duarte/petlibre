//! Indicator LED driver against the fake GPIO model. Indicators enable only
//! from a verified stopped state with their latches preloaded LOW; only PB22
//! can be input-verified (Port A indicators are output-only); and a failed
//! write, direction, pad or stopped check aborts to all LOW.

use wellfed_hal::leds::{self, Led};
use wellfed_hal::sdk::fake;
use wellfed_hal::stopped;

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
        let readback = leds::observe(step.led);
        assert!(readback.accepted(step.high));
        // Only PB22 can verify a level; Port A indicators are never input-verified.
        assert_eq!(readback.matches(step.high), step.led.input_readable());
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
        h.direction_hook = Some(|| {
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
        assert!(!leds::observe(led).accepted(true));
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
    use leds::Decision;
    let fault = Led::Fault.pin();
    let (port, bit) = ((fault >> 5) as usize, 1 << (fault & 31));

    ready();
    fake::with(|h| h.readback_override_mask[port] = bit);
    assert!(leds::request(Led::Fault, true));
    let stale = leds::observe(Led::Fault);
    assert!(stale.latch_high && !stale.input_high && stale.directions_ok && stale.stopped_ok);
    assert!(!stale.accepted(true));

    fake::with(|h| {
        assert_eq!(h.interrupts, 0);
        h.readback_override_mask[port] = 0;
    });

    assert!(leds::observe(Led::Fault).matches(true));
    fake::with(|h| h.pad_control[fault as usize] = 11);
    assert!(!leds::observe(Led::Fault).accepted(true));

    ready();
    fake::with(|h| {
        h.blocked_gpio_write[port] = bit;
        h.readback_override_mask[port] = bit;
        h.readback_override[port] = bit;
    });
    assert!(leds::request(Led::Fault, true));
    let blocked = leds::observe(Led::Fault);
    assert!(!blocked.latch_high && blocked.input_high && !blocked.accepted(true));

    // Output-only indicators ignore the input but still abort on pad or latch faults.
    ready();
    fake::with(|h| h.readback_override_mask[0] = 1);
    assert!(leds::request(Led::Connection, true));
    let output = leds::observe(Led::Connection);
    assert_eq!(output.decision(true), Decision::OutputOnly);
    assert!(output.accepted(true) && !output.matches(true));

    fake::with(|h| h.pad_control[0] = 11);
    assert_eq!(
        leds::observe(Led::Connection).decision(true),
        Decision::Abort
    );

    ready();
    fake::with(|h| h.blocked_gpio_write[0] = 1);
    assert!(leds::request(Led::Connection, true));
    assert_eq!(
        leds::observe(Led::Connection).decision(true),
        Decision::Abort
    );
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
        if !step.led.input_readable() {
            assert_eq!(readback.decision(step.high), Decision::OutputOnly);
        } else if step.high {
            assert_eq!(readback.decision(true), Decision::InputMismatch);
            assert!(!readback.accepted(true));
            mismatches += 1;
        } else {
            assert_eq!(readback.decision(false), Decision::Verified);
        }
        assert!(stopped::observe().verified());
        duration += step.delay_ms;
    }

    assert_eq!(mismatches, 2);
    assert_eq!(duration, 14_000);
    assert!(leds::all_low());
}

#[test]
fn high_mismatch_continuation_refuses_latch_direction_stop_and_pad_failures() {
    use leds::{Decision, Readback};
    let high = Readback {
        latch_high: true,
        input_high: false,
        input_highs: 0,
        input_readable: true,
        directions_ok: true,
        stopped_ok: true,
        pad: 0,
    };

    // Extra samples are reported only; they never verify a failed input read.
    let sampled = Readback {
        input_highs: leds::INPUT_SAMPLES,
        ..high
    };
    assert_eq!(sampled.decision(true), Decision::InputMismatch);

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
fn the_indicator_service_drives_lights_while_the_motor_runs_and_emitters_are_lit() {
    use leds::Decision;
    use wellfed_hal::plaf103::pins::{IR_PA_BIT, MOTOR_A_BIT};
    ready();

    // A feed: one motor pin driven, the outlet emitter lit.
    fake::with(|h| h.output[0] = (h.output[0] & !MOTOR_A_BIT) | IR_PA_BIT);
    assert!(!stopped::observe().verified());
    let outputs = fake::with(|h| {
        (
            h.output[0] & stopped::PA_MASK,
            h.output[1] & stopped::PB_MASK,
        )
    });

    for led in Led::ALL {
        assert!(leds::show(led, true));
        assert!(
            !leds::observe(led).accepted(true),
            "the probe's decision still wants stopped outputs"
        );
        let expected = if led.input_readable() {
            Decision::Verified
        } else {
            Decision::OutputOnly
        };
        assert_eq!(leds::shown(led, true), expected);
        assert!(leds::show(led, false));
        assert_eq!(leds::shown(led, false), expected);
    }
    fake::with(|h| {
        assert_eq!(
            (
                h.output[0] & stopped::PA_MASK,
                h.output[1] & stopped::PB_MASK
            ),
            outputs,
            "no motor or emitter bit moved"
        );
    });

    // A latch that did not take still fails its check.
    fake::with(|h| h.blocked_gpio_write[0] = 1 << Led::Lock.pin());
    assert!(leds::show(Led::Lock, true));
    assert_eq!(leds::shown(Led::Lock, true), Decision::Abort);

    // A pin no longer an output refuses the request and darkens them all.
    fake::with(|h| {
        h.blocked_gpio_write[0] = 0;
        h.output[0] |= leds::PA_MASK;
        h.direction[1] &= !leds::PB_MASK;
    });
    assert!(!leds::show(Led::Food, true));
    fake::with(|h| assert_eq!(h.output[0] & leds::PA_MASK, 0));
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
