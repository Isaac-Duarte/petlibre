//! FW20 button service against the fake SDK: the pins open as inputs, a
//! press and a hold come out as gestures in order, the reader takes them from
//! the queue, and a full queue drops new gestures rather than old ones.

use std::sync::{Mutex, MutexGuard};

use wellfed_core::buttons::{Button, Buttons, Config, Gesture};
use wellfed_hal::buttons::{self, POLL_MS, Record, Service};
use wellfed_hal::sdk::fake;

/// The queue is process-wide; the fake hardware is per thread.
static SERIAL: Mutex<()> = Mutex::new(());

const RELEASED: u32 = 0xf0;

fn start() -> (MutexGuard<'static, ()>, Service) {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    fake::reset_hardware();
    buttons::reset();
    fake::with(|h| {
        h.input[1] |= RELEASED;
        h.direction[1] |= RELEASED;
    });
    (guard, Service::new(Config::BENCH))
}

fn press(levels: u32) {
    fake::with(|h| h.input[1] = (h.input[1] & !RELEASED) | levels);
}

/// Steps for `ms`, collecting the records.
fn run(service: &mut Service, ms: u32) -> Vec<Record> {
    let mut records = Vec::new();
    for _ in 0..ms / POLL_MS {
        fake::with(|h| h.uptime_ms += POLL_MS);
        let step = service.step();
        assert_eq!(step.delay_ms, POLL_MS);
        records.extend(step.record);
    }
    records
}

fn gestures(records: &[Record]) -> Vec<Gesture> {
    records
        .iter()
        .filter_map(|record| match record {
            Record::Gesture { gesture, .. } => Some(*gesture),
            _ => None,
        })
        .collect()
}

#[test]
fn the_pins_open_as_inputs_with_nobody_pressing() {
    let (_guard, mut service) = start();

    let step = service.step();

    assert_eq!(
        step.record,
        Some(Record::Open {
            inputs: true,
            pressed: Buttons::NONE
        })
    );
    assert_eq!(fake::with(|h| h.direction[1]) & RELEASED, 0);
}

#[test]
fn a_dispense_hold_reaches_the_reader_in_order() {
    let (_guard, mut service) = start();
    let dispense = Buttons::of(Button::Dispense);
    service.step();

    press(0xb0);
    let mut records = run(&mut service, 2_500);
    press(RELEASED);
    records.extend(run(&mut service, 100));

    let expected = [
        Gesture::Down { chord: dispense },
        Gesture::Hold {
            chord: dispense,
            level: 1,
        },
        Gesture::Up {
            chord: dispense,
            level: 1,
            held_ms: 2_500,
        },
    ];
    assert_eq!(gestures(&records), expected);
    let read: Vec<Gesture> = std::iter::from_fn(buttons::next).collect();
    assert_eq!(read, expected);
}

#[test]
fn a_full_queue_drops_new_gestures_and_counts_them() {
    let (_guard, mut service) = start();
    service.step();

    // Twelve taps, each a down and an up, with nobody reading.
    let mut records = Vec::new();
    for _ in 0..12 {
        press(0xe0);
        records.extend(run(&mut service, 100));
        press(RELEASED);
        records.extend(run(&mut service, 100));
    }

    let Some(Record::Gesture { dropped, .. }) = records.last() else {
        panic!("{records:?}");
    };
    assert_eq!(*dropped, 16);
    let read: Vec<Gesture> = std::iter::from_fn(buttons::next).collect();
    assert_eq!(read.len(), 8);
    assert!(matches!(read[0], Gesture::Down { .. }));
}

#[test]
fn an_invalid_configuration_is_refused_without_touching_the_pins() {
    let (_guard, _) = start();
    let mut service = Service::new(Config {
        debounce_ms: 0,
        ..Config::BENCH
    });

    let step = service.step();

    assert_eq!(step.record, Some(Record::Refused));
    assert_eq!(step.delay_ms, 0);
    assert_eq!(fake::with(|h| h.direction[1]) & RELEASED, RELEASED);
}
