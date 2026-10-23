//! FW34 BLE setup against the fake peripheral: the feeder advertises only
//! after two minutes offline, answers one signed `wifi` request per hello
//! with the transport's own reply, and queues the credentials for the
//! settings writer; anything unsigned, replayed or not `wifi` changes nothing.

use std::sync::{Mutex, MutexGuard};

use wellfed_core::ble_setup::{OFFLINE_MS, PREFIX, parse_prefix, prefix};
use wellfed_core::request_auth::hmac_sha256;
use wellfed_core::store::net::NetStore;
use wellfed_core::store::slot::JournalIo;
use wellfed_hal::ble::{BleSetup, DropReason, FailStage, Record, RequestKind, SETUP};
use wellfed_hal::sdk::fake::api::FAKE_REQUEST_KEY;
use wellfed_hal::sdk::fake::{self, ble_record};
use wellfed_hal::sdk::{Advertising, BleEvent, WifiEvent};
use wellfed_hal::{net_status, network};

/// The network queue and status are process-wide; the fake hardware is per thread.
static SERIAL: Mutex<()> = Mutex::new(());

/// A started, ready setup task with an empty, writable credential store.
fn start() -> (MutexGuard<'static, ()>, BleSetup) {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    fake::reset_hardware();
    network::reset();
    net_status::reset();
    open_store();

    let mut setup = BleSetup::default();
    assert_eq!(setup.step().record, None);
    assert_eq!(fake::with(|h| h.ble_started), Some(SETUP));
    ble_record(BleEvent::READY, 0, 0);
    assert!(matches!(setup.step().record, Some(Record::Ready { .. })));
    (guard, setup)
}

fn open_store() {
    use std::ffi::{c_uint, c_void};
    unsafe extern "C" fn read(
        _: *mut c_void,
        slot: c_uint,
        offset: u32,
        bytes: *mut c_void,
        size: usize,
    ) -> bool {
        // SAFETY: the store passes a live buffer of `size` bytes.
        let out = unsafe { std::slice::from_raw_parts_mut(bytes.cast::<u8>(), size) };
        wellfed_hal::sdk::storage_read(network::NET_BASE + slot, offset, out)
    }
    let io = JournalIo {
        context: std::ptr::null_mut(),
        unit_bytes: wellfed_hal::sdk::SECTOR_BYTES,
        read: Some(read),
        program: None,
        erase: None,
    };
    let mut store = NetStore::default();
    // SAFETY: a static callback over the fake storage.
    unsafe { store.open(io) };
    network::publish(&store, true);
}

fn wait(setup: &mut BleSetup, ms: u32) {
    let end = fake::with(|h| h.uptime_ms) + ms;
    while fake::with(|h| h.uptime_ms) < end {
        let step = setup.step();
        fake::with(|h| h.uptime_ms += step.delay_ms);
    }
}

