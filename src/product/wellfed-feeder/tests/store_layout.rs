//! The on-flash bytes of every record the product's stores write, pinned:
//! settings, Wi-Fi credentials and their clear, the client table, the time
//! settings, the portion calibration, the feeding plans and the event
//! ring's records. These are the layouts deployed feeders hold, so a
//! change to the shared slot layer must leave every byte where it was. The
//! units are larger than the records so the bytes after each record are
//! pinned erased too.
//!
//! The readers are pinned apart from the writers: each store opens the
//! pinned bytes, placed in erased flash, and reads back the values that
//! wrote them.

use wellfed_core::feed::progress::BowlMode;
use wellfed_core::settings::{MealReminder, Window};
use wellfed_device_api::clients::{Client, Label, Role};
use wellfed_device_api::mailbox::{
    Calibration, Claim, NtpServer, PLANS, Plan, PlansChange, TimeSettings, Weekdays, Zone,
};
use wellfed_device_api::wifi::Credentials;
use wellfed_feeder::store::calibration::{CalibrationStatus, CalibrationStore};
use wellfed_feeder::store::clients::{ClientStatus, ClientStore};
use wellfed_feeder::store::events::{BODY_BYTES, EventLog, Read, Record};
use wellfed_feeder::store::net::{NetStatus, NetStore};
use wellfed_feeder::store::plans::{Plans, PlansStatus, PlansStore};
use wellfed_feeder::store::settings::{Settings, SettingsStatus, SettingsStore};
use wellfed_feeder::store::time::{TimeStatus, TimeStore};
use wellfed_testkit::journal_flash::FakeJournalFlash;

/// `bytes` as lowercase hex, 32 bytes a line.
fn lines(bytes: &[u8]) -> Vec<String> {
    bytes
        .chunks(32)
        .map(|line| line.iter().map(|b| format!("{b:02x}")).collect())
        .collect()
}

/// Unit `slot` must hold exactly `golden` (`len` bytes) followed by erased
/// bytes; a mismatch prints the record found in the form this file pins it.
fn pinned(name: &str, f: &FakeJournalFlash, slot: usize, len: usize, golden: &[&str]) {
    let found = lines(&f.bytes[slot][..len]);
    if found != golden {
        let shown: Vec<String> = found
            .iter()
            .map(|l| format!("            \"{l}\","))
            .collect();
        panic!("{name} changed; found:\n{}", shown.join("\n"));
    }
    assert!(
        f.bytes[slot][len..].iter().all(|&b| b == u8::MAX),
        "{name}: bytes after the record"
    );
}

#[test]
fn settings_records() {
    let mut f = FakeJournalFlash::with_unit(512);
    let mut s = SettingsStore::default();
    // SAFETY: the fake flash outlives the store.
    unsafe { s.open(f.io(), 0x5046_5731_3502_0001) };

    assert!(s.save(&Settings::default()));
    pinned("default settings", &f, 0, 128, DEFAULT_SETTINGS);
    assert!(s.save(&Settings {
        bowl_mode: BowlMode::Double,
        lights: false,
        sound: true,
        volume: 100,
        utc_offset_minutes: -570,
        lights_window: Window::Between {
            start: 1320,
            end: 420
        },
        sound_window: Window::Between {
            start: 480,
            end: 1200
        },
        desiccant_days: 30,
        desiccant_reset: Some(20_389),
        meal_reminder: MealReminder::Before { minutes: 60 },
    }));
    pinned("custom settings", &f, 1, 128, CUSTOM_SETTINGS);
    assert!(s.save(&Settings {
        bowl_mode: BowlMode::Single,
        lights: true,
        sound: false,
        volume: 0,
        utc_offset_minutes: 14 * 60,
        lights_window: Window::Always,
        sound_window: Window::Between { start: 0, end: 0 },
        desiccant_days: 1,
        desiccant_reset: Some(0),
        meal_reminder: MealReminder::Before { minutes: 1 },
    }));
    pinned("third settings", &f, 0, 128, THIRD_SETTINGS);
}

