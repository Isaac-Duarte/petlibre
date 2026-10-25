//! Settings writer against the fake SDK storage, with the hooks already
//! armed (arming itself is covered by the confirmation and updater tests).

use std::cell::Cell;
use std::sync::{Mutex, MutexGuard};

use wellfed_core::store::clients::{ClientStatus, Label, Role};
use wellfed_core::store::events::Read;
use wellfed_core::store::seed::SeedStatus;
use wellfed_core::store::settings::SettingsStatus;
use wellfed_core::transport::{SettingsFields, SettingsResult};
use wellfed_hal::flash::park::Mmio;
use wellfed_hal::flash::peer::MARKER_REGISTER;
use wellfed_hal::flash_lock::arming::Hooks;
use wellfed_hal::flash_lock::provider::Provider;
use wellfed_hal::keys::{self, Change, Requested};
use wellfed_hal::rng;
use wellfed_hal::sdk::{self, fake};
use wellfed_hal::{events, network, settings};
use wellfed_sdk::settings::{
    self as writer, ErrorStage, Identity, KeyChange, KeyResult, Record, Writer,
};
use wellfed_sdk::storage::{FlashOps, SETTINGS_BASE};

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(true) };
    static ATTEMPT_SEEN: Cell<bool> = const { Cell::new(false) };
}

struct NoMmio;

impl Mmio for NoMmio {
    const KM0: bool = false;
    fn load(_: u32) -> u32 {
        unreachable!("no arming with the hooks armed")
    }
    fn store(_: u32, _: u32) {
        unreachable!("no arming with the hooks armed")
    }
    fn barrier() {}
    fn mask() -> u32 {
        0
    }
    fn halt() -> ! {
        panic!("halt")
    }
}

struct Armed;

impl Hooks for Armed {
    fn watchdog() -> bool {
        true
    }
    fn install(_: *mut Provider) -> bool {
        true
    }
    fn armed() -> bool {
        ARMED.with(Cell::get)
    }
}

impl FlashOps for Armed {
    fn install(_: *mut Provider) -> bool {
        true
    }
    fn read_id() -> [u8; 3] {
        [0; 3]
    }
    fn armed() -> bool {
        ARMED.with(Cell::get)
    }
    fn erase(sector: u32) -> bool {
        // Every write runs inside a marked attempt.
        ATTEMPT_SEEN.with(|s| s.set(sdk::backup_read(MARKER_REGISTER) != 0));
        sdk::storage_erase(sector)
    }
    fn program(sector: u32, offset: u32, data: &[u8]) -> bool {
        sdk::storage_program(sector, offset, data)
    }
    fn read(sector: u32, offset: u32, bytes: &mut [u8]) -> bool {
        sdk::storage_read(sector, offset, bytes)
    }
}

/// The settings and network views are process-wide; the fake hardware is per thread.
static SERIAL: Mutex<()> = Mutex::new(());

fn start() -> (MutexGuard<'static, ()>, Writer<Armed>, Provider) {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    fake::reset_hardware();
    settings::reset();
    network::reset();
    keys::reset();
    events::reset();
    ARMED.with(|a| a.set(true));
    (guard, Writer::default(), Provider::default())
}

fn step(writer: &mut Writer<Armed>, provider: &mut Provider) -> Option<Record> {
    // SAFETY: the provider outlives the run.
    unsafe { writer.step::<NoMmio, Armed>(provider) }.record
}

fn current() -> (bool, bool, u8, u32, bool) {
    match settings::request(SettingsFields::default()) {
        SettingsResult::Current { view, .. } => (
            view.lights,
            view.sound,
            view.volume,
            view.generation,
            view.writable,
        ),
        other => panic!("{other:?}"),
    }
}

