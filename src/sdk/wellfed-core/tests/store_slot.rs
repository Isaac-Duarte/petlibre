//! The dual-slot store under its torn-write rule, apart from any store's
//! policy: the shared header and trailer, what each state of a unit reads
//! as, the newest-valid-wins pick, and a power cut at every mutation and a
//! failure at every call of a write, for both extents and several unit
//! sizes. Each store's own power-cut test then checks its policy over the
//! same rule.

use wellfed_core::store::slot::{
    CRC_AT, DualSlot, Extent, Found, Layout, Pick, Prepare, RECORD_BYTES, crc32, get_u64, newest,
    put_u64, seal, target,
};
use wellfed_testkit::journal_flash::{FakeJournalFlash, FlashIo};

const UNIT: Layout = Layout {
    magic: *b"PLFTEST1",
    version: 3,
    extent: Extent::Unit,
};
const RECORD: Layout = Layout {
    extent: Extent::Record,
    ..UNIT
};
/// Unit sizes: the record exactly, a tail shorter than a chunk, and a
/// tail of whole chunks. A sector's every call is covered once, by the
/// failure test; cutting at each of its mutations proves nothing more.
const SIZES: [usize; 3] = [RECORD_BYTES, 148, 256];
const SECTOR: usize = 4096;

fn units(f: &mut FakeJournalFlash, layout: &'static Layout) -> DualSlot<FlashIo> {
    // SAFETY: every test keeps its flash in place while the units are used.
    DualSlot::new(unsafe { f.io() }, layout)
}

/// A record of generation `generation`, its payload byte `value` at 20.
fn record(layout: &Layout, generation: u64, value: u8) -> [u8; RECORD_BYTES] {
    let mut p = layout.record();
    put_u64::<12, _>(&mut p, generation);
    p[20] = value;
    seal(&mut p);
    p
}

/// The generation and the payload; anything else in the fields is refused.
fn decode(p: &[u8; RECORD_BYTES]) -> Option<(u64, u8)> {
    let generation = get_u64::<12, _>(p);
    (generation != 0 && p[21..CRC_AT].iter().all(|&b| b == 0)).then_some((generation, p[20]))
}

fn scan(f: &mut FakeJournalFlash, layout: &'static Layout, slot: u32) -> Found<(u64, u8)> {
    units(f, layout)
        .scan(slot, decode)
        .expect("the fake flash reads")
}

/// Store `p` in unit `slot` directly, as another writer or decay would.
fn place(f: &mut FakeJournalFlash, slot: usize, p: &[u8]) {
    f.bytes[slot][..p.len()].copy_from_slice(p);
}

#[test]
fn the_header_and_trailer_are_the_shared_layout() {
    let p = record(&UNIT, 0x0102_0304_0506_0708, 0x5a);

    assert_eq!(&p[..8], b"PLFTEST1");
    assert_eq!(&p[8..12], &[3, 0, 128, 0]);
    assert_eq!(&p[12..21], &[8, 7, 6, 5, 4, 3, 2, 1, 0x5a]);
    assert_eq!(p[120..124], crc32(&p[..120]).to_le_bytes());
    assert_eq!(&p[124..], b"CMIT");

    // A record of another length carries that length and its trailer at
    // its own end.
    let mut long: [u8; 584] = UNIT.record();
    seal(&mut long);
    assert_eq!(&long[10..12], &584u16.to_le_bytes());
    assert_eq!(long[576..580], crc32(&long[..576]).to_le_bytes());
    assert_eq!(&long[580..], b"CMIT");
}

