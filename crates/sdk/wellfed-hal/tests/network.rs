//! FW25 stored Wi-Fi credentials: the transport's queue, and the join
//! half's choice between stored and build credentials with fallback.

use std::sync::Mutex;

use wellfed_core::net_store::{Credentials, NetStore};
use wellfed_core::transport::{WifiFields, WifiResult, WifiSource};
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

/// Write a credential record into the fake storage, as the writer would.
fn store(credentials: Option<Credentials>) {
    use wellfed_core::storage::JournalIo;
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
        wellfed_hal::sdk::storage_read(NET_BASE + slot, offset, out)
    }
    unsafe extern "C" fn program(
        _: *mut c_void,
        slot: c_uint,
        offset: u32,
        bytes: *const c_void,
        size: usize,
    ) -> bool {
        // SAFETY: the store passes a live buffer of `size` bytes.
        let data = unsafe { std::slice::from_raw_parts(bytes.cast::<u8>(), size) };
        wellfed_hal::sdk::storage_program(NET_BASE + slot, offset, data)
    }
    unsafe extern "C" fn erase(_: *mut c_void, slot: c_uint) -> bool {
        wellfed_hal::sdk::storage_erase(NET_BASE + slot)
    }
    let io = JournalIo {
        context: std::ptr::null_mut(),
        unit_bytes: wellfed_hal::sdk::SECTOR_BYTES,
        read: Some(read),
        program: Some(program),
        erase: Some(erase),
    };
    let mut s = NetStore::default();
    // SAFETY: static callbacks over the fake storage.
    unsafe { s.open(io) };
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
    reset();
    assert_eq!(
        select(),
        (BUILD_SSID.to_vec(), BUILD_PSK.to_vec(), 0),
        "nothing stored"
    );

    reset();
    store(Some(
        Credentials::new(b"stored-net", b"stored-pass").unwrap(),
    ));
    for attempt in 0..FALLBACK_AFTER {
        assert_eq!(
            select(),
            (b"stored-net".to_vec(), b"stored-pass".to_vec(), 1),
            "attempt {attempt}"
        );
        wellfed_wifi_outcome(0);
    }
    assert_eq!(select(), (BUILD_SSID.to_vec(), BUILD_PSK.to_vec(), 2));
    assert_eq!(source(), WifiSource::Fallback);

    // Joined once this boot: later failures keep the stored network.
    reset();
    select();
    wellfed_wifi_outcome(1);
    for _ in 0..2 * FALLBACK_AFTER {
        assert_eq!(select().2, 1);
        wellfed_wifi_outcome(0);
    }
}

#[test]
fn requests_queue_valid_credentials_or_a_clear() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    fake::reset_hardware();
    reset();
    assert_eq!(
        request(WifiFields::default()),
        WifiResult::Busy,
        "store not opened"
    );
    store(None);

    let read = request(WifiFields::default());
    assert!(
        matches!(read, WifiResult::Current { pending: false, view } if view.stored_ssid_crc.is_none() && view.writable)
    );

    let set = WifiFields {
        ssid: Some("6e6574"),
        psk: Some("70617373776f7264"),
        clear: false,
    };
    assert!(matches!(
        request(set),
        WifiResult::Current { pending: true, .. }
    ));
    assert_eq!(request(set), WifiResult::Busy);
    let queued = pending().unwrap().unwrap();
    assert_eq!(
        (queued.ssid(), queued.psk()),
        (&b"net"[..], &b"password"[..])
    );
    finish();

    // A passphrase with a control character fails the store's own check.
    let bad = WifiFields {
        ssid: Some("6e6574"),
        psk: Some("7061737377006f72"),
        clear: false,
    };
    assert_eq!(request(bad), WifiResult::Invalid);
    assert_eq!(pending(), None);

    let clear = WifiFields {
        clear: true,
        ..WifiFields::default()
    };
    assert!(matches!(
        request(clear),
        WifiResult::Current { pending: true, .. }
    ));
    assert_eq!(pending(), Some(None));
    finish();
}