#[test]
fn changes_are_saved_once_each_inside_a_marked_attempt() {
    let (_guard, mut writer, mut provider) = start();
    assert_eq!(
        settings::request(SettingsFields::default()),
        SettingsResult::Busy,
        "nothing published before the store opens"
    );

    let Some(Record::Open {
        status, generation, ..
    }) = step(&mut writer, &mut provider)
    else {
        panic!("open first")
    };
    assert_eq!((status, generation), (SettingsStatus::Empty, 0));
    assert_eq!(current(), (true, true, 50, 0, true));
    assert!(matches!(
        step(&mut writer, &mut provider),
        Some(Record::NetOpen { stored: false, .. })
    ));
    assert!(matches!(
        step(&mut writer, &mut provider),
        Some(Record::Keys { .. })
    ));
    assert_eq!(
        step(&mut writer, &mut provider),
        Some(Record::Events {
            open: true,
            last: None,
            oldest: None,
            epoch: events::epoch().unwrap(),
        }),
        "the event ring opens empty"
    );
    assert_eq!(
        step(&mut writer, &mut provider),
        None,
        "idle without a change"
    );

    let change = SettingsFields {
        lights: Some(false),
        volume: Some(20),
        ..Default::default()
    };
    assert!(matches!(
        settings::request(change),
        SettingsResult::Current { pending: true, .. }
    ));
    assert_eq!(
        settings::request(change),
        SettingsResult::Busy,
        "one change at a time"
    );

    let Some(Record::Saved {
        generation,
        settings: saved,
    }) = step(&mut writer, &mut provider)
    else {
        panic!("saved")
    };
    assert_eq!(
        (generation, saved.lights, saved.sound, saved.volume),
        (1, false, true, 20)
    );
    assert!(ATTEMPT_SEEN.with(Cell::get));
    assert_eq!(
        sdk::backup_read(MARKER_REGISTER),
        0,
        "the attempt is cleared after"
    );
    assert_eq!(current(), (false, true, 20, 1, true));
    assert_eq!(settings::pending(), None);

    let base = SETTINGS_BASE as usize * 4096;
    assert_eq!(
        fake::with(|h| h.storage[base..base + 8].to_vec()),
        b"PLFSET01"
    );

    // The next change goes to the other slot; a reopen sees it.
    settings::request(SettingsFields {
        sound: Some(false),
        ..Default::default()
    });
    assert!(matches!(
        step(&mut writer, &mut provider),
        Some(Record::Saved { generation: 2, .. })
    ));
    let mut reopened = Writer::default();
    settings::reset();
    assert!(matches!(
        step(&mut reopened, &mut provider),
        Some(Record::Open { generation: 2, .. })
    ));
    assert_eq!(current(), (false, false, 20, 2, true));
}

#[test]
fn an_unwritable_store_reads_but_refuses_changes() {
    let (_guard, mut writer, mut provider) = start();
    // Garbage in both slots: CORRUPT, defaults, read-only until a reset.
    let base = SETTINGS_BASE as usize * 4096;
    fake::with(|h| h.storage[base..base + 2 * 4096].fill(0x5a));
    let Some(Record::Open { status, .. }) = step(&mut writer, &mut provider) else {
        panic!("open")
    };
    assert_eq!(status, SettingsStatus::Corrupt);
    assert!(!current().4);
    assert_eq!(
        settings::request(SettingsFields {
            lights: Some(false),
            ..Default::default()
        }),
        SettingsResult::ReadOnly
    );
    assert_eq!(fake::with(|h| h.storage[base]), 0x5a, "nothing written");
}

#[test]
fn wifi_credentials_are_saved_inside_a_marked_attempt_and_cleared() {
    use wellfed_core::store::net::Credentials;
    use wellfed_core::transport::WifiResult;
    let (_guard, mut writer, mut provider) = start();
    step(&mut writer, &mut provider); // open
    step(&mut writer, &mut provider); // net open
    step(&mut writer, &mut provider); // keys
    step(&mut writer, &mut provider); // events

    let set = Credentials::new(b"net", b"password").unwrap();
    assert!(matches!(
        network::change(Some(set)),
        WifiResult::Current { pending: true, .. }
    ));
    assert_eq!(
        step(&mut writer, &mut provider),
        Some(Record::NetSaved {
            generation: 1,
            stored: true
        })
    );
    assert!(ATTEMPT_SEEN.with(Cell::get));
    assert_eq!(sdk::backup_read(MARKER_REGISTER), 0);
    assert!(matches!(
        network::current(),
        WifiResult::Current { view, pending: false } if view.stored_ssid_crc.is_some()
    ));

    network::change(None);
    assert_eq!(
        step(&mut writer, &mut provider),
        Some(Record::NetSaved {
            generation: 2,
            stored: false
        })
    );
}

