//! The plans store and its changes: saves alternate and reload, a power cut
//! keeps the old or the new plans, a newer schema is read-only, a record
//! that fails its checks is not read, and each change moves the marks and
//! the claim as the schedule relies on.

use wellfed_core::store::slot::{RECORD_BYTES, crc32};
use wellfed_device_api::mailbox::{Claim, PLANS, Plan, PlansChange, Table, Weekdays};
use wellfed_feeder::store::plans::{Plans, PlansStatus, PlansStore};
use wellfed_testkit::journal_flash::{FakeJournalFlash, FlashIo, UNIT};

fn open(f: &mut FakeJournalFlash) -> PlansStore<FlashIo> {
    let mut s = PlansStore::default();
    // SAFETY: the fake flash outlives the store in every test.
    unsafe { s.open(f.io()) };
    s
}

const BREAKFAST: Plan = Plan {
    minute: 7 * 60 + 30,
    portions: 2,
    days: Weekdays::EVERY_DAY,
    enabled: true,
};

const ONCE: Plan = Plan {
    days: Weekdays::ONCE,
    ..BREAKFAST
};

fn table(plans: &[(usize, Plan)]) -> Table {
    let mut table = [None; PLANS];
    for &(slot, plan) in plans {
        table[slot] = Some(plan);
    }
    table
}

/// Plans with breakfast in slot 0, saved at `at_s`.
fn breakfast(at_s: u32) -> Plans {
    let change = PlansChange::Replace {
        table: table(&[(0, BREAKFAST)]),
        at_s: Some(at_s),
    };
    Plans::default().changed(&change).unwrap()
}

fn claim(plan: u8, due_s: u32, high_water: u32) -> Claim {
    Claim {
        plan,
        due_s,
        high_water,
        disabled: false,
    }
}

#[test]
fn nothing_until_saved_then_alternate_and_reload() {
    let mut f = FakeJournalFlash::with_unit(UNIT);
    let mut s = open(&mut f);
    assert_eq!(s.status, PlansStatus::Empty);
    assert_eq!(s.plans, Plans::default());

    let first = breakfast(1_000);
    let second = first
        .changed(&PlansChange::Claim {
            claim: claim(0, 2_000, 4),
            revision: first.revision,
        })
        .unwrap();
    assert!(s.save(&first));
    assert!(s.save(&second));
    let r = open(&mut f);
    assert_eq!((r.status, r.sequence, r.slot), (PlansStatus::Loaded, 2, 1));
    assert_eq!(r.plans, second);
}

#[test]
fn power_cut_at_every_mutation_keeps_old_or_new_and_stays_writable() {
    let old = breakfast(1_000);
    let new = Plans::default()
        .changed(&PlansChange::Replace {
            table: table(&[(3, ONCE)]),
            at_s: None,
        })
        .unwrap();
    let mut probe = FakeJournalFlash::with_unit(UNIT);
    let mut s = open(&mut probe);
    assert!(s.save(&old));
    let start = probe.mutations;
    assert!(s.save(&new));
    let span = i64::from(probe.mutations - start);

    for cut in 0..=span {
        let mut f = FakeJournalFlash::with_unit(UNIT);
        let mut s = open(&mut f);
        assert!(s.save(&old));
        f.budget = cut;
        let _ = s.save(&new);
        f.budget = -1;

        let mut r = open(&mut f);
        assert!(r.plans == old || r.plans == new, "cut {cut}");
        assert!(r.writable(), "cut {cut}: {:?}", r.status);
        assert!(r.save(&new));
        assert_eq!(open(&mut f).plans, new);
    }
}

/// Rewrite slot 0's record with `change` applied and a valid checksum.
fn rewrite(f: &mut FakeJournalFlash, change: impl FnOnce(&mut [u8])) {
    let mut record = f.bytes[0][..RECORD_BYTES].to_vec();
    change(&mut record);
    let crc = crc32(&record[..120]);
    record[120..124].copy_from_slice(&crc.to_le_bytes());
    f.bytes[0][..RECORD_BYTES].copy_from_slice(&record);
}

#[test]
fn a_newer_schema_is_read_only_with_no_plans() {
    let mut f = FakeJournalFlash::with_unit(UNIT);
    let mut s = open(&mut f);
    assert!(s.save(&breakfast(1_000)));
    rewrite(&mut f, |record| record[8] = 2);

    let mut r = open(&mut f);
    assert_eq!(
        (r.status, r.plans),
        (PlansStatus::ReadOnly, Plans::default())
    );
    assert!(!r.writable());
    assert!(!r.save(&breakfast(2_000)));
}

#[test]
fn records_that_fail_their_checks_are_not_read() {
    type Change = fn(&mut [u8]);
    let cases: [(&str, Change); 8] = [
        ("minute past the day", |r| {
            r[21..23].copy_from_slice(&1440u16.to_le_bytes());
        }),
        ("no portions", |r| r[23] = 0),
        ("days past Sunday", |r| r[24] = 0x80),
        ("unknown plan flag", |r| r[20] |= 4),
        ("bytes in an empty slot", |r| r[26] = 1),
        ("claim past the last slot", |r| {
            r[110] = 1;
            r[111] = 10;
        }),
        ("unknown claim flag", |r| r[110] = 0xee),
        ("sequence 0", |r| r[12..16].fill(0)),
    ];
    for (name, change) in cases {
        let mut f = FakeJournalFlash::with_unit(UNIT);
        let mut s = open(&mut f);
        assert!(s.save(&breakfast(1_000)));
        rewrite(&mut f, change);
        let r = open(&mut f);
        assert_eq!(
            (r.status, r.plans),
            (PlansStatus::Empty, Plans::default()),
            "{name}"
        );
    }
}