#[test]
fn net_records() {
    let mut f = FakeJournalFlash::with_unit(512);
    let mut s = NetStore::default();
    // SAFETY: the fake flash outlives the store.
    unsafe { s.open(f.io()) };

    let short = Credentials::new(b"home", b"passphrase-1").unwrap();
    assert!(s.save(Some(&short)));
    pinned("short credentials", &f, 0, 128, SHORT_CREDENTIALS);

    let ssid: [u8; 32] = core::array::from_fn(|i| 0x80 | (i as u8));
    let psk: [u8; 63] = core::array::from_fn(|i| b'!' + (i as u8));
    let long = Credentials::new(&ssid, &psk).unwrap();
    assert!(s.save(Some(&long)));
    pinned("longest credentials", &f, 1, 128, LONGEST_CREDENTIALS);

    assert!(s.save(None));
    pinned("clear", &f, 0, 128, CLEAR);
}

/// The time settings: a zone and an address, then the defaults written.
fn time_settings() -> [TimeSettings; 2] {
    [
        TimeSettings {
            zone: Some(Zone::parse(b"MST7MDT,M3.2.0,M11.1.0").unwrap()),
            ntp: NtpServer::Address {
                ip: [10, 42, 103, 1],
                port: 1123,
            },
        },
        TimeSettings::default(),
    ]
}

#[test]
fn time_records() {
    let mut f = FakeJournalFlash::with_unit(512);
    let mut s = TimeStore::default();
    // SAFETY: the fake flash outlives the store.
    unsafe { s.open(f.io()) };

    let [zone, defaults] = time_settings();
    assert!(s.save(&zone));
    pinned("zone and address", &f, 0, 128, ZONE_AND_ADDRESS);
    assert!(s.save(&defaults));
    pinned("time defaults", &f, 1, 128, TIME_DEFAULTS);
}

#[test]
fn time_store_reads_the_pinned_records() {
    let [zone, defaults] = time_settings();
    for (record, slot, generation, settings) in [
        (ZONE_AND_ADDRESS, 0, 1, zone),
        (TIME_DEFAULTS, 1, 2, defaults),
    ] {
        let mut f = flash(512, &[(slot, record)]);
        let mut s = TimeStore::default();
        // SAFETY: the fake flash outlives the store.
        assert_eq!(unsafe { s.open(f.io()) }, TimeStatus::Loaded);
        assert_eq!(
            (s.generation, s.slot, s.settings),
            (generation, slot as u32, settings)
        );
    }
}

/// A measured weight, then the weight cleared.
const CALIBRATIONS: [Calibration; 2] = [
    Calibration {
        portion_mg: Some(9_800),
    },
    Calibration { portion_mg: None },
];

#[test]
fn calibration_records() {
    let mut f = FakeJournalFlash::with_unit(512);
    let mut s = CalibrationStore::default();
    // SAFETY: the fake flash outlives the store.
    unsafe { s.open(f.io()) };

    let [weighed, cleared] = CALIBRATIONS;
    assert!(s.save(&weighed));
    pinned("weighed", &f, 0, 128, WEIGHED);
    assert!(s.save(&cleared));
    pinned("cleared", &f, 1, 128, CLEARED);
}

#[test]
fn calibration_store_reads_the_pinned_records() {
    let [weighed, cleared] = CALIBRATIONS;
    for (record, slot, generation, calibration) in
        [(WEIGHED, 0, 1, weighed), (CLEARED, 1, 2, cleared)]
    {
        let mut f = flash(512, &[(slot, record)]);
        let mut s = CalibrationStore::default();
        // SAFETY: the fake flash outlives the store.
        assert_eq!(unsafe { s.open(f.io()) }, CalibrationStatus::Loaded);
        assert_eq!(
            (s.generation, s.slot, s.calibration),
            (generation, slot as u32, calibration)
        );
    }
}

/// Breakfast every day in slot 0 and a one-off in slot 2, saved at
/// 2026-10-28T07:00Z; then breakfast's occurrence claimed.
fn plans() -> [Plans; 2] {
    let mut table = [None; PLANS];
    table[0] = Some(Plan {
        minute: 7 * 60 + 30,
        portions: 2,
        days: Weekdays::EVERY_DAY,
        enabled: true,
    });
    table[2] = Some(Plan {
        minute: 18 * 60,
        portions: 1,
        days: Weekdays::ONCE,
        enabled: false,
    });
    let saved = Plans::default()
        .changed(&PlansChange::Replace {
            table,
            at_s: Some(1_793_170_800),
        })
        .unwrap();
    let claim = Claim {
        plan: 0,
        due_s: 1_793_172_600,
        high_water: 41,
        disabled: false,
    };
    let claimed = saved
        .changed(&PlansChange::Claim {
            claim,
            revision: saved.revision,
        })
        .unwrap();
    [saved, claimed]
}

