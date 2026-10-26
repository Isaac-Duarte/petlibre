//! KM0 boot-trial and reset-storm decisions, and KM4's side of the storm
//! count: no flash or register access here.

use wellfed_core::ota::{CONFIRM_LEN, MARKER, confirm_record};
use wellfed_hal_common::trial::{
    Decision, HALT_LIMIT, LIMIT, STORM_LIMIT, Storm, decide, deliberate, resets, settled,
};

const ID: [u8; 32] = [0x5a; 32];
/// A marker whose first byte was cleared: what give-way leaves behind.
const CLEARED: [u8; 8] = *b"\x001958711";
const NONE: [u8; CONFIRM_LEN] = [0xff; CONFIRM_LEN];

/// KM0's own boot count, bumped at every boot.
fn bump(register: u32) -> u32 {
    (register & 0xffff_0000) | ((register + 1) & 0xffff)
}

/// Boot repeatedly without power loss; KM0's own boot count is bumped too.
fn boots(n: usize, bank0: [u8; 8], record: [u8; CONFIRM_LEN]) -> Vec<Decision> {
    boots_from(n, &MARKER, bank0, record, |_| None)
}

/// As `boots`, with `km4(register)` standing for what KM4 writes before the
/// boot's reset: `None` for a reset nobody asked for.
fn boots_from(
    n: usize,
    bank1: &[u8; 8],
    bank0: [u8; 8],
    record: [u8; CONFIRM_LEN],
    km4: impl Fn(u32) -> Option<u32>,
) -> Vec<Decision> {
    let mut register = 0;
    let mut out = Vec::new();
    for _ in 0..n {
        register = bump(register);
        let (decision, value) = decide(bank1, &bank0, &record, register, &ID);
        assert_eq!(value & 0xffff, register & 0xffff, "the boot count is kept");
        register = km4(value).unwrap_or(value);
        out.push(decision);
    }
    out
}

#[test]
fn an_unconfirmed_image_gives_way_after_the_limit_with_bank_0_valid() {
    assert_eq!(LIMIT, 3);
    assert_eq!(
        boots(4, MARKER, NONE),
        [
            Decision::Counted(1),
            Decision::Counted(2),
            Decision::Counted(3),
            Decision::GiveWay
        ]
    );
}

#[test]
fn a_confirmed_image_runs_a_trial_never() {
    let record = confirm_record(&ID, 0);
    let stable = |register| Some(settled(register, &ID));
    assert!(
        boots_from(20, &MARKER, MARKER, record, stable)
            .iter()
            .all(|d| *d == Decision::Confirmed { resets: 0 })
    );

    // A record for another build does not count.
    let other = confirm_record(&[1; 32], 0);
    assert_eq!(boots(1, MARKER, other), [Decision::Counted(1)]);
}

#[test]
fn without_a_valid_bank_0_it_keeps_bank_1() {
    let out = boots(6, CLEARED, NONE);
    assert_eq!(
        &out[..3],
        [
            Decision::Counted(1),
            Decision::Counted(2),
            Decision::Counted(3)
        ]
    );
    assert!(out[3..].iter().all(|d| *d == Decision::NoFallback(3)));
}

#[test]
fn running_bank_0_runs_no_trial_and_clears_the_trial_count() {
    // Register layout: tag in bits 24-31, storm count 20-23, trial count
    // 16-19, KM0 boot count 0-15.
    let register = (0x5a << 24) | (2 << 16) | 7;
    let (decision, value) = decide(&CLEARED, &MARKER, &NONE, register, &ID);
    assert_eq!(decision, Decision::NotBank1 { resets: 0 });
    assert_eq!(value, (0x5a << 24) | (1 << 20) | 7, "this boot counts");
}

#[test]
fn a_count_from_another_image_does_not_carry_over() {
    let register = (0x11 << 24) | (3 << 16) | (4 << 20) | 9;
    let (decision, value) = decide(&MARKER, &MARKER, &NONE, register, &ID);
    assert_eq!(decision, Decision::Counted(1));
    assert_eq!(value, (0x5a << 24) | (1 << 16) | 9);

    let record = confirm_record(&ID, 0);
    let (decision, _) = decide(&MARKER, &MARKER, &record, register, &ID);
    assert_eq!(decision, Decision::Confirmed { resets: 0 });
}

