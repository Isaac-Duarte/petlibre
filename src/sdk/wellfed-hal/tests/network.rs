//! Stored Wi-Fi credentials: the request queue, and the join
//! half's choice between stored and build credentials with fallback.

use std::sync::Mutex;

use wellfed_core::store::net::{Credentials, NetStore};
use wellfed_core::transport::{WifiResult, WifiSource};
use wellfed_hal::network::*;
use wellfed_hal::sdk::fake;

/// The module's state is process-wide.
static SERIAL: Mutex<()> = Mutex::new(());

const BUILD_SSID: &[u8] = b"build-net";
const BUILD_PSK: &[u8] = b"build-pass";

fn select() -> (Vec<u8>, Vec<u8>, u32) {
    let mut out = JoinCredentials {
        ssid: BUILD_SSID.as_ptr(),
        ssid_len: BUILD_SSID.len() as u32,
        psk: BUILD_PSK.as_ptr(),
        psk_len: BUILD_PSK.len() as u32,
        source: 0,
    };
    // SAFETY: `out` is live; the test is the only Wi-Fi task.
    unsafe { wellfed_wifi_select(&mut out) };
    // SAFETY: the pointers name the build's constants or the static credentials.
    let ssid = unsafe { std::slice::from_raw_parts(out.ssid, out.ssid_len as usize) }.to_vec();
    // SAFETY: the shim view points into the static join state.
    let psk = unsafe { std::slice::from_raw_parts(out.psk, out.psk_len as usize) }.to_vec();
    (ssid, psk, out.source)
}

/// The C Wi-Fi task's report after a join attempt.
fn outcome(joined: i32) {
    // SAFETY: the test is the only Wi-Fi task, and no select is running.
    unsafe { wellfed_wifi_outcome(joined) };
}

/// The module's state forgotten, under `SERIAL`.
fn forget() {
    // SAFETY: the test is the only Wi-Fi task.
    unsafe { reset() };
}

/// Write a credential record into the fake storage, as the writer would.
fn store(credentials: Option<Credentials>) {
    use wellfed_core::store::slot::SlotIo;

    struct FakeNet;

    impl SlotIo for FakeNet {
        fn unit_bytes(&self) -> u32 {
            wellfed_hal::sdk::SECTOR_BYTES
        }
        fn read(&self, slot: u32, offset: u32, bytes: &mut [u8]) -> bool {
            wellfed_hal::sdk::storage_read(NET_BASE + slot, offset, bytes)
        }
        fn program(&self, slot: u32, offset: u32, bytes: &[u8]) -> bool {
            // SAFETY: the fake's storage, which this test owns.
            unsafe { wellfed_hal::sdk::storage_program(NET_BASE + slot, offset, bytes) }
        }
        fn erase(&self, slot: u32) -> bool {
            // SAFETY: as in `program`.
            unsafe { wellfed_hal::sdk::storage_erase(NET_BASE + slot) }
        }
    }

    let mut s = NetStore::default();
    s.open(FakeNet);
    assert!(s.save(credentials.as_ref()));
    publish(&s, true);
}

#[test]
fn the_fallback_rule() {
    assert_eq!(choose(false, 99, false), WifiSource::Build);
    assert_eq!(choose(true, FALLBACK_AFTER - 1, false), WifiSource::Stored);
    assert_eq!(choose(true, FALLBACK_AFTER, false), WifiSource::Fallback);
    assert_eq!(
        choose(true, 99, true),
        WifiSource::Stored,
        "joined once: just a lost network"
    );
}

#[test]
fn joins_use_stored_credentials_until_they_keep_failing() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    fake::reset_hardware();
    forget();
    assert_eq!(
        select(),
        (BUILD_SSID.to_vec(), BUILD_PSK.to_vec(), 0),
        "nothing stored"
    );

    forget();
    store(Some(
        Credentials::new(b"stored-net", b"stored-pass").unwrap(),
    ));
    for attempt in 0..FALLBACK_AFTER {
        assert_eq!(
            select(),
            (b"stored-net".to_vec(), b"stored-pass".to_vec(), 1),
            "attempt {attempt}"
        );
        outcome(0);
    }
    assert_eq!(select(), (BUILD_SSID.to_vec(), BUILD_PSK.to_vec(), 2));
    assert_eq!(source(), WifiSource::Fallback);

    // Joined once this boot: later failures keep the stored network.
    forget();
    select();
    outcome(1);
    for _ in 0..2 * FALLBACK_AFTER {
        assert_eq!(select().2, 1);
        outcome(0);
    }
}

#[test]
fn a_save_mid_boot_is_joined_with_a_fresh_count() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    fake::reset_hardware();
    forget();
    store(Some(Credentials::new(b"old-net", b"old-pass1").unwrap()));
    for _ in 0..FALLBACK_AFTER {
        select();
        outcome(0);
    }
    assert_eq!(select().2, 2, "fallen back to the build's");

    // BLE setup (or the device API) saves new credentials while offline.
    store(Some(Credentials::new(b"new-net", b"new-pass1").unwrap()));
    for attempt in 0..FALLBACK_AFTER {
        assert_eq!(
            select(),
            (b"new-net".to_vec(), b"new-pass1".to_vec(), 1),
            "attempt {attempt}"
        );
        outcome(0);
    }
    assert_eq!(select().2, 2, "the new ones can still fall back");
}

#[test]
fn changes_queue_checked_credentials_or_a_clear() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    fake::reset_hardware();
    forget();
    assert_eq!(current(), WifiResult::Busy, "store not opened");
    assert_eq!(change(None), WifiResult::Busy);
    store(None);

    let read = current();
    assert!(
        matches!(read, WifiResult::Current { pending: false, view } if view.stored_ssid_crc.is_none() && view.writable)
    );

    let set = Credentials::new(b"net", b"password").unwrap();
    assert!(matches!(
        change(Some(set)),
        WifiResult::Current { pending: true, .. }
    ));
    assert_eq!(change(Some(set)), WifiResult::Busy);
    assert!(matches!(
        current(),
        WifiResult::Current { pending: true, .. }
    ));
    let queued = pending().unwrap().unwrap();
    assert_eq!(
        (queued.ssid(), queued.psk()),
        (&b"net"[..], &b"password"[..])
    );
    finish();

    assert!(matches!(
        change(None),
        WifiResult::Current { pending: true, .. }
    ));
    assert_eq!(pending(), Some(None));
    finish();
}