#[test]
fn plans_records() {
    let mut f = FakeJournalFlash::with_unit(512);
    let mut s = PlansStore::default();
    // SAFETY: the fake flash outlives the store.
    unsafe { s.open(f.io()) };

    let [saved, claimed] = plans();
    assert!(s.save(&saved));
    pinned("plans saved", &f, 0, 128, PLANS_SAVED);
    assert!(s.save(&claimed));
    pinned("plans claimed", &f, 1, 128, PLANS_CLAIMED);
}

#[test]
fn plans_store_reads_the_pinned_records() {
    let [saved, claimed] = plans();
    for (record, slot, sequence, plans) in
        [(PLANS_SAVED, 0, 1, saved), (PLANS_CLAIMED, 1, 2, claimed)]
    {
        let mut f = flash(512, &[(slot, record)]);
        let mut s = PlansStore::default();
        // SAFETY: the fake flash outlives the store.
        assert_eq!(unsafe { s.open(f.io()) }, PlansStatus::Loaded);
        assert_eq!(
            (s.sequence, s.slot, s.plans),
            (sequence, slot as u32, plans)
        );
    }
}

const PLANS_SAVED: &[&str] = &[
    "504c46504c4e303101008000010000000100000003c201027f00000000000138",
    "0401000000000000000000000000000000000000000000000000000000000000",
    "000000000000709de16a00000000709de16a0000000000000000000000000000",
    "000000000000000000000000000000000000000000000000a6f513af434d4954",
];
const PLANS_CLAIMED: &[&str] = &[
    "504c46504c4e303101008000020000000100000003c201027f00000000000138",
    "0401000000000000000000000000000000000000000000000000000000000000",
    "000000000000709de16a00000000709de16a0000000000000000000000000000",
    "0000000000000000000000000000010078a4e16a2900000036e38ece434d4954",
];

#[test]
fn client_records() {
    let mut f = FakeJournalFlash::with_unit(1024);
    let mut s = ClientStore::default();
    // SAFETY: the fake flash outlives the store.
    unsafe { s.open(f.io()) };

    let identity: [u8; 32] = core::array::from_fn(|i| 0x40 + (i as u8));
    s.set_identity(&identity).unwrap();
    pinned("identity alone", &f, 0, 584, IDENTITY_ALONE);

    let roles = [Role::Owner, Role::Member, Role::Server, Role::UpdateSigner];
    for (n, role) in (1u8..).zip(roles) {
        let label = [b'a' + n; 16];
        let label = Label::new(&label[..usize::from(n) * 4]).unwrap();
        s.enroll([n; 32], label, role).unwrap();
    }
    s.revoke(&[2; 32]).unwrap();
    pinned(
        "identity and three clients",
        &f,
        1,
        584,
        IDENTITY_AND_THREE_CLIENTS,
    );

    s.reset(&[0xee; 32]).unwrap();
    pinned("after a reset", &f, 0, 584, AFTER_A_RESET);
}

#[test]
fn event_records() {
    let mut f = FakeJournalFlash::with_units(512, 2);
    let mut log = EventLog::closed();
    // SAFETY: the fake flash outlives the log.
    unsafe { log.open(f.io(), 2, 0x5eed_0001) };

    let body: [u8; BODY_BYTES] = core::array::from_fn(|i| 0x10 + (i as u8));
    assert_eq!(log.append(Some(1_761_400_000), &body), Ok(1));
    assert_eq!(log.append(None, &[0; BODY_BYTES]), Ok(2));
    assert_eq!(log.append(Some(u32::MAX), &[0xff; BODY_BYTES]), Ok(3));
    pinned("three events", &f, 0, 96, THREE_EVENTS);
}

/// Erased units of `unit` bytes holding each `(slot, record)` given.
fn flash(unit: usize, records: &[(usize, &[&str])]) -> FakeJournalFlash {
    let mut f = FakeJournalFlash::with_unit(unit);
    for &(slot, lines) in records {
        let hex: String = lines.concat();
        for (i, pair) in hex.as_bytes().chunks(2).enumerate() {
            let pair = core::str::from_utf8(pair).unwrap();
            f.bytes[slot][i] = u8::from_str_radix(pair, 16).unwrap();
        }
    }
    f
}