#[test]
fn each_state_of_a_unit_reads_as_the_rule_names_it() {
    for layout in [&UNIT, &RECORD] {
        let valid = record(layout, 2, 9);
        let mut f = FakeJournalFlash::with_unit(256);
        assert_eq!(scan(&mut f, layout, 0), Found::Erased);

        place(&mut f, 0, &valid);
        assert_eq!(scan(&mut f, layout, 0), Found::Valid((2, 9)));

        // Programmed in order and cut short: no marker.
        for cut in [1, 5, 8, 9, 64, 124] {
            let mut f = FakeJournalFlash::with_unit(256);
            place(&mut f, 0, &valid[..cut]);
            assert_eq!(scan(&mut f, layout, 0), Found::Partial, "cut at {cut}");
        }

        // No marker, and not a prefix of this store's record.
        let mut other = valid;
        other[0] = b'Q';
        other[124..].fill(u8::MAX);
        let mut f = FakeJournalFlash::with_unit(256);
        place(&mut f, 0, &other);
        assert_eq!(scan(&mut f, layout, 0), Found::Unmarked);

        // The marker, but a bit gone from the body.
        let mut torn = valid;
        torn[40] = 0x7f;
        place(&mut f, 0, &torn);
        assert_eq!(scan(&mut f, layout, 0), Found::Torn);

        // Checked, but another store's magic or length, or fields the store
        // refuses.
        for (at, value) in [(0, b'Q'), (10, 64), (100, 1)] {
            let mut p = valid;
            p[at] = value;
            let crc = crc32(&p[..120]);
            p[120..124].copy_from_slice(&crc.to_le_bytes());
            place(&mut f, 0, &p);
            assert_eq!(scan(&mut f, layout, 0), Found::Invalid, "byte {at}");
        }

        // An older version, checked: superseded, never decoded.
        let mut p = valid;
        p[8] = 2;
        let crc = crc32(&p[..120]);
        p[120..124].copy_from_slice(&crc.to_le_bytes());
        place(&mut f, 0, &p);
        assert_eq!(scan(&mut f, layout, 0), Found::Older);

        // A newer version, checked.
        let mut p = valid;
        p[8] = 4;
        let crc = crc32(&p[..120]);
        p[120..124].copy_from_slice(&crc.to_le_bytes());
        place(&mut f, 0, &p);
        assert_eq!(scan(&mut f, layout, 0), Found::Future);

        // Written bytes after the record: only a unit read whole sees them.
        let mut f = FakeJournalFlash::with_unit(256);
        place(&mut f, 0, &valid);
        f.bytes[0][200] = 0;
        let expected = if layout.extent == Extent::Unit {
            Found::Invalid
        } else {
            Found::Valid((2, 9))
        };
        assert_eq!(scan(&mut f, layout, 0), expected);
        f.bytes[0][..RECORD_BYTES].fill(u8::MAX);
        let expected = if layout.extent == Extent::Unit {
            Found::Invalid
        } else {
            Found::Erased
        };
        assert_eq!(scan(&mut f, layout, 0), expected);

        // A failed read is no state at all.
        let mut f = FakeJournalFlash::with_unit(256);
        f.fail_call = 1;
        assert_eq!(units(&mut f, layout).scan(0, decode), None);
    }
}

#[test]
fn the_newest_valid_record_wins_unless_a_newer_version_is_present() {
    let v = |generation| Found::Valid(generation);
    let same = |&g: &u64| g;

    assert_eq!(newest(v(1), v(2), same), Pick::Newest(1, 2));
    assert_eq!(newest(v(3), v(2), same), Pick::Newest(0, 3));
    assert_eq!(newest(v(2), v(2), same), Pick::Newest(0, 2), "a tie");
    for other in [
        Found::Erased,
        Found::Partial,
        Found::Unmarked,
        Found::Torn,
        Found::Invalid,
    ] {
        assert_eq!(newest(v(5), other, same), Pick::Newest(0, 5));
        assert_eq!(newest(other, v(5), same), Pick::Newest(1, 5));
        assert_eq!(newest(other, Found::Erased, same), Pick::Nothing);
        assert_eq!(newest(Found::Future, other, same), Pick::Future);
    }
    assert_eq!(newest(v(9), Found::Future, same), Pick::Future);

    assert_eq!(target(0, 1), 0, "an empty store starts at unit 0");
    assert_eq!((target(4, 0), target(4, 1)), (1, 0));
}

/// Units holding `history` alternating records, as saves leave them; the
/// last record's unit.
fn saved(f: &mut FakeJournalFlash, layout: &'static Layout, history: u8) -> u32 {
    let mut slot = 0;
    for generation in 1..=u64::from(history) {
        slot = target(generation - 1, slot);
        let p = record(layout, generation, generation as u8);
        assert!(units(f, layout).write(slot, &p, Prepare::Erase));
    }
    slot
}

/// After a write cut short or failed, on units that held `before`: the
/// other unit is untouched, the written one holds its old record, a state a
/// cut leaves, or the new record, and the newest pick is the old record or
/// the new one.
fn check_cut(
    f: &mut FakeJournalFlash,
    layout: &'static Layout,
    before: &FakeJournalFlash,
    written: u32,
    history: u8,
    case: &str,
) {
    let (w, other) = (written as usize, 1 - written as usize);
    assert_eq!(
        f.bytes[other], before.bytes[other],
        "{case}: the other unit"
    );

    let new = (u64::from(history) + 1, history + 1);
    let was = scan(&mut before.clone(), layout, written);
    let found = scan(f, layout, written);
    assert!(
        matches!(
            found,
            Found::Erased | Found::Partial | Found::Unmarked | Found::Torn
        ) || found == Found::Valid(new)
            || found == was,
        "{case}: {found:?}"
    );
    if found == was {
        assert_eq!(f.bytes[w], before.bytes[w], "{case}: an unchanged unit");
    }

    let (a, b) = (scan(f, layout, 0), scan(f, layout, 1));
    let old = (history > 0).then(|| (u64::from(history), history));
    match newest(a, b, |&(generation, _)| generation) {
        Pick::Newest(_, value) => assert!(Some(value) == old || value == new, "{case}: {value:?}"),
        Pick::Nothing => assert_eq!(old, None, "{case}: the old record was lost"),
        Pick::Future => panic!("{case}: a newer version from nowhere"),
    }
}

