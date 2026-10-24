//! Power monitor against the fake ADC: health reports a fresh sample
//! with its age, nothing once it is stale or the ADC fails, and no pack
//! voltage when the channel reads near zero.

use petlibre_core::adc_scale::AdcCalibration;
use petlibre_hal::power::{self, DIVIDER_PER_MILLE, Monitor, Record, SAMPLE_MS, STALE_MS, Step};
use petlibre_hal::sdk::fake;
use std::sync::{Mutex, MutexGuard};

/// The published sample is process-wide, so the tests take turns.
static SERIAL: Mutex<()> = Mutex::new(());

/// The fake's erased efuse decodes to the stock defaults.
fn pin_mv(raw: u16) -> u32 {
    AdcCalibration::decode(&[0xff; 4])
        .and_then(|c| c.scale_16(&[raw; 16]))
        .unwrap()
}

fn start(raw: u16, adapter: bool) -> (MutexGuard<'static, ()>, Monitor) {
    let guard = SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    fake::reset_hardware();
    power::reset();
    fake::with(|h| {
        h.battery_adc = raw;
        h.uptime_ms = 10_000;
        if adapter {
            h.input[0] |= 1 << 19;
        } else {
            h.input[0] &= !(1 << 19);
        }
    });
    (guard, Monitor::default())
}

fn advance(ms: u32) {
    fake::with(|h| h.uptime_ms += ms);
}

#[test]
fn reports_a_fresh_sample_with_its_age_until_it_goes_stale() {
    let (_serial, mut monitor) = start(2849, true);
    assert_eq!(power::health(10_000), None);

    assert_eq!(
        monitor.step(),
        Step {
            delay_ms: 1,
            record: Some(Record::Opened { ok: true }),
        }
    );
    let step = monitor.step();
    assert_eq!(step.delay_ms, SAMPLE_MS);
    assert_eq!(
        step.record,
        Some(Record::Sample {
            pin_mv: pin_mv(2849),
            adapter: true,
        })
    );

    advance(1_200);
    let health = power::health(11_200).unwrap();
    assert!(health.adapter);
    assert_eq!(health.age_ms, 1_200);
    assert_eq!(
        health.battery_mv,
        Some(pin_mv(2849) * DIVIDER_PER_MILLE / 1000)
    );

    // The same adapter level is not logged again.
    advance(SAMPLE_MS);
    assert_eq!(monitor.step().record, None);
    assert_eq!(power::health(16_200).unwrap().age_ms, 0);

    // A task that stops sampling reads as unknown, not as the last value.
    assert!(power::health(16_200 + STALE_MS).is_some());
    assert_eq!(power::health(16_201 + STALE_MS), None);
}

#[test]
fn an_adapter_change_is_logged_and_no_pack_reads_as_none() {
    let (_serial, mut monitor) = start(240, true);
    monitor.step();
    monitor.step();
    assert_eq!(power::health(10_000).unwrap().battery_mv, None);

    fake::with(|h| h.input[0] &= !(1 << 19));
    advance(SAMPLE_MS);
    assert_eq!(
        monitor.step().record,
        Some(Record::Sample {
            pin_mv: pin_mv(240),
            adapter: false,
        })
    );
    assert!(!power::health(15_000).unwrap().adapter);
}

#[test]
fn a_failed_conversion_stops_the_monitor_and_clears_health() {
    let (_serial, mut monitor) = start(2849, true);
    monitor.step();
    monitor.step();
    assert!(power::health(10_000).is_some());

    fake::with(|h| h.stuck = true);
    assert_eq!(
        monitor.step(),
        Step {
            delay_ms: 0,
            record: Some(Record::Failed),
        }
    );
    assert_eq!(power::health(10_000), None);
    assert!(!fake::with(|h| h.adc_enabled));
    assert_eq!(monitor.step().delay_ms, 0);
}

#[test]
fn an_unreadable_efuse_never_starts_the_adc() {
    let (_serial, mut monitor) = start(2849, true);
    fake::with(|h| h.efuse_fail = true);

    assert_eq!(monitor.step().record, Some(Record::Opened { ok: false }));
    assert_eq!(monitor.step().delay_ms, 0);
    assert_eq!(power::health(10_000), None);
}