#[test]
fn settings_store_reads_the_pinned_records() {
    for (record, slot, generation, settings) in [
        (DEFAULT_SETTINGS, 0, 1, Settings::default()),
        (
            CUSTOM_SETTINGS,
            1,
            2,
            Settings {
                bowl_mode: BowlMode::Double,
                lights: false,
                sound: true,
                volume: 100,
                utc_offset_minutes: -570,
                lights_window: Window::Between {
                    start: 1320,
                    end: 420,
                },
                sound_window: Window::Between {
                    start: 480,
                    end: 1200,
                },
                desiccant_days: 30,
                desiccant_reset: Some(20_389),
                meal_reminder: MealReminder::Before { minutes: 60 },
            },
        ),
        (
            THIRD_SETTINGS,
            0,
            3,
            Settings {
                bowl_mode: BowlMode::Single,
                lights: true,
                sound: false,
                volume: 0,
                utc_offset_minutes: 14 * 60,
                lights_window: Window::Always,
                sound_window: Window::Between { start: 0, end: 0 },
                desiccant_days: 1,
                desiccant_reset: Some(0),
                meal_reminder: MealReminder::Before { minutes: 1 },
            },
        ),
    ] {
        let mut f = flash(512, &[(slot, record)]);
        let mut s = SettingsStore::default();
        // SAFETY: the fake flash outlives the store.
        let status = unsafe { s.open(f.io(), 0x5046_5731_3502_0001) };
        assert_eq!(status, SettingsStatus::Loaded);
        assert_eq!(
            (s.generation, s.slot, s.settings),
            (generation, slot as u32, settings)
        );
    }
}

#[test]
fn net_store_reads_the_pinned_records() {
    let ssid: [u8; 32] = core::array::from_fn(|i| 0x80 | (i as u8));
    let psk: [u8; 63] = core::array::from_fn(|i| b'!' + (i as u8));
    for (record, slot, generation, credentials) in [
        (
            SHORT_CREDENTIALS,
            0,
            1,
            Credentials::new(b"home", b"passphrase-1"),
        ),
        (LONGEST_CREDENTIALS, 1, 2, Credentials::new(&ssid, &psk)),
        (CLEAR, 0, 3, None),
    ] {
        let mut f = flash(512, &[(slot, record)]);
        let mut s = NetStore::default();
        // SAFETY: the fake flash outlives the store.
        assert_eq!(unsafe { s.open(f.io()) }, NetStatus::Loaded);
        assert_eq!(
            (s.generation, s.slot, s.credentials),
            (generation, slot as u32, credentials)
        );
    }
}

#[test]
fn client_store_reads_the_pinned_records() {
    let identity: [u8; 32] = core::array::from_fn(|i| 0x40 + (i as u8));
    let client = |n: u8, role, enrolled| Client {
        key: [n; 32],
        label: Label::new(&[b'a' + n; 16][..usize::from(n) * 4]).unwrap(),
        role,
        enrolled,
    };
    let three = vec![
        client(1, Role::Owner, 2),
        client(3, Role::Server, 4),
        client(4, Role::UpdateSigner, 5),
    ];
    for (record, slot, generation, secret, clients) in [
        (IDENTITY_ALONE, 0, 1, identity, Vec::new()),
        (IDENTITY_AND_THREE_CLIENTS, 1, 6, identity, three),
        (AFTER_A_RESET, 0, 7, [0xee; 32], Vec::new()),
    ] {
        let mut f = flash(1024, &[(slot, record)]);
        let mut s = ClientStore::default();
        // SAFETY: the fake flash outlives the store.
        assert_eq!(unsafe { s.open(f.io()) }, ClientStatus::Loaded);
        assert_eq!(
            (s.generation, s.slot, s.identity()),
            (generation, slot as u32, Some(&secret))
        );
        assert_eq!(s.clients().copied().collect::<Vec<_>>(), clients);
    }
}

#[test]
fn event_ring_reads_the_pinned_records() {
    let mut f = flash(512, &[(0, THREE_EVENTS)]);
    let mut log = EventLog::closed();
    // SAFETY: the fake flash outlives the log.
    unsafe { log.open(f.io(), 2, 0x0bad_0bad) };
    assert_eq!((log.epoch(), log.last_seq()), (0x5eed_0001, Some(3)));

    let body: [u8; BODY_BYTES] = core::array::from_fn(|i| 0x10 + (i as u8));
    let expected = [
        (1, Some(1_761_400_000), body),
        (2, None, [0; BODY_BYTES]),
        (3, Some(u32::MAX), [0xff; BODY_BYTES]),
    ];
    let mut after = None;
    for (seq, at, body) in expected {
        assert_eq!(
            log.read_after(after),
            Read::Record(Record { seq, at, body })
        );
        after = Some(seq);
    }
    assert_eq!(log.read_after(after), Read::End);
}