fn online() {
    net_status::observe(&WifiEvent {
        kind: WifiEvent::DNS,
        value: 0x0a2a_6701,
        ..WifiEvent::default()
    });
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

/// Connect, enable notifications and return the hello's nonce.
fn connect(setup: &mut BleSetup) -> String {
    ble_record(BleEvent::CONNECT, 0, 12);
    assert_eq!(setup.step().record, Some(Record::Connect { interval: 12 }));
    ble_record(BleEvent::NOTIFY, 0, 1);
    assert_eq!(setup.step().record, None);

    let hello = String::from_utf8(notified().remove(0)).unwrap();
    let nonce = hello
        .strip_prefix(r#"{"op":"hello","nonce":""#)
        .and_then(|rest| rest.strip_suffix(r#""}"#))
        .unwrap();
    nonce.to_string()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A request as the server signs it, `fields` between the op and the nonce.
fn signed(key: &[u8; 32], op: &str, fields: &str, nonce: &str, seq: u32) -> Vec<u8> {
    let prefix = format!(
        r#"{{"id":"0f8fad5b-d9cb-469f-a165-70867728950e","op":"{op}"{fields},"nonce":"{nonce}","seq":{seq}"#
    );
    let mac = hmac_sha256(key, &[prefix.as_bytes()]);
    format!(r#"{prefix},"mac":"{}"}}"#, hex(&mac)).into_bytes()
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

/// Step until a record arrives.
fn record(setup: &mut BleSetup) -> Record {
    for _ in 0..20 {
        let step = setup.step();
        fake::with(|h| h.uptime_ms += step.delay_ms);
        if let Some(record) = step.record {
            return record;
        }
    }
    panic!("no record");
}

const CREDENTIALS: &str = r#","ssid":"6e6574","psk":"70617373776f7264""#;

#[test]
fn advertising_waits_for_two_minutes_offline_and_stops_online() {
    let (_guard, mut setup) = start();
    wait(&mut setup, OFFLINE_MS - 1_000);
    assert_eq!(fake::with(|h| h.ble_advertising), Advertising::Off);

    wait(&mut setup, 2_000);
    assert_eq!(fake::with(|h| h.ble_advertising), Advertising::On);

    online();
    wait(&mut setup, 1_000);
    assert_eq!(fake::with(|h| h.ble_advertising), Advertising::Off);
}

#[test]
fn a_signed_wifi_request_queues_the_credentials() {
    let (_guard, mut setup) = start();
    let nonce = connect(&mut setup);
    write(&signed(&FAKE_REQUEST_KEY, "wifi", CREDENTIALS, &nonce, 1));

    assert_eq!(
        record(&mut setup),
        Record::Request {
            kind: RequestKind::Wifi,
            ok: true
        }
    );
    let reply = String::from_utf8(notified().remove(0)).unwrap();
    assert!(
        reply.contains(r#""id":"0f8fad5b-d9cb-469f-a165-70867728950e""#)
            && reply.contains(r#""pending":true"#),
        "{reply}"
    );
    assert!(
        !reply.contains("70617373"),
        "the passphrase is never echoed"
    );
    let queued = network::pending().unwrap().unwrap();
    assert_eq!(
        (queued.ssid(), queued.psk()),
        (&b"net"[..], &b"password"[..])
    );
}

#[test]
fn wrong_keys_replays_and_other_ops_change_nothing() {
    let (_guard, mut setup) = start();

    let nonce = connect(&mut setup);
    write(&signed(&[1; 32], "wifi", CREDENTIALS, &nonce, 1));
    assert_eq!(
        record(&mut setup),
        Record::Request {
            kind: RequestKind::Unauthenticated,
            ok: false
        }
    );
    assert!(
        String::from_utf8(notified().remove(0))
            .unwrap()
            .contains(r#""error":"auth""#)
    );

    // A good request from an earlier hello is refused under the new nonce.
    let stale = signed(&FAKE_REQUEST_KEY, "wifi", CREDENTIALS, &nonce, 1);
    ble_record(BleEvent::NOTIFY, 0, 1);
    setup.step();
    notified();
    write(&stale);
    assert_eq!(
        record(&mut setup),
        Record::Request {
            kind: RequestKind::Unauthenticated,
            ok: false
        }
    );
    notified();

    ble_record(BleEvent::NOTIFY, 0, 1);
    setup.step();
    let fresh = String::from_utf8(notified().remove(0)).unwrap();
    let nonce = &fresh[23..39];
    write(&signed(
        &FAKE_REQUEST_KEY,
        "settings",
        r#","lights":0"#,
        nonce,
        1,
    ));
    assert_eq!(
        record(&mut setup),
        Record::Request {
            kind: RequestKind::Unsupported,
            ok: false
        }
    );
    assert_eq!(network::pending(), None);
}

#[test]
fn unsigned_or_oversized_messages_drop_the_link() {
    let (_guard, mut setup) = start();
    connect(&mut setup);
    write(br#"{"id":"r1","op":"wifi","clear":1}"#);
    assert_eq!(
        record(&mut setup),
        Record::Drop {
            reason: DropReason::Malformed
        }
    );
    assert_eq!(fake::with(|h| h.ble_disconnects), 1);

    ble_record(BleEvent::DISCONNECT, 0x13, 0);
    setup.step();
    connect(&mut setup);
    fake::with(|h| h.ble_control.extend([0xff, 0xff]));
    assert_eq!(
        record(&mut setup),
        Record::Drop {
            reason: DropReason::TooLong
        }
    );
}

#[test]
fn a_radio_that_never_comes_up_is_reported_once() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    fake::reset_hardware();
    let mut setup = BleSetup::default();
    setup.step();
    ble_record(BleEvent::READY, BleEvent::READY_WIFI, 0);
    assert_eq!(
        setup.step().record,
        Some(Record::Failed {
            stage: FailStage::Radio
        })
    );
    let step = setup.step();
    assert_eq!((step.record, step.delay_ms > 1_000), (None, true));
}