#[test]
fn codes_carry_the_counts() {
    assert_eq!(Decision::NotBank1 { resets: 0 }.code(), 0);
    assert_eq!(Decision::NotBank1 { resets: 2 }.code(), 0x200);
    assert_eq!(Decision::Confirmed { resets: 1 }.code(), 0x101);
    assert_eq!(Decision::Counted(2).code(), 0x202);
    assert_eq!(Decision::GiveWay.code(), 3);
    assert_eq!(Decision::NoFallback(3).code(), 0x304);
    assert_eq!(Decision::Held { resets: 5 }.code(), 0x505);
    assert_eq!(Decision::StormGiveWay { resets: 5 }.code(), 0x506);
    // `firstboot_km0.c` prints `limit=5` with the storm give-way.
    assert_eq!(STORM_LIMIT, 5);
}

#[test]
fn a_confirmed_build_that_keeps_resetting_gives_way_at_the_storm_limit() {
    let record = confirm_record(&ID, 0);
    let out = boots_from(7, &MARKER, MARKER, record, |_| None);
    let expected: Vec<_> = (0..STORM_LIMIT)
        .map(|resets| Decision::Confirmed { resets })
        .chain([Decision::StormGiveWay {
            resets: STORM_LIMIT,
        }])
        .collect();
    assert_eq!(out[..6], expected[..]);

    // The give-way leaves no count behind for bank 0, even for the same
    // build there.
    let full = (0x5a << 24) | (STORM_LIMIT << 20) | 4;
    let (decision, value) = decide(&MARKER, &MARKER, &record, full, &ID);
    assert_eq!(
        decision,
        Decision::StormGiveWay {
            resets: STORM_LIMIT
        }
    );
    assert_eq!(value, 4);
}

#[test]
fn a_boot_that_runs_stable_or_restarts_on_purpose_ends_the_burst() {
    let record = confirm_record(&ID, 0);
    // Every third boot runs past the stable window: never a storm.
    let count = std::cell::Cell::new(0);
    let sometimes = |register| {
        count.set(count.get() + 1);
        (count.get() % 3 == 0).then(|| settled(register, &ID))
    };
    let out = boots_from(30, &MARKER, MARKER, record, sometimes);
    assert!(
        out.iter()
            .all(|d| matches!(d, Decision::Confirmed { resets } if *resets < 3)),
        "{out:?}"
    );

    // Requested restarts are no faults at all.
    let out = boots_from(30, &MARKER, MARKER, record, |r| Some(deliberate(r)));
    assert!(out.iter().all(|d| *d == Decision::Confirmed { resets: 0 }));
}

#[test]
fn a_storm_with_nothing_to_fall_back_to_holds_then_halts() {
    // Bank 0 itself: early resets every boot.
    let out = boots_from(12, &CLEARED, MARKER, NONE, |_| None);
    for (resets, decision) in (0u32..).zip(&out) {
        let expected = if resets < STORM_LIMIT {
            Decision::NotBank1 { resets }
        } else {
            Decision::Held {
                resets: resets.min(15),
            }
        };
        assert_eq!(*decision, expected);
    }

    // A confirmed bank 1 with no valid bank 0 holds the same way.
    let record = confirm_record(&ID, 0);
    let out = boots_from(6, &MARKER, CLEARED, record, |_| None);
    assert_eq!(
        out[5],
        Decision::Held {
            resets: STORM_LIMIT
        }
    );
}

#[test]
fn km4_reads_the_resets_before_this_boot_and_acts_on_them() {
    let register = |storm: u32| (0x5a << 24) | (storm << 20) | 3;
    // KM0 counted this boot already.
    assert_eq!(resets(register(1), &ID), 0);
    assert_eq!(resets(register(6), &ID), 5);
    // Another image's count is none of this one's.
    assert_eq!(resets((0x11 << 24) | (6 << 20), &ID), 0);
    assert_eq!(resets(0, &ID), 0);

    assert_eq!(Storm::of(STORM_LIMIT - 1), Storm::Normal);
    assert_eq!(Storm::of(STORM_LIMIT), Storm::Held);
    assert_eq!(Storm::of(HALT_LIMIT - 1), Storm::Held);
    assert_eq!(Storm::of(HALT_LIMIT), Storm::Halted);
}