// The pinned records, as deployed feeders hold them.

/// Default settings.
const DEFAULT_SETTINGS: &[&str] = &[
    "504c465345543031040080000100000000000000010002353157465000033200",
    "0000000000000000000000000e00000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "00000000000000000000000000000000000000000000000038234339434d4954",
];

/// Custom settings.
const CUSTOM_SETTINGS: &[&str] = &[
    "504c46534554303104008000020000000000000001000235315746500102643c",
    "c6fd01012805a401e001b0041e010000a54f0000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "000000000000000000000000000000000000000000000000ab6afb43434d4954",
];

/// Third settings.
const THIRD_SETTINGS: &[&str] = &[
    "504c465345543031040080000300000000000000010002353157465000010001",
    "4803000100000000000000000101000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000009b63283d434d4954",
];

/// Short credentials.
const SHORT_CREDENTIALS: &[&str] = &[
    "504c464e45543031010080000100000000000000040c686f6d65000000000000",
    "0000000000000000000000000000000000000000000070617373706872617365",
    "2d31000000000000000000000000000000000000000000000000000000000000",
    "000000000000000000000000000000000000000000000000d74400c3434d4954",
];

/// Longest credentials.
const LONGEST_CREDENTIALS: &[&str] = &[
    "504c464e45543031010080000200000000000000203f80818283848586878889",
    "8a8b8c8d8e8f909192939495969798999a9b9c9d9e9f2122232425262728292a",
    "2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a",
    "4b4c4d4e4f505152535455565758595a5b5c5d5e5f000000ecb02325434d4954",
];

/// Clear.
const CLEAR: &[&str] = &[
    "504c464e45543031010080000300000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "000000000000000000000000000000000000000000000000e5918e43434d4954",
];

/// Identity alone.
const IDENTITY_ALONE: &[&str] = &[
    "504c46434c493031010048020100000000000000404142434445464748494a4b",
    "4c4d4e4f505152535455565758595a5b5c5d5e5f010000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "24b1233e434d4954",
];

/// Identity and three clients.
const IDENTITY_AND_THREE_CLIENTS: &[&str] = &[
    "504c46434c493031010048020600000000000000404142434445464748494a4b",
    "4c4d4e4f505152535455565758595a5b5c5d5e5f010000000000000000000000",
    "0101010101010101010101010101010101010101010101010101010101010101",
    "6262626200000000000000000000000001000000020000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0303030303030303030303030303030303030303030303030303030303030303",
    "6464646464646464646464640000000003000000040000000000000000000000",
    "0404040404040404040404040404040404040404040404040404040404040404",
    "6565656565656565656565656565656504000000050000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "938aecda434d4954",
];

/// After a reset.
const AFTER_A_RESET: &[&str] = &[
    "504c46434c493031010048020700000000000000eeeeeeeeeeeeeeeeeeeeeeee",
    "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee010000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0b7116ea434d4954",
];

/// Three events.
const THREE_EVENTS: &[&str] = &[
    "010000000100ed5ec0d4fc68101112131415161718191a1b1c1d1e1fbedaf516",
    "020000000100ed5e0000000000000000000000000000000000000000529a0291",
    "030000000100ed5effffffffffffffffffffffffffffffffffffffff728e95b3",
];

/// Time settings: a zone rule and an NTP address.
const ZONE_AND_ADDRESS: &[&str] = &[
    "504c4654494d3031010080000100000000000000020a2a67016304164d535437",
    "4d44542c4d332e322e302c4d31312e312e300000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000007c75de9a434d4954",
];

/// Time settings: no zone, NTP from the gateway on port 123.
const TIME_DEFAULTS: &[&str] = &[
    "504c4654494d303101008000020000000000000001000000007b000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000008228174a434d4954",
];

/// A portion weighed at 9.8 g.
const WEIGHED: &[&str] = &[
    "504c4643414c3031010080000100000000000000482600000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000007613cdb6434d4954",
];

/// The weight cleared.
const CLEARED: &[&str] = &[
    "504c4643414c3031010080000200000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "00000000000000000000000000000000000000000000000058f10112434d4954",
];
