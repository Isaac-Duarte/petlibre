//! FW45 setup mode's BLE link against the fake peripheral: it advertises
//! only while asked and nobody is connected, reassembles a host's framed
//! writes into one held message, frames what the firmware sends, and drops
//! a host that overruns the frame limit or outstays the session.

use wellfed_core::ble_setup::{MAX_MESSAGE, PREFIX, parse_prefix, prefix};
use wellfed_hal::ble::{BleSetup, DropReason, FailStage, Link, Record, SESSION_MS, SETUP, Step};
use wellfed_hal::sdk::fake::{self, ble_record};
use wellfed_hal::sdk::{Advertising, BleEvent};

/// A started, ready link task.
fn start() -> BleSetup {
    fake::reset_hardware();
    let mut setup = BleSetup::default();
    assert_eq!(setup.step(false).record, None);
    assert_eq!(fake::with(|h| h.ble_started), Some(SETUP));
    ble_record(BleEvent::READY, 0, 0);
    assert!(matches!(
        setup.step(false).record,
        Some(Record::Ready { .. })
    ));
    setup
}

/// Step once, advancing the fake clock by the step's delay.
fn step(setup: &mut BleSetup, advertise: bool) -> Step {
    let step = setup.step(advertise);
    fake::with(|h| h.uptime_ms += step.delay_ms);
    step
}

/// Step until the link reports something.
fn link(setup: &mut BleSetup) -> Link {
    for _ in 0..20 {
        if let Some(link) = step(setup, false).link {
            return link;
        }
    }
    panic!("no link event")
}

fn connect(setup: &mut BleSetup) {
    ble_record(BleEvent::CONNECT, 0, 12);
    assert_eq!(
        setup.step(false).record,
        Some(Record::Connect { interval: 12 })
    );
    ble_record(BleEvent::NOTIFY, 0, 1);
    assert_eq!(link(setup), Link::Opened);
}

fn write(message: &[u8]) {
    let framed: Vec<u8> = prefix(message.len())
        .unwrap()
        .into_iter()
        .chain(message.iter().copied())
        .collect();
    // In pieces, as a host writes them to a 23-byte MTU.
    for piece in framed.chunks(20) {
        fake::with(|h| h.ble_control.extend(piece));
    }
}

/// Every message notified since the last call, unframed.
fn notified() -> Vec<Vec<u8>> {
    let bytes = fake::with(|h| std::mem::take(&mut h.ble_notified));
    let mut out = Vec::new();
    let mut rest = &bytes[..];
    while rest.len() >= PREFIX {
        let len = parse_prefix([rest[0], rest[1]]).unwrap();
        out.push(rest[PREFIX..PREFIX + len].to_vec());
        rest = &rest[PREFIX + len..];
    }
    assert!(rest.is_empty());
    out
}

#[test]
fn it_advertises_only_while_asked_and_unconnected() {
    let mut setup = start();
    step(&mut setup, false);
    assert_eq!(fake::with(|h| h.ble_advertising), Advertising::Off);
    step(&mut setup, true);
    assert_eq!(fake::with(|h| h.ble_advertising), Advertising::On);

    connect(&mut setup);
    step(&mut setup, true);
    assert_eq!(fake::with(|h| h.ble_advertising), Advertising::Off);

    ble_record(BleEvent::DISCONNECT, 0x13, 0);
    assert_eq!(link(&mut setup), Link::Closed);
    step(&mut setup, true);
    assert_eq!(fake::with(|h| h.ble_advertising), Advertising::On);
}

#[test]
fn a_message_in_pieces_is_held_until_consumed() {
    let mut setup = start();
    connect(&mut setup);

    let first = [7u8; 32];
    write(&first);
    write(b"next");
    assert_eq!(link(&mut setup), Link::Message);
    assert_eq!(setup.message(), Some(&first[..]));
    assert_eq!(step(&mut setup, false).link, None, "held, not reread");

    setup.consume();
    assert_eq!(setup.message(), None);
    assert_eq!(link(&mut setup), Link::Message);
    assert_eq!(setup.message(), Some(&b"next"[..]));
}

#[test]
fn replies_go_out_framed_and_only_to_an_open_link() {
    let mut setup = start();
    assert_eq!(setup.send(b"early"), Err(DropReason::Notify));

    connect(&mut setup);
    assert_eq!(setup.send(&[1; 96]), Ok(()));
    assert_eq!(notified(), vec![vec![1; 96]]);
    assert_eq!(setup.send(&[0; MAX_MESSAGE + 1]), Err(DropReason::TooLong));
}

#[test]
fn an_oversized_frame_or_a_long_session_drops_the_host() {
    let mut setup = start();
    connect(&mut setup);
    fake::with(|h| h.ble_control.extend([0xff, 0xff]));
    let drops = |setup: &mut BleSetup| (0..20).find_map(|_| step(setup, false).record).unwrap();
    assert_eq!(
        drops(&mut setup),
        Record::Drop {
            reason: DropReason::TooLong
        }
    );
    assert_eq!(fake::with(|h| h.ble_disconnects), 1);

    ble_record(BleEvent::DISCONNECT, 0x13, 0);
    assert_eq!(link(&mut setup), Link::Closed);
    connect(&mut setup);
    fake::with(|h| h.uptime_ms += SESSION_MS);
    assert_eq!(
        drops(&mut setup),
        Record::Drop {
            reason: DropReason::Timeout
        }
    );
}

#[test]
fn a_stack_that_never_comes_up_fails() {
    fake::reset_hardware();
    let mut setup = BleSetup::default();
    setup.step(false);
    let failed = (0..400)
        .find_map(|_| step(&mut setup, false).record)
        .unwrap();
    assert_eq!(
        failed,
        Record::Failed {
            stage: FailStage::Timeout
        }
    );
}

#[test]
fn records_format() {
    let mut text = String::new();
    wellfed_hal::ble::describe(
        Record::Drop {
            reason: DropReason::Protocol,
        },
        &mut text,
    )
    .unwrap();
    assert_eq!(text, "drop reason=protocol");
}
