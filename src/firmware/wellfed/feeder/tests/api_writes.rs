//! A client's store write during a feed is refused `Busy` rather than left
//! waiting for the feed's end, past the client's answer timeout.

use std::sync::{Mutex, MutexGuard};

use wellfed_core::store::slot::SlotIo;
use wellfed_feeder::api::KeyChange;
use wellfed_feeder::mailbox::{SettingsFields, SettingsResult, SettingsView, WifiResult};
use wellfed_feeder::store::clients::{Label, Role};
use wellfed_feeder::store::net::{Credentials, NetStore};
use wellfed_fw_feeder::api::{keys_request, settings_request, wifi_request};
use wellfed_fw_feeder::feeding::{claim_outputs, writes_queued};
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

/// A client's change queued just before a feed is written before the feed
/// starts, so its answer comes before the feed rather than after it.
#[test]
fn a_change_queued_just_before_a_feed_goes_first() {
    use std::cell::Cell;

    use wellfed_sdk::hal::interlock::WRITE_WAIT_MS;

    fn queue_settings(interlock: &Interlock) {
        assert!(matches!(
            settings_request(interlock, volume(7)),
            SettingsResult::Current { pending: true, .. }
        ));
    }
    fn queue_wifi(interlock: &Interlock) {
        let credentials = Credentials::new(b"net", b"password").unwrap();
        assert!(matches!(
            wifi_request(interlock, Some(Some(credentials))),
            WifiResult::Current { pending: true, .. }
        ));
    }
    fn queue_revoke(interlock: &Interlock) {
        assert!(keys_request(interlock, KeyChange::Revoke { key: [7; 32] }).is_some());
    }

    for queue in [queue_settings, queue_wifi, queue_revoke] {
        let _guard = start();
        let interlock = Interlock::new();
        queue(&interlock);
        assert!(writes_queued());

        // The settings writer takes it on its next poll and finishes it.
        let now = Cell::new(0_u32);
        let claimed = claim_outputs(
            &interlock,
            writes_queued,
            || now.get(),
            |ms| {
                now.set(now.get() + ms);
                if now.get() == 200 {
                    settings::finish();
                    network::finish();
                    keys::finish(keys::KeyResult::Done);
                }
            },
            |_| unreachable!(),
        );
        assert_eq!(claimed, Ok(()));
        assert_eq!(now.get(), 200, "claimed once the change was written");
        assert!(now.get() < WRITE_WAIT_MS);
    }
}
