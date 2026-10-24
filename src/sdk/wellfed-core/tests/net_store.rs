//! Stored Wi-Fi credentials: alternating saves, clear, validation, a
//! power cut at every mutation, and a newer schema.

use wellfed_core::store::net::{Credentials, NetStatus, NetStore};
use wellfed_core::store::slot::{RECORD_BYTES, crc32};
use wellfed_testkit::journal_flash::{FakeJournalFlash, FlashIo, UNIT};

fn open(f: &mut FakeJournalFlash) -> NetStore<FlashIo> {
    let mut s = NetStore::default();
    // SAFETY: the fake flash outlives the store in every test.
    unsafe { s.open(f.io()) };
    s
}

fn creds(n: u8) -> Credentials {
    Credentials::new(&[b'n', b'0' + n], b"passphrase-1").unwrap()
}

#[test]
fn saves_alternate_clear_and_reload() {
    let mut f = FakeJournalFlash::with_unit(UNIT);
    let mut s = open(&mut f);
    assert_eq!((s.status, s.credentials), (NetStatus::Empty, None));

    assert!(s.save(Some(&creds(1))));
    assert!(s.save(Some(&creds(2))));
    let r = open(&mut f);
    assert_eq!((r.status, r.generation, r.slot), (NetStatus::Loaded, 2, 1));
    assert_eq!(r.credentials.unwrap().ssid(), b"n2");
    assert_eq!(r.credentials.unwrap().psk(), b"passphrase-1");

    let mut r = r;
    assert!(r.save(None), "a clear is a record too");
    let r = open(&mut f);
    assert_eq!(
        (r.status, r.generation, r.credentials),
        (NetStatus::Loaded, 3, None)
    );
}

#[test]
fn credentials_are_validated() {
    assert!(Credentials::new(b"", b"passphrase").is_none());
    assert!(Credentials::new(&[b'x'; 33], b"passphrase").is_none());
    assert!(Credentials::new(b"x", b"short").is_none());
    assert!(Credentials::new(b"x", &[b'p'; 64]).is_none());
    assert!(Credentials::new(b"x", b"tab\there!").is_none());
    assert!(
        Credentials::new(&[0xff; 32], &[b'~'; 63]).is_some(),
        "any SSID bytes"
    );
}

#[test]
fn power_cut_at_every_mutation_keeps_old_or_new_and_stays_writable() {
    for history in 0..3u8 {
        let mut probe = FakeJournalFlash::with_unit(UNIT);
        let mut s = open(&mut probe);
        for n in 0..history {
            assert!(s.save(Some(&creds(n))));
        }
        let start = probe.mutations;
        assert!(s.save(Some(&creds(9))));
        let span = i64::from(probe.mutations - start);

        for cut in 0..=span {
            let mut f = FakeJournalFlash::with_unit(UNIT);
            let mut s = open(&mut f);
            for n in 0..history {
                assert!(s.save(Some(&creds(n))));
            }
            f.budget = cut;
            let _ = s.save(Some(&creds(9)));
            f.budget = -1;

            let mut r = open(&mut f);
            let old = history.checked_sub(1).map(creds);
            assert!(
                r.credentials == old || r.credentials == Some(creds(9)),
                "history {history} cut {cut}"
            );
            assert!(r.writable(), "history {history} cut {cut}: {:?}", r.status);
            assert!(r.save(Some(&creds(7))));
            assert_eq!(open(&mut f).credentials, Some(creds(7)));
        }
    }
}

#[test]
fn a_newer_schema_is_read_only_and_uses_the_build_credentials() {
    let mut f = FakeJournalFlash::with_unit(UNIT);
    let mut s = open(&mut f);
    assert!(s.save(Some(&creds(1))));

    // Rewrite slot 0 as version 2 with a valid CRC.
    let mut record = f.bytes[0][..RECORD_BYTES].to_vec();
    record[8] = 2;
    let crc = crc32(&record[..120]);
    record[120..124].copy_from_slice(&crc.to_le_bytes());
    f.bytes[0][..RECORD_BYTES].copy_from_slice(&record);

    let mut r = open(&mut f);
    assert_eq!((r.status, r.credentials), (NetStatus::ReadOnly, None));
    assert!(!r.save(Some(&creds(2))));
}