#[test]
fn a_held_boot_that_runs_stable_stays_held_and_a_reboot_frees_it() {
    let held = (0x5a << 24) | ((STORM_LIMIT + 2) << 20) | 3;
    assert_eq!(Storm::of(resets(held, &ID)), Storm::Held);
    let calm = settled(held, &ID);
    assert_eq!(calm, (0x5a << 24) | (STORM_LIMIT << 20) | 3);
    // The next boot is still held, with the halt a whole burst away.
    let (decision, value) = decide(&CLEARED, &MARKER, &NONE, bump(calm), &ID);
    assert_eq!(
        decision,
        Decision::Held {
            resets: STORM_LIMIT
        }
    );
    assert_eq!(Storm::of(resets(value, &ID)), Storm::Held);

    // A normal boot's stable run clears the count; the owner's reboot
    // clears it whatever it was.
    let normal = (0x5a << 24) | (3 << 20) | 3;
    assert_eq!(settled(normal, &ID), (0x5a << 24) | 3);
    assert_eq!(deliberate(held), (0x5a << 24) | 3);
}

#[test]
fn trial_counts_never_become_storm_counts() {
    // Two failed trial boots, then the build confirms on the third.
    let mut register = 0;
    for _ in 0..3 {
        register = bump(register);
        register = decide(&MARKER, &MARKER, &NONE, register, &ID).1;
    }
    let record = confirm_record(&ID, 0);
    let (decision, value) = decide(&MARKER, &MARKER, &record, bump(register), &ID);
    assert_eq!(decision, Decision::Confirmed { resets: 0 });
    assert_eq!((value >> 16) & 0xf, 0, "the trial count is gone");
}

/// The whole board, boot after boot: the loader's choice, KM0's decision
/// and the give-way's marker write, then KM4, which halts or resets early
/// (a fault at every boot, on either bank). `cut_after` power-ons a cut
/// clears the backup registers, as power loss does.
fn faulting_board(cut_after: Option<u32>, boots: u32) -> (u32, Option<u32>, bool) {
    let record = confirm_record(&ID, 0);
    let mut bank1 = MARKER;
    let mut register = 0;
    // Resets since the last power-on, and the most of them.
    let (mut resets_now, mut resets_max) = (0, 0);
    for boot in 1..=boots {
        if cut_after.is_some_and(|n| boot % n == 0) {
            register = 0;
            resets_now = 0;
        }
        register = bump(register);
        let (decision, value) = decide(&bank1, &MARKER, &record, register, &ID);
        register = value;
        if matches!(decision, Decision::StormGiveWay { .. }) {
            bank1 = CLEARED;
        } else if Storm::of(resets(register, &ID)) == Storm::Halted {
            return (resets_max, Some(boot), bank1 == CLEARED);
        }
        resets_now += 1;
        resets_max = resets_max.max(resets_now);
    }
    (resets_max, None, bank1 == CLEARED)
}

#[test]
fn a_build_that_faults_at_every_boot_never_resets_forever() {
    // Bank 1's storm gives way, bank 0's holds, then halts: within one
    // power-on, the board resets a bounded number of times and then stays
    // up, stopped.
    let bound = STORM_LIMIT + 1 + HALT_LIMIT;
    let (resets, halted, gave_way) = faulting_board(None, 1_000);
    assert!(gave_way);
    assert_eq!(halted, Some(bound + 1));
    assert_eq!(resets, bound);

    // Power cuts only restart the counts: however they fall, no power-on
    // sees more resets than that, and a cut mid-storm never loses the
    // give-way already made.
    for cut_after in 2..=20 {
        let (resets, _, gave_way) = faulting_board(Some(cut_after), 500);
        assert!(resets <= bound, "cut every {cut_after}: {resets}");
        // A power-on that boots `STORM_LIMIT + 1` times sees the give-way.
        assert_eq!(gave_way, cut_after > STORM_LIMIT, "cut every {cut_after}");
    }
}