/// The `PLCFG1` error words, as they were when the stage was a string.
#[test]
fn error_records_format_as_before() {
    let words: Vec<_> = ErrorStage::ALL.iter().map(|s| s.as_str()).collect();
    assert_eq!(words, ["unarmed", "save"]);

    let mut text = String::new();
    writer::describe(
        Record::Error {
            stage: ErrorStage::Save,
            status: SettingsStatus::Corrupt,
        },
        &mut text,
    )
    .unwrap();
    assert_eq!(
        text,
        format!("error stage=save status={}", SettingsStatus::Corrupt.code())
    );

    text.clear();
    writer::describe(
        Record::NetError {
            stage: ErrorStage::Unarmed,
        },
        &mut text,
    )
    .unwrap();
    assert_eq!(text, "net error stage=unarmed");
}

/// The writer's first three records on one boot: open, net open, then keys.
fn boot_keys() -> Record {
    let (mut writer, mut provider) = (Writer::default(), Provider::default());
    step(&mut writer, &mut provider);
    step(&mut writer, &mut provider);
    step(&mut writer, &mut provider).expect("the keys record")
}

#[test]
fn keys_carry_the_seed_and_create_the_identity_once() {
    let (_guard, _, _) = start();
    rng::reset();
    assert_eq!(
        boot_keys(),
        Record::Keys {
            seed: SeedStatus::Empty,
            mixed: false,
            saved: false,
            clients: ClientStatus::Empty,
            enrolled: 0,
            identity: Identity::Missing,
        },
        "no generator: nothing written"
    );

    fake::with(|h| h.adc_noise = 0x2468_ace1);
    assert_eq!(rng::start().result, Ok(()));
    let first = boot_keys();
    assert!(matches!(
        first,
        Record::Keys {
            seed: SeedStatus::Empty,
            mixed: false,
            saved: true,
            identity: Identity::Created,
            ..
        }
    ));
    assert!(ATTEMPT_SEEN.with(Cell::get));
    assert_eq!(sdk::backup_read(MARKER_REGISTER), 0);

    // The next boot finds both, and stores another seed.
    assert!(matches!(
        boot_keys(),
        Record::Keys {
            seed: SeedStatus::Loaded,
            mixed: true,
            saved: true,
            clients: ClientStatus::Loaded,
            identity: Identity::Loaded,
            ..
        }
    ));
}

/// One queued key change, applied on the writer's next step.
fn change(writer: &mut Writer<Armed>, provider: &mut Provider, change: Change) -> Record {
    assert_eq!(keys::request(change), Requested::Queued);
    let record = step(writer, provider).expect("the change record");
    assert_eq!(keys::pending(), None, "the slot is free again");
    record
}

fn changed(change: KeyChange, result: KeyResult, enrolled: usize) -> Record {
    Record::KeysChanged {
        change,
        result,
        enrolled,
    }
}

#[test]
fn key_changes_apply_and_a_reset_publishes_a_new_identity() {
    let (_guard, mut writer, mut provider) = start();
    fake::with(|h| h.adc_noise = 0x1357_9bdf);
    assert_eq!(rng::start().result, Ok(()));
    for _ in 0..4 {
        step(&mut writer, &mut provider); // open, net open, keys, events
    }
    let (first, identity) = keys::identity_since(0).expect("the identity is published");

    let phone = Change::Enroll {
        key: [7; 32],
        label: Label::new(b"phone").unwrap(),
        role: Role::Owner,
    };
    assert_eq!(
        change(&mut writer, &mut provider, phone),
        changed(KeyChange::Enroll, KeyResult::Done, 1)
    );
    let enrolled = keys::ticket();
    assert_eq!(keys::outcome(enrolled), Some(KeyResult::Done));
    assert!(ATTEMPT_SEEN.with(Cell::get));
    assert_eq!(sdk::backup_read(MARKER_REGISTER), 0);
    assert_eq!(
        change(&mut writer, &mut provider, phone),
        changed(KeyChange::Enroll, KeyResult::Duplicate, 1)
    );
    // A caller holding the earlier ticket no longer sees an outcome.
    assert_eq!(keys::outcome(enrolled), None);
    assert_eq!(keys::outcome(keys::ticket()), Some(KeyResult::Duplicate));
    assert_eq!(
        keys::identity_since(first),
        None,
        "an enrollment keeps the identity"
    );

    let revoke = Change::Revoke { key: [8; 32] };
    assert_eq!(
        change(&mut writer, &mut provider, revoke),
        changed(KeyChange::Revoke, KeyResult::NotFound, 1)
    );

    assert_eq!(
        change(&mut writer, &mut provider, Change::Reset),
        changed(KeyChange::Reset, KeyResult::Done, 0)
    );
    let (_, new) = keys::identity_since(first).expect("the new identity");
    assert_ne!(new, identity);

    // The next boot loads the reset table.
    keys::reset();
    assert!(matches!(
        boot_keys(),
        Record::Keys {
            enrolled: 0,
            identity: Identity::Loaded,
            ..
        }
    ));
    assert_eq!(keys::identity_since(0).map(|(_, secret)| secret), Some(new));
}

