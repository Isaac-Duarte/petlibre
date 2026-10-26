//! The on-flash bytes of every record the product's stores write, pinned:
//! settings, Wi-Fi credentials and their clear, the client table, and the
//! event ring's records. These are the layouts deployed feeders hold, so a
//! change to the shared slot layer must leave every byte where it was. The
//! units are larger than the records so the bytes after each record are
//! pinned erased too.

use wellfed_core::feed::progress::BowlMode;
use wellfed_feeder::store::clients::{ClientStore, Label, Role};
use wellfed_feeder::store::events::{BODY_BYTES, EventLog};
use wellfed_feeder::store::net::{Credentials, NetStore};
use wellfed_feeder::store::settings::{Settings, SettingsStore};
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
    pinned(
        "default settings",
        &f,
        0,
        128,
        &[
            "504c465345543031010080000100000000000000010002353157465000033200",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "0000000000000000000000000000000000000000000000008be505f5434d4954",
        ],
    );
    assert!(s.save(&Settings {
        bowl_mode: BowlMode::Double,
        lights: false,
        sound: true,
        volume: 100,
        utc_offset_minutes: -570,
    }));
    pinned(
        "custom settings",
        &f,
        1,
        128,
        &[
            "504c465345543031010080000200000000000000010002353157465001026400",
            "c6fd000000000000000000000000000000000000000000000000000000000000",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "0000000000000000000000000000000000000000000000005eec6e13434d4954",
        ],
    );
    assert!(s.save(&Settings {
        bowl_mode: BowlMode::Single,
        lights: true,
        sound: false,
        volume: 0,
        utc_offset_minutes: 14 * 60,
    }));
    pinned(
        "third settings",
        &f,
        0,
        128,
        &[
            "504c465345543031010080000300000000000000010002353157465000010000",
            "4803000000000000000000000000000000000000000000000000000000000000",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "000000000000000000000000000000000000000000000000f25491d6434d4954",
        ],
    );
}

#[test]
fn net_records() {
    let mut f = FakeJournalFlash::with_unit(512);
    let mut s = NetStore::default();
    // SAFETY: the fake flash outlives the store.
    unsafe { s.open(f.io()) };

    let short = Credentials::new(b"home", b"passphrase-1").unwrap();
    assert!(s.save(Some(&short)));
    pinned(
        "short credentials",
        &f,
        0,
        128,
        &[
            "504c464e45543031010080000100000000000000040c686f6d65000000000000",
            "0000000000000000000000000000000000000000000070617373706872617365",
            "2d31000000000000000000000000000000000000000000000000000000000000",
            "000000000000000000000000000000000000000000000000d74400c3434d4954",
        ],
    );

    let ssid: [u8; 32] = core::array::from_fn(|i| 0x80 | (i as u8));
    let psk: [u8; 63] = core::array::from_fn(|i| b'!' + (i as u8));
    let long = Credentials::new(&ssid, &psk).unwrap();
    assert!(s.save(Some(&long)));
    pinned(
        "longest credentials",
        &f,
        1,
        128,
        &[
            "504c464e45543031010080000200000000000000203f80818283848586878889",
            "8a8b8c8d8e8f909192939495969798999a9b9c9d9e9f2122232425262728292a",
            "2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a",
            "4b4c4d4e4f505152535455565758595a5b5c5d5e5f000000ecb02325434d4954",
        ],
    );

    assert!(s.save(None));
    pinned(
        "clear",
        &f,
        0,
        128,
        &[
            "504c464e45543031010080000300000000000000000000000000000000000000",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "000000000000000000000000000000000000000000000000e5918e43434d4954",
        ],
    );
}

#[test]
fn client_records() {
    let mut f = FakeJournalFlash::with_unit(1024);
    let mut s = ClientStore::default();
    // SAFETY: the fake flash outlives the store.
    unsafe { s.open(f.io()) };

    let identity: [u8; 32] = core::array::from_fn(|i| 0x40 + (i as u8));
    s.set_identity(&identity).unwrap();
    pinned(
        "identity alone",
        &f,
        0,
        584,
        &[
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
        ],
    );

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
        &[
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
        ],
    );

    s.reset(&[0xee; 32]).unwrap();
    pinned(
        "after a reset",
        &f,
        0,
        584,
        &[
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
        ],
    );
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
    pinned(
        "three events",
        &f,
        0,
        96,
        &[
            "010000000100ed5ec0d4fc68101112131415161718191a1b1c1d1e1fbedaf516",
            "020000000100ed5e0000000000000000000000000000000000000000529a0291",
            "030000000100ed5effffffffffffffffffffffffffffffffffffffff728e95b3",
        ],
    );
}
