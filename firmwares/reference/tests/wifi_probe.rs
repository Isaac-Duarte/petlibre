//! FW07 Wi-Fi probe against the fake SDK's event ring: every shim event is
//! logged once and in order, an overwritten event is reported and skipped,
//! and the shim can be started only once.

use petlibre_fw_reference::probes::wifi::{POLL_MS, Probe, Record, Step};
use petlibre_hal::sdk::{WifiEvent, fake};
use petlibre_hal::stopped;

fn start() -> Probe {
    fake::reset_hardware();
    fake::with(|h| h.interrupts = 1);
    assert!(stopped::initialize().verified());
    let mut probe = Probe::default();
    assert_eq!(
        probe.step(),
        Step {
            delay_ms: POLL_MS,
            record: Some(Record::Start { created: true })
        }
    );
    probe
}

fn push(kind: u32, attempt: u32, value: u32) -> WifiEvent {
    fake::with(|h| {
        let event = WifiEvent {
            seq: h.wifi_events.len() as u32 + 1,
            kind,
            attempt,
            value,
            ..WifiEvent::default()
        };
        h.wifi_events.push(event);
        event
    })
}

#[test]
fn every_event_is_logged_once_in_order() {
    let mut probe = start();
    assert_eq!(
        probe.step(),
        Step {
            delay_ms: POLL_MS,
            record: None
        }
    );

    let events = [
        push(1, 0, 0),
        push(2, 1, 1),
        push(3, 1, 0),
        push(4, 1, 0x0a2a_0005),
    ];

    for event in events {
        assert_eq!(
            probe.step(),
            Step {
                delay_ms: 1,
                record: Some(Record::Event {
                    event,
                    stopped: true
                })
            }
        );
    }

    assert_eq!(probe.step().record, None);

    // Later events, after a quiet period.
    let lost = push(5, 1, 60_000);
    assert_eq!(
        probe.step().record,
        Some(Record::Event {
            event: lost,
            stopped: true
        })
    );
}

#[test]
fn an_overwritten_event_is_reported_and_skipped() {
    let mut probe = start();
    fake::with(|h| h.wifi_ring = 2);
    let _first = push(1, 0, 0);
    let second = push(6, 1, 1000);
    let third = push(6, 2, 2000);

    assert_eq!(probe.step().record, Some(Record::Overrun { seq: 1 }));

    assert_eq!(
        probe.step().record,
        Some(Record::Event {
            event: second,
            stopped: true
        })
    );
    assert_eq!(
        probe.step().record,
        Some(Record::Event {
            event: third,
            stopped: true
        })
    );
}

#[test]
fn a_second_start_is_refused() {
    let _ = start();
    let mut again = Probe::default();

    assert_eq!(
        again.step(),
        Step {
            delay_ms: 0,
            record: Some(Record::Start { created: false })
        }
    );
    assert_eq!(again.step().record, None);
}
