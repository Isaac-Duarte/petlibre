//! A client's store write during a feed is refused `Busy` rather than left
//! waiting for the feed's end, past the client's answer timeout.

use std::sync::{Mutex, MutexGuard};

use wellfed_core::store::clients::{Label, Role};
use wellfed_core::store::net::{Credentials, NetStore};
use wellfed_core::store::slot::SlotIo;
use wellfed_core::transport::{SettingsFields, SettingsResult, SettingsView, WifiResult};
use wellfed_feeder::api::KeyChange;
use wellfed_fw_feeder::api::{keys_request, settings_request, wifi_request};
use wellfed_sdk::hal::interlock::Interlock;
use wellfed_sdk::hal::sdk::fake;
use wellfed_sdk::hal::{keys, network, settings};

/// The mailboxes are process-wide.
static SERIAL: Mutex<()> = Mutex::new(());

/// Read-only credential slots, as the join half reads them.
struct FakeNet;

impl SlotIo for FakeNet {
    fn unit_bytes(&self) -> u32 {
        wellfed_sdk::hal::sdk::SECTOR_BYTES
    }
    fn read(&self, slot: u32, offset: u32, bytes: &mut [u8]) -> bool {
        wellfed_sdk::hal::sdk::storage_read(network::NET_BASE + slot, offset, bytes)
    }
    fn program(&self, _: u32, _: u32, _: &[u8]) -> bool {
        false
    }
    fn erase(&self, _: u32) -> bool {
        false
    }
    fn writable(&self) -> bool {
        false
    }
}

/// The stores as the settings writer publishes them once open, writable.
fn start() -> MutexGuard<'static, ()> {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    fake::reset_hardware();
    settings::reset();
    keys::reset();
    // SAFETY: the test holds `SERIAL` and runs no Wi-Fi task.
    unsafe { network::reset() };

    settings::publish(SettingsView {
        lights: true,
        sound: true,
        volume: 50,
        generation: 1,
        writable: true,
    });
    let mut store = NetStore::default();
    store.open(FakeNet);
    network::publish(&store, true);
    guard
}

fn volume(volume: u8) -> SettingsFields {
    SettingsFields {
        volume: Some(volume),
        ..SettingsFields::default()
    }
}

#[test]
fn writes_during_a_feed_are_refused_busy_and_reads_still_answer() {
    let _guard = start();
    let interlock = Interlock::new();
    interlock.claim_feed(u32::MAX).unwrap();
    let credentials = Credentials::new(b"net", b"password").unwrap();
    let revoke = KeyChange::Revoke { key: [7; 32] };

    assert_eq!(
        settings_request(&interlock, volume(7)),
        SettingsResult::Busy
    );
    assert!(matches!(
        settings_request(&interlock, SettingsFields::default()),
        SettingsResult::Current { pending: false, .. }
    ));
    assert_eq!(
        wifi_request(&interlock, Some(Some(credentials))),
        WifiResult::Busy
    );
    assert!(matches!(
        wifi_request(&interlock, None),
        WifiResult::Current { pending: false, .. }
    ));
    assert_eq!(keys_request(&interlock, revoke), None);
    // Nothing was queued for the writer.
    assert_eq!(settings::pending(), None);
    assert!(!network::saving());
    assert_eq!(keys::pending(), None);

    // At rest each is queued for the writer.
    interlock.release_feed();
    assert!(matches!(
        settings_request(&interlock, volume(7)),
        SettingsResult::Current { pending: true, .. }
    ));
    assert!(matches!(
        wifi_request(&interlock, Some(Some(credentials))),
        WifiResult::Current { pending: true, .. }
    ));
    let enroll = KeyChange::Enroll {
        key: [8; 32],
        label: Label::new(b"phone").unwrap(),
        role: Role::Member,
    };
    assert!(keys_request(&interlock, enroll).is_some());
}