#[test]
fn a_reset_needs_the_generator_and_an_armed_writer() {
    let (_guard, mut writer, mut provider) = start();
    fake::with(|h| h.adc_noise = 0x0bad_cafe);
    assert_eq!(rng::start().result, Ok(()));
    for _ in 0..4 {
        step(&mut writer, &mut provider);
    }
    let (first, _) = keys::identity_since(0).unwrap();

    rng::reset();
    assert_eq!(
        change(&mut writer, &mut provider, Change::Reset),
        changed(KeyChange::Reset, KeyResult::NoGenerator, 0)
    );

    assert_eq!(rng::start().result, Ok(()));
    ARMED.with(|a| a.set(false));
    assert_eq!(
        change(&mut writer, &mut provider, Change::Reset),
        changed(KeyChange::Reset, KeyResult::Unarmed, 0)
    );
    assert_eq!(keys::identity_since(first), None, "the identity stands");
}

#[test]
fn key_change_records_format() {
    let mut text = String::new();
    writer::describe(
        changed(KeyChange::Reset, KeyResult::NoGenerator, 0),
        &mut text,
    )
    .unwrap();
    assert_eq!(text, "keys change=reset result=no-generator enrolled=0");
}

/// The loop's queued events reach the ring, survive a reboot, and
/// come back through the read mailbox in order.
#[test]
fn queued_events_are_appended_and_read_back_after_a_reboot() {
    let (_guard, mut writer, mut provider) = start();
    for _ in 0..4 {
        step(&mut writer, &mut provider); // open, net open, keys, events
    }
    assert!(events::record(Some(1_700_000_000), [1; 16]));
    assert!(events::record(None, [2; 16]));
    step(&mut writer, &mut provider);
    step(&mut writer, &mut provider);
    assert_eq!(events::last_seq(), Some(2));

    // The next boot finds both; a reader asks, the writer answers.
    events::reset();
    let mut writer = Writer::default();
    for _ in 0..3 {
        step(&mut writer, &mut provider);
    }
    assert_eq!(
        step(&mut writer, &mut provider),
        Some(Record::Events {
            open: true,
            last: Some(2),
            oldest: Some(1),
            epoch: events::epoch().unwrap(),
        })
    );
    assert_eq!(events::read_result(None), None, "asked");
    step(&mut writer, &mut provider);
    let Some(Read::Record(first)) = events::read_result(None) else {
        panic!("the first event");
    };
    assert_eq!(
        (first.seq, first.at, first.body),
        (1, Some(1_700_000_000), [1; 16])
    );
    assert_eq!(events::read_result(Some(1)), None);
    step(&mut writer, &mut provider);
    assert!(matches!(
        events::read_result(Some(1)),
        Some(Read::Record(r)) if r.seq == 2
    ));

    // The end of the ring is current only until the next append.
    assert_eq!(events::read_result(Some(2)), None);
    step(&mut writer, &mut provider);
    assert_eq!(events::read_result(Some(2)), Some(Read::End));
    assert!(events::record(None, [3; 16]));
    step(&mut writer, &mut provider);
    assert_eq!(
        events::read_result(Some(2)),
        None,
        "stale once a record lands"
    );
}
