//! Read-only ADC probe against the fake SDK: the LP ADC enable is
//! explicit and verified, a failed enable, efuse read or conversion ends the
//! probe, and the motor pins never leave STOP.

use wellfed_hal::board::MOTOR_MASK;
use wellfed_hal::sdk::{ADC_ENABLE_BIT, GPIO_ENABLE_BIT, fake};
use wellfed_hal::stopped;
use wellfed_probes::adc::{Channel, Probe, Record, SAMPLE_MS};

fn start() -> Probe {
    fake::reset_hardware();
    fake::with(|h| h.interrupts = 1);
    assert!(stopped::initialize().verified());
    Probe::default()
}

#[test]
fn opens_with_explicit_adc_enable_and_samples_read_only() {
    let mut probe = start();

    fake::with(|h| {
        h.current_adc = 300;
        h.battery_adc = 2600;
        h.input[0] |= 1 << 19;
    });

    let step = probe.step();
    assert_eq!(step.delay_ms, SAMPLE_MS);
    let Some(Record::Open {
        outputs,
        clock,
        efuse,
        calibration,
        ok,
    }) = step.record
    else {
        panic!("{step:?}");
    };
    assert!(outputs && ok && clock.verified());
    assert_eq!(
        (clock.function_before, clock.clock_before),
        (GPIO_ENABLE_BIT, GPIO_ENABLE_BIT)
    );
    assert_eq!(clock.function_after, GPIO_ENABLE_BIT | ADC_ENABLE_BIT);
    assert_eq!(efuse, Some([0xff; 4]));
    assert!(calibration.default_offset && calibration.default_gain);

    for n in 0..3 {
        let step = probe.step();
        assert_eq!(step.delay_ms, SAMPLE_MS);

        let Some(Record::Sample {
            n: got,
            dc,
            stopped,
            current,
            battery,
            supply,
            ..
        }) = step.record
        else {
            panic!("{step:?}");
        };

        assert_eq!(got, n);
        assert!(dc && stopped);

        // Default calibration: (300 * 10 - 2480) * 1000 / 12050.
        assert_eq!(
            current,
            Channel {
                min: 300,
                max: 300,
                mean: 300,
                scaled: 43
            }
        );
        assert_eq!(battery.mean, 2600);
        assert_eq!(battery.scaled, (26000 - 2480) * 1000 / 12050);
        assert_eq!(supply.mean, 3000);
    }

    // Motor pins never left STOP; emitters never enabled.
    fake::with(|h| assert_eq!(h.output[0] & MOTOR_MASK, MOTOR_MASK));
}

#[test]
fn an_already_enabled_adc_is_preserved_and_still_verified() {
    let mut probe = start();
    fake::with(|h| {
        h.lp_function |= ADC_ENABLE_BIT | 0x41;
        h.lp_clock |= ADC_ENABLE_BIT;
    });

    let Some(Record::Open { clock, ok, .. }) = probe.step().record else {
        panic!();
    };

    assert!(ok && clock.verified());
    assert_eq!(clock.function_before, clock.function_after);
}

#[test]
fn a_blocked_adc_enable_or_efuse_failure_ends_without_sampling() {
    for blocked in [true, false] {
        let mut probe = start();
        fake::with(|h| {
            h.blocked_gpio_clock = blocked;
            h.efuse_fail = !blocked;
        });

        let step = probe.step();
        let Some(Record::Open {
            outputs,
            clock,
            efuse,
            ok,
            ..
        }) = step.record
        else {
            panic!("{step:?}");
        };

        assert!(outputs && !ok);
        assert_eq!(clock.verified(), !blocked);
        assert_eq!(efuse.is_none(), !blocked);
        assert_eq!(step.delay_ms, 0);
        assert_eq!(probe.step().record, None);
        fake::with(|h| assert_eq!(h.reads, 0));
    }
}

#[test]
fn a_stuck_conversion_trips_and_ends() {
    let mut probe = start();
    assert!(matches!(
        probe.step().record,
        Some(Record::Open { ok: true, .. })
    ));
    assert!(matches!(
        probe.step().record,
        Some(Record::Sample { n: 0, .. })
    ));

    fake::with(|h| h.stuck = true);

    let step = probe.step();
    assert_eq!(step.delay_ms, 0);
    assert!(
        matches!(
            step.record,
            Some(Record::Error {
                n: 1,
                stopped: true,
                ..
            })
        ),
        "{step:?}"
    );

    assert!(probe.board.view().faulted || !probe.board.view().opened);
    fake::with(|h| assert_eq!(h.output[0] & MOTOR_MASK, MOTOR_MASK));
}
