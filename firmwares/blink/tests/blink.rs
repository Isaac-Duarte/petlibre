//! The blinker against the fake GPIO model: it alternates the connection
//! indicator with a settle and readback before each hold, reports only the
//! first levels, and stops with every indicator LOW the moment outputs stop
//! reading back stopped.

use wellfed_fw_blink::blink::{Blinker, HALF_PERIOD_MS, LED, REPORTED, Record, Step};
use wellfed_sdk::hal::leds::{self, Decision};
use wellfed_sdk::hal::sdk::{self, fake};
use wellfed_sdk::hal::stopped;

fn ready() -> Blinker {
    fake::reset_hardware();
    assert!(stopped::initialize().verified());
    assert!(leds::initialize());
    Blinker::default()
}

/// One request and its readback: the settle step, then the hold step.
fn level(blinker: &mut Blinker) -> Step {
    let settle = blinker.step();
    assert_eq!(settle.record, None);
    assert_eq!(settle.delay_ms, leds::SETTLE_MS);
    blinker.step()
}

#[test]
fn alternates_the_indicator_and_holds_each_level_half_a_period() {
    let mut blinker = ready();

    let mut records = Vec::new();
    for _ in 0..10 {
        let step = level(&mut blinker);
        assert_eq!(step.delay_ms + leds::SETTLE_MS, HALF_PERIOD_MS);
        records.extend(step.record);
        assert!(stopped::observe().verified());
    }

    // Port A has no input readback, so each level is output-only.
    let expected: Vec<Record> = (1..=REPORTED)
        .map(|count| Record::Level {
            high: count % 2 == 1,
            decision: Decision::OutputOnly,
            count,
        })
        .collect();
    assert_eq!(records, expected);
    assert!(!leds::observe(LED).latch_high);
}

#[test]
fn a_refused_request_stops_with_every_indicator_low() {
    let mut blinker = ready();
    level(&mut blinker);
    assert!(leds::observe(LED).latch_high);

    // Motor STOP lost: the next request is refused, and nothing retries.
    fake::with(|h| h.output[0] &= !sdk::MOTOR_A_BIT);
    let step = blinker.step();

    assert_eq!(step.record, Some(Record::Refused { high: false }));
    assert_eq!(step.delay_ms, 0);
    assert!(!leds::observe(LED).latch_high);
}