#[test]
fn a_replace_starts_new_and_moved_plans_from_their_save() {
    let saved = breakfast(1_000);
    assert_eq!(saved.marks[0], 1_000);
    assert_eq!(saved.revision, 1);

    // Later: slot 0's portions change (its occurrences stay), slot 1 is
    // new and slot 2 moves; each moved or new plan starts at the save.
    let mut earlier = Plans::default()
        .changed(&PlansChange::Replace {
            table: table(&[(0, BREAKFAST), (2, BREAKFAST)]),
            at_s: Some(1_000),
        })
        .unwrap();
    earlier.marks[0] = 5_000;
    let next = earlier
        .changed(&PlansChange::Replace {
            table: table(&[
                (
                    0,
                    Plan {
                        portions: 3,
                        ..BREAKFAST
                    },
                ),
                (1, BREAKFAST),
                (
                    2,
                    Plan {
                        minute: 8 * 60,
                        ..BREAKFAST
                    },
                ),
            ]),
            at_s: Some(9_000),
        })
        .unwrap();
    assert_eq!(next.marks[..3], [5_000, 9_000, 9_000]);

    // A deleted slot forgets its mark; one saved without a clock has none.
    let cleared = next
        .changed(&PlansChange::Replace {
            table: table(&[(0, BREAKFAST), (4, BREAKFAST)]),
            at_s: None,
        })
        .unwrap();
    assert_eq!(cleared.marks[..5], [5_000, 0, 0, 0, 0]);
    assert_eq!(cleared.revision, next.revision + 1);
}

#[test]
fn a_claim_marks_its_occurrence_and_a_plan_that_runs_once_disables_itself() {
    let plans = Plans::default()
        .changed(&PlansChange::Replace {
            table: table(&[(0, BREAKFAST), (1, ONCE)]),
            at_s: Some(1_000),
        })
        .unwrap();

    // A repeating plan's claim leaves the table and its revision alone.
    let first = claim(0, 2_000, 4);
    let claimed = plans
        .changed(&PlansChange::Claim {
            claim: first,
            revision: plans.revision,
        })
        .unwrap();
    assert_eq!(claimed.claim, Some(first));
    assert_eq!(claimed.view(true).mark(0), Some(2_000));
    assert_eq!(claimed.revision, plans.revision);

    // The next claim folds the last into its mark; a plan that runs once
    // is disabled with its claim, which says so.
    let once = claim(1, 3_000, 5);
    let again = claimed
        .changed(&PlansChange::Claim {
            claim: once,
            revision: claimed.revision,
        })
        .unwrap();
    assert_eq!(again.marks[0], 2_000);
    assert_eq!(again.claim.map(|c| c.disabled), Some(true));
    assert_eq!(again.table[1].map(|p| p.enabled), Some(false));
    assert_eq!(again.revision, claimed.revision + 1);

    // Given back, the occurrence and the plan are as they were.
    let held = again.claim.unwrap();
    let released = again
        .changed(&PlansChange::Release { claim: held })
        .unwrap();
    assert_eq!(released.claim, None);
    assert_eq!(released.view(true).mark(1), Some(1_000));
    assert_eq!(released.table[1].map(|p| p.enabled), Some(true));
}

#[test]
fn a_change_against_plans_that_moved_on_is_dropped() {
    let plans = breakfast(1_000);
    let stale = PlansChange::Claim {
        claim: claim(0, 2_000, 4),
        revision: plans.revision + 1,
    };
    assert_eq!(plans.changed(&stale), None);
    let empty_slot = PlansChange::Pass {
        plan: 5,
        due_s: 2_000,
        revision: plans.revision,
    };
    assert_eq!(plans.changed(&empty_slot), None);
    let no_claim = PlansChange::Release {
        claim: claim(0, 2_000, 4),
    };
    assert_eq!(plans.changed(&no_claim), None);
    let disabled = Plans::default()
        .changed(&PlansChange::Replace {
            table: table(&[(
                0,
                Plan {
                    enabled: false,
                    ..BREAKFAST
                },
            )]),
            at_s: Some(1_000),
        })
        .unwrap();
    let claim_disabled = PlansChange::Claim {
        claim: claim(0, 2_000, 4),
        revision: disabled.revision,
    };
    assert_eq!(disabled.changed(&claim_disabled), None);
}

#[test]
fn a_settle_and_a_pass_spend_the_occurrence() {
    let plans = breakfast(1_000);
    let held = claim(0, 2_000, 4);
    let claimed = plans
        .changed(&PlansChange::Claim {
            claim: held,
            revision: plans.revision,
        })
        .unwrap();
    let settled = claimed
        .changed(&PlansChange::Settle { claim: held })
        .unwrap();
    assert_eq!((settled.claim, settled.marks[0]), (None, 2_000));

    let passed = settled
        .changed(&PlansChange::Pass {
            plan: 0,
            due_s: 9_000,
            revision: settled.revision,
        })
        .unwrap();
    assert_eq!(passed.marks[0], 9_000);
    // A mark never moves back.
    let behind = passed
        .changed(&PlansChange::Pass {
            plan: 0,
            due_s: 4_000,
            revision: passed.revision,
        })
        .unwrap();
    assert_eq!(behind.marks[0], 9_000);
}
