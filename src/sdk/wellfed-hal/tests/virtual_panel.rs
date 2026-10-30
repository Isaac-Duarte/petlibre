//! The virtual panel against the fake SDK: a line typed on the log UART
//! holds its button through the same engine as a finger, and a line that
//! lost bytes to an overrun presses nothing.
#![cfg(feature = "virtual-panel")]

use std::sync::{Mutex, MutexGuard};

use wellfed_core::buttons::{Button, Buttons, Config, Gesture};
use wellfed_core::virtual_panel::{Line, Press};
use wellfed_hal::buttons::{self, POLL_MS, Record, Service};
use wellfed_hal::sdk::fake;

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
    let mut service = Service::new(Config::BENCH);
    service.step();
    (guard, service)
}

fn type_line(text: &[u8]) {
    fake::with(|h| h.log_uart_rx.extend(text));
}

fn run(service: &mut Service, ms: u32) -> Vec<Record> {
    let mut records = Vec::new();
    for _ in 0..ms / POLL_MS {
        fake::with(|h| h.uptime_ms += POLL_MS);
        records.extend(service.step().record);
    }
    records
}

#[test]
fn a_typed_press_is_a_finger_s_press() {
    let (_guard, mut service) = start();

    type_line(b"press dispense\n");
    let records = run(&mut service, 400);

    let dispense = Buttons::of(Button::Dispense);
    assert_eq!(
        records.first(),
        Some(&Record::Virtual(Line::Press(Press {
            chord: dispense,
            hold_ms: 150
        })))
    );
    let gestures: Vec<Gesture> = records
        .iter()
        .filter_map(|record| match record {
            Record::Gesture { gesture, .. } => Some(*gesture),
            _ => None,
        })
        .collect();
    assert!(matches!(
        gestures.as_slice(),
        [Gesture::Down { chord }, Gesture::Up { chord: up, .. }] if *chord == dispense && *up == dispense
    ));
    assert_eq!(buttons::next(), Some(gestures[0]));
}

#[test]
fn a_chord_holds_its_buttons_together() {
    let (_guard, mut service) = start();

    type_line(b"\npress lock+dispense hold 300\n");
    let records = run(&mut service, 500);

    let chord = Buttons::of(Button::Lock).with(Button::Dispense);
    assert!(records.iter().any(|record| matches!(
        record,
        Record::Gesture { gesture: Gesture::Down { chord: down }, .. } if *down == chord
    )));
}

#[test]
fn an_overrun_reported_on_the_leading_line_end_refuses_the_line_behind_it() {
    let (_guard, mut service) = start();

    // The FIFO kept the line end and the line's start; the rest was lost,
    // and the UART flags it with the oldest byte, the line end.
    fake::with(|h| h.log_uart_overrun = true);
    type_line(b"\npress dis");
    type_line(b"pense\n");
    let records = run(&mut service, 400);

    assert_eq!(records, [Record::Virtual(Line::Refused)]);
    assert_eq!(buttons::next(), None);
}

#[test]
fn a_line_that_overran_presses_nothing() {
    let (_guard, mut service) = start();

    type_line(b"press dis");
    fake::with(|h| h.log_uart_overrun = true);
    type_line(b"pense\n");
    let records = run(&mut service, 400);

    assert_eq!(records, [Record::Virtual(Line::Refused)]);
    assert_eq!(buttons::next(), None);
}
