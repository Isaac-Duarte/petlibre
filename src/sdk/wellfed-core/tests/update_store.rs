//! The update store: the candidate's states, the floor that only rises,
//! retired key ids that are never forgotten, the release slot and bank 0's
//! image, across reopens and a power cut at every flash mutation.

use wellfed_core::ota::offer::{ImageKind, key_id};
use wellfed_core::store::slot::crc32;
use wellfed_core::store::update::{
    Bank0, Candidate, RECORD_BYTES, REVOKED, State, UpdateError, UpdateStatus, UpdateStore,
};
use wellfed_testkit::journal_flash::{FakeJournalFlash, FlashIo};

const UNIT: usize = 512;

fn open(f: &mut FakeJournalFlash) -> UpdateStore<FlashIo> {
    let mut s = UpdateStore::default();
    // SAFETY: the fake flash outlives the store in every test.
    unsafe { s.open(f.io()) };
    s
}

fn candidate(version: u32, build: u8) -> Candidate {
    Candidate {
        digest: [build ^ 0x55; 32],
        version,
        build_id: [build; 32],
        kind: ImageKind::Replacement,
    }
}

fn bank0(build: u8) -> Bank0 {
    Bank0 {
        build_id: [build; 32],
        len: 942_080,
        sha256: [build ^ 0x0f; 32],
        key: build.is_multiple_of(2).then_some([build; 8]),
    }
}

#[test]
fn a_candidate_goes_pending_activated_then_confirms_and_raises_the_floor() {
    let mut f = FakeJournalFlash::with_unit(UNIT);
    let mut s = open(&mut f);
    assert_eq!(s.status, UpdateStatus::Empty);
    assert_eq!(s.record().min_version, 0);

    s.set_pending(candidate(7, 1)).unwrap();
    assert_eq!(open(&mut f).record().state, State::Pending(candidate(7, 1)));

    let mut s = open(&mut f);
    s.set_activated(candidate(7, 1)).unwrap();
    assert_eq!(s.confirm(&[2; 32]), Err(UpdateError::NotActivated));
    assert_eq!(s.confirm(&[1; 32]), Ok(7));

    let s = open(&mut f);
    assert_eq!(s.record().min_version, 7);
    assert_eq!(s.record().state, State::None);

    // A downgrade confirms without lowering the floor.
    let mut s = open(&mut f);
    s.set_activated(candidate(3, 4)).unwrap();
    assert_eq!(s.confirm(&[4; 32]), Ok(3));
    assert_eq!(open(&mut f).record().min_version, 7);
}

#[test]
fn retired_ids_are_kept_and_the_release_slot_takes_one_live_key() {
    let mut f = FakeJournalFlash::with_unit(UNIT);
    let mut s = open(&mut f);
    let release = [0x33; 32];

    s.add_key(&release).unwrap();
    s.add_key(&release).unwrap();
    assert_eq!(s.add_key(&[0x44; 32]), Err(UpdateError::Occupied));
    assert_eq!(open(&mut f).record().added_key(), Some(release));

    let mut s = open(&mut f);
    s.revoke(&key_id(&release)).unwrap();
    s.revoke(&key_id(&release)).unwrap();
    let r = open(&mut f);
    assert_eq!(r.record().revoked(), &[key_id(&release)]);
    assert_eq!(r.record().added_key(), None);

    // A retired slot takes a new key; a retired key never comes back.
    let mut s = open(&mut f);
    assert_eq!(s.add_key(&release), Err(UpdateError::Revoked));
    s.add_key(&[0x44; 32]).unwrap();
    for n in 1..REVOKED {
        s.revoke(&[u8::try_from(n).unwrap(); 8]).unwrap();
    }
    assert_eq!(s.revoke(&[0x99; 8]), Err(UpdateError::Full));
    let r = open(&mut f);
    assert_eq!(r.record().revoked().len(), REVOKED);
    assert_eq!(r.record().added_key(), Some([0x44; 32]));
}

#[test]
fn bank_0_is_recorded_only_when_it_changes() {
    let mut f = FakeJournalFlash::with_unit(UNIT);
    let mut s = open(&mut f);
    s.set_bank0(bank0(5)).unwrap();
    let writes = f.mutations;
    let mut s = open(&mut f);
    s.set_bank0(bank0(5)).unwrap();
    assert_eq!(f.mutations, writes, "an unchanged bank 0 costs no write");

    s.set_bank0(bank0(6)).unwrap();
    assert_eq!(open(&mut f).record().bank0, Some(bank0(6)));
}

#[test]
fn a_cancelled_candidate_is_forgotten_without_touching_the_floor() {
    let mut f = FakeJournalFlash::with_unit(UNIT);
    let mut s = open(&mut f);
    s.set_activated(candidate(4, 1)).unwrap();
    s.confirm(&[1; 32]).unwrap();
    s.set_pending(candidate(9, 2)).unwrap();
    s.clear().unwrap();
    let r = open(&mut f);
    assert_eq!((r.record().state, r.record().min_version), (State::None, 4));
}

/// Everything the store holds, for comparing before and after a cut.
fn snapshot(f: &mut FakeJournalFlash) -> (u32, State, Option<[u8; 32]>, Vec<[u8; 8]>) {
    let s = open(f);
    let r = s.record();
    (r.min_version, r.state, r.added, r.revoked().to_vec())
}