#[test]
fn a_power_cut_at_every_mutation_leaves_the_old_record_or_the_new() {
    for layout in [&UNIT, &RECORD] {
        for unit in SIZES {
            for history in 0..3u8 {
                let mut base = FakeJournalFlash::with_unit(unit);
                let last = saved(&mut base, layout, history);
                let written = target(u64::from(history), last);
                let next = record(layout, u64::from(history) + 1, history + 1);

                let mut probe = base.clone();
                let start = probe.mutations;
                assert!(units(&mut probe, layout).write(written, &next, Prepare::Erase));
                let span = i64::from(probe.mutations - start);
                assert_eq!(span as usize, unit + RECORD_BYTES, "one erase, one record");

                for cut in 0..=span {
                    let mut f = base.clone();
                    f.budget = cut;
                    let ok = units(&mut f, layout).write(written, &next, Prepare::Erase);
                    f.budget = -1;

                    // The fake reports a cut even after its last mutation.
                    let case = format!("{layout:?} unit {unit} history {history} cut {cut}");
                    assert!(!ok, "{case}");
                    check_cut(&mut f, layout, &base, written, history, &case);

                    // The written unit takes the next write whatever it holds.
                    assert!(units(&mut f, layout).write(written, &next, Prepare::Erase));
                    assert_eq!(
                        scan(&mut f, layout, written),
                        Found::Valid((u64::from(history) + 1, history + 1))
                    );
                }
            }
        }
    }
}

#[test]
fn a_failure_at_every_call_leaves_the_old_record_or_the_new() {
    for layout in [&UNIT, &RECORD] {
        for unit in SIZES.into_iter().chain([SECTOR]) {
            for history in 0..3u8 {
                let mut base = FakeJournalFlash::with_unit(unit);
                let last = saved(&mut base, layout, history);
                let written = target(u64::from(history), last);
                let next = record(layout, u64::from(history) + 1, history + 1);

                let mut probe = base.clone();
                let start = probe.calls;
                assert!(units(&mut probe, layout).write(written, &next, Prepare::Erase));
                let calls = probe.calls - start;
                // Erase, the blank check (record, then the tail in chunks),
                // body, verify, marker, verify.
                assert!(calls >= 6);

                for failure in 1..=calls {
                    let mut f = base.clone();
                    f.fail_call = f.calls + failure;
                    let ok = units(&mut f, layout).write(written, &next, Prepare::Erase);
                    f.fail_call = 0;

                    let case = format!("{layout:?} unit {unit} history {history} call {failure}");
                    assert!(!ok, "{case}: a failed call fails the write");
                    check_cut(&mut f, layout, &base, written, history, &case);
                }
            }
        }
    }
}

#[test]
fn a_write_that_did_not_land_is_caught_by_its_checks() {
    for layout in [&UNIT, &RECORD] {
        let mut f = FakeJournalFlash::with_unit(256);
        let last = saved(&mut f, layout, 2);
        let next = record(layout, 3, 3);

        f.silent_program = true;
        assert!(!units(&mut f, layout).write(1 - last, &next, Prepare::Erase));
        f.silent_program = false;

        // An erase that did nothing leaves the old record, which the blank
        // check refuses to program over.
        f.silent_erase = true;
        place(&mut f, 1 - last as usize, &record(layout, 1, 1));
        assert!(!units(&mut f, layout).write(1 - last, &next, Prepare::Erase));
        assert_eq!(scan(&mut f, layout, 1 - last), Found::Valid((1, 1)));
        f.silent_erase = false;

        // Nor does a first provisioning program over anything.
        assert!(!units(&mut f, layout).write(1 - last, &next, Prepare::Erased));
    }
}

#[test]
fn unbound_units_refuse_everything_and_a_clear_erases_both() {
    let unbound: DualSlot<FlashIo> = DualSlot::unbound(&UNIT);
    assert_eq!(unbound.scan(0, decode), None);
    assert!(!unbound.writable() && !unbound.clear());
    assert!(!unbound.write(0, &record(&UNIT, 1, 1), Prepare::Erase));

    let mut f = FakeJournalFlash::with_unit(SECTOR);
    saved(&mut f, &UNIT, 2);
    f.bytes[1][4000] = 0;
    assert!(units(&mut f, &UNIT).clear());
    assert!(f.bytes.iter().flatten().all(|&b| b == u8::MAX));
}
