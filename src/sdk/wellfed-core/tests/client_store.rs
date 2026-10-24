//! The client table: identity, enrollment, revocation and the reset
//! across reopens, its limits, a power cut at every mutation, and a newer
//! schema.

use wellfed_core::store::clients::{
    CLIENTS, ClientError, ClientStatus, ClientStore, Label, RECORD_BYTES, Role,
};
use wellfed_core::store::slot::crc32;
use wellfed_testkit::journal_flash::FakeJournalFlash;

const UNIT: usize = 1024;

fn open(f: &mut FakeJournalFlash) -> ClientStore {
    let mut s = ClientStore::default();
    // SAFETY: the fake flash outlives the store in every test.
    unsafe { s.open(f.io()) };
    s
}

fn label(text: &str) -> Label {
    Label::new(text.as_bytes()).unwrap()
}

fn keys(f: &mut FakeJournalFlash) -> Vec<[u8; 32]> {
    open(f).clients().map(|client| client.key).collect()
}

#[test]
fn enroll_revoke_and_reset_survive_reopening() {
    let mut f = FakeJournalFlash::with_unit(UNIT);
    let mut s = open(&mut f);
    assert_eq!((s.status, s.identity()), (ClientStatus::Empty, None));

    s.set_identity(&[0xaa; 32]).unwrap();
    assert_eq!(s.set_identity(&[0xbb; 32]), Err(ClientError::IdentitySet));
    s.enroll([1; 32], label("phone"), Role::Owner).unwrap();
    s.enroll([2; 32], label("server"), Role::Server).unwrap();
    assert_eq!(
        s.enroll([1; 32], label("again"), Role::Member),
        Err(ClientError::Duplicate)
    );

    let r = open(&mut f);
    assert_eq!((r.status, r.generation), (ClientStatus::Loaded, 3));
    assert_eq!(r.identity(), Some(&[0xaa; 32]));
    let phone = r.find(&[1; 32]).unwrap();
    assert_eq!(
        (phone.label.as_bytes(), phone.role, phone.enrolled),
        (&b"phone"[..], Role::Owner, 2)
    );
    assert_eq!(r.find(&[2; 32]).unwrap().role, Role::Server);

    let mut r = r;
    r.revoke(&[1; 32]).unwrap();
    assert_eq!(r.revoke(&[1; 32]), Err(ClientError::NotFound));
    assert_eq!(keys(&mut f), vec![[2; 32]]);

    let mut r = open(&mut f);
    r.reset(&[0xcc; 32]).unwrap();
    let r = open(&mut f);
    assert_eq!(r.identity(), Some(&[0xcc; 32]));
    assert_eq!(r.clients().count(), 0);
}

#[test]
fn the_table_holds_eight_and_a_freed_slot_is_reused() {
    let mut f = FakeJournalFlash::with_unit(UNIT);
    let mut s = open(&mut f);
    for n in 0..CLIENTS {
        let n = u8::try_from(n).unwrap();
        s.enroll([n; 32], label("c"), Role::Member).unwrap();
    }
    assert_eq!(
        s.enroll([99; 32], label("c"), Role::Member),
        Err(ClientError::Full)
    );
    s.revoke(&[3; 32]).unwrap();
    s.enroll([99; 32], label("c"), Role::UpdateSigner).unwrap();
    assert_eq!(open(&mut f).clients().count(), CLIENTS);
    assert_eq!(
        open(&mut f).find(&[99; 32]).unwrap().role,
        Role::UpdateSigner
    );
}

#[test]
fn labels_are_validated() {
    assert!(Label::new(b"").is_none());
    assert!(Label::new(&[b'x'; 17]).is_none());
    assert!(Label::new(b"nul\0").is_none());
    assert_eq!(Label::new(&[b'x'; 16]).unwrap().as_bytes(), &[b'x'; 16]);
}

#[test]
fn power_cut_at_every_mutation_keeps_old_or_new_and_stays_writable() {
    for history in 0..3u8 {
        let saved = |f: &mut FakeJournalFlash| {
            let mut s = open(f);
            for n in 0..history {
                s.enroll([n; 32], label("old"), Role::Member).unwrap();
            }
            s
        };
        let old: Vec<[u8; 32]> = (0..history).map(|n| [n; 32]).collect();
        let mut new = old.clone();
        new.push([9; 32]);

        let mut probe = FakeJournalFlash::with_unit(UNIT);
        let mut s = saved(&mut probe);
        let start = probe.mutations;
        s.enroll([9; 32], label("new"), Role::Owner).unwrap();
        let span = i64::from(probe.mutations - start);

        for cut in 0..=span {
            let mut f = FakeJournalFlash::with_unit(UNIT);
            let mut s = saved(&mut f);
            f.budget = cut;
            let _ = s.enroll([9; 32], label("new"), Role::Owner);
            f.budget = -1;

            let found = keys(&mut f);
            assert!(found == old || found == new, "history {history} cut {cut}");
            let mut r = open(&mut f);
            assert!(r.writable(), "history {history} cut {cut}: {:?}", r.status);
            r.revoke(&[9; 32]).ok();
            r.enroll([7; 32], label("after"), Role::Member).unwrap();
            assert!(keys(&mut f).contains(&[7; 32]));
        }
    }
}

#[test]
fn a_failed_write_keeps_the_previous_table() {
    let mut f = FakeJournalFlash::with_unit(UNIT);
    let mut s = open(&mut f);
    s.enroll([1; 32], label("a"), Role::Owner).unwrap();
    f.fail_call = f.calls + 1;
    assert_eq!(
        s.enroll([2; 32], label("b"), Role::Member),
        Err(ClientError::Io)
    );
    assert_eq!(s.clients().count(), 1);
    assert_eq!(
        s.enroll([2; 32], label("b"), Role::Member),
        Err(ClientError::NotWritable)
    );
}

#[test]
fn a_newer_schema_or_too_small_a_unit_refuses_writes() {
    let mut f = FakeJournalFlash::with_unit(UNIT);
    let mut s = open(&mut f);
    s.set_identity(&[1; 32]).unwrap();

    let mut record = f.bytes[0][..RECORD_BYTES].to_vec();
    record[8] = 2;
    let crc_at = RECORD_BYTES - 8;
    let crc = crc32(&record[..crc_at]);
    record[crc_at..crc_at + 4].copy_from_slice(&crc.to_le_bytes());
    f.bytes[0][..RECORD_BYTES].copy_from_slice(&record);
    let mut r = open(&mut f);
    assert_eq!((r.status, r.identity()), (ClientStatus::ReadOnly, None));
    assert_eq!(
        r.enroll([1; 32], label("x"), Role::Owner),
        Err(ClientError::NotWritable)
    );

    let mut small = FakeJournalFlash::with_unit(256);
    assert_eq!(open(&mut small).status, ClientStatus::Io);
}
