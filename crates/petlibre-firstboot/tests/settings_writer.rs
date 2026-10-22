//! FW25 settings writer against the fake SDK storage, with the hooks already
//! armed (arming itself is covered by the confirmation and updater tests).

use std::cell::Cell;

use petlibre_amebad::arming::Hooks;
use petlibre_amebad::flash::park::Mmio;
use petlibre_amebad::flash::peer::MARKER_REGISTER;
use petlibre_amebad::provider::Provider;
use petlibre_amebad::sdk::{self, fake};
use petlibre_amebad::settings;
use petlibre_core::settings_store::SettingsStatus;
use petlibre_core::transport::{SettingsFields, SettingsResult};
use petlibre_firstboot::probes::flash::{FlashOps, SETTINGS_BASE};
use petlibre_firstboot::settings::{Record, Writer};

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

fn start() -> (Writer, Provider) {
    fake::reset_hardware();
    settings::reset();
    ARMED.with(|a| a.set(true));
    (Writer::default(), Provider::default())
}

fn step(writer: &mut Writer, provider: &mut Provider) -> Option<Record> {
    // SAFETY: the provider outlives the run.
    unsafe { writer.step::<NoMmio, Armed, Armed>(provider) }.record
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
    let (mut writer, mut provider) = start();
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
    assert_eq!((status, generation), (SettingsStatus::EMPTY, 0));
    assert_eq!(current(), (true, true, 50, 0, true));
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
    let (mut writer, mut provider) = start();
    // Garbage in both slots: CORRUPT, defaults, read-only until a reset.
    let base = SETTINGS_BASE as usize * 4096;
    fake::with(|h| h.storage[base..base + 2 * 4096].fill(0x5a));
    let Some(Record::Open { status, .. }) = step(&mut writer, &mut provider) else {
        panic!("open")
    };
    assert_eq!(status, SettingsStatus::CORRUPT);
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