#[test]
fn a_power_cut_at_every_mutation_keeps_old_or_new_and_never_lowers_the_floor() {
    type Change = fn(&mut UpdateStore<FlashIo>) -> Result<(), UpdateError>;
    let changes: [(&str, Change); 4] = [
        ("pending", |s| s.set_pending(candidate(12, 3))),
        ("confirm", |s| s.confirm(&[2; 32]).map(|_| ())),
        ("revoke", |s| s.revoke(&[0x77; 8])),
        ("add", |s| s.add_key(&[0x66; 32])),
    ];
    let history = |f: &mut FakeJournalFlash| {
        let mut s = open(f);
        s.set_activated(candidate(5, 1)).unwrap();
        s.confirm(&[1; 32]).unwrap();
        s.revoke(&[0x11; 8]).unwrap();
        s.set_activated(candidate(8, 2)).unwrap();
        s
    };

    for (name, change) in changes {
        let mut probe = FakeJournalFlash::with_unit(UNIT);
        let old = {
            history(&mut probe);
            snapshot(&mut probe)
        };
        let mut s = open(&mut probe);
        let start = probe.mutations;
        change(&mut s).unwrap();
        let new = snapshot(&mut probe);
        let span = i64::from(probe.mutations - start);
        assert!(span > 0, "{name} wrote nothing");

        for cut in 0..=span {
            let mut f = FakeJournalFlash::with_unit(UNIT);
            history(&mut f);
            let mut s = open(&mut f);
            f.budget = cut;
            let _ = change(&mut s);
            f.budget = -1;

            let found = snapshot(&mut f);
            assert!(found == old || found == new, "{name} cut {cut}: {found:?}");
            assert!(found.0 >= 5, "{name} cut {cut}: the floor fell");
            assert!(
                found.3.contains(&[0x11; 8]),
                "{name} cut {cut}: an id was forgotten"
            );
            let mut r = open(&mut f);
            assert!(r.writable(), "{name} cut {cut}: {:?}", r.status);
            r.set_pending(candidate(20, 9)).unwrap();
        }
    }
}

#[test]
fn a_failed_write_keeps_the_previous_record_and_stops_writing() {
    let mut f = FakeJournalFlash::with_unit(UNIT);
    let mut s = open(&mut f);
    s.set_pending(candidate(2, 1)).unwrap();
    f.fail_call = f.calls + 1;
    assert_eq!(s.set_pending(candidate(3, 2)), Err(UpdateError::Io));
    assert_eq!(s.record().state, State::Pending(candidate(2, 1)));
    assert_eq!(s.clear(), Err(UpdateError::NotWritable));
}

#[test]
fn a_newer_schema_is_read_only_and_a_small_unit_refuses_writes() {
    let mut f = FakeJournalFlash::with_unit(UNIT);
    let mut s = open(&mut f);
    s.set_pending(candidate(2, 1)).unwrap();

    let mut record = f.bytes[0][..RECORD_BYTES].to_vec();
    record[8] = 2;
    let crc_at = RECORD_BYTES - 8;
    let crc = crc32(&record[..crc_at]);
    record[crc_at..crc_at + 4].copy_from_slice(&crc.to_le_bytes());
    f.bytes[0][..RECORD_BYTES].copy_from_slice(&record);

    let mut r = open(&mut f);
    assert_eq!(r.status, UpdateStatus::ReadOnly);
    assert!(!r.readable(), "a floor this build cannot read is no floor");
    assert_eq!(
        r.set_pending(candidate(3, 2)),
        Err(UpdateError::NotWritable)
    );

    let mut small = FakeJournalFlash::with_unit(128);
    let s = open(&mut small);
    assert_eq!(s.status, UpdateStatus::Io);
    assert!(!s.readable());
}

#[test]
fn a_corrupted_record_falls_back_to_the_older_one() {
    let mut f = FakeJournalFlash::with_unit(UNIT);
    let mut s = open(&mut f);
    s.set_pending(candidate(2, 1)).unwrap();
    s.set_pending(candidate(3, 2)).unwrap();
    f.bytes[1][40] ^= 1;
    assert_eq!(open(&mut f).record().state, State::Pending(candidate(2, 1)));
}

/// Going back to stock forgets the floor and a pending candidate, and nothing
/// that was a promise about keys or a record that an image activated.
#[test]
fn starting_over_forgets_the_floor_and_a_pending_candidate_only() {
    let mut f = FakeJournalFlash::with_unit(UNIT);
    let mut s = open(&mut f);
    s.set_activated(candidate(7, 1)).unwrap();
    s.confirm(&[1; 32]).unwrap();
    s.add_key(&[3; 32]).unwrap();
    s.revoke(&[4; 8]).unwrap();
    let stock = candidate(9, 2);
    s.set_activated(stock).unwrap();

    let mut s = open(&mut f);
    s.start_over().unwrap();
    let r = open(&mut f);
    assert_eq!(r.record().min_version, 0);
    assert_eq!(r.record().state, State::Activated(stock));
    assert_eq!(r.record().added, Some([3; 32]));
    assert_eq!(r.record().revoked(), [[4; 8]]);

    // Repeated after a cut, it writes nothing.
    let generation = r.generation;
    let mut s = open(&mut f);
    s.start_over().unwrap();
    assert_eq!(open(&mut f).generation, generation);

    let mut s = open(&mut f);
    s.set_pending(candidate(9, 2)).unwrap();
    s.start_over().unwrap();
    assert_eq!(open(&mut f).record().state, State::None);
}
