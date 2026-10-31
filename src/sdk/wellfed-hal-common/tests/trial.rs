//! KM0 boot-trial and reset-storm decisions, and KM4's side of the storm
//! count: no flash or register access here.

use wellfed_core::ota::{CONFIRM_LEN, MARKER, confirm_record};
use wellfed_hal_common::trial::{
    Decision, HALT_LIMIT, HALT_MS, LIMIT, MAX_LEVEL, STABLE_MS, STORM_LIMIT, STORM_STATE, Storm,
    decide, deliberate, halt_over, hold_ms, level, past_trial, released, resets, settled,
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
fn without_a_valid_bank_0_a_spent_trial_runs_on_held_never_resetting_forever() {
    let out = boots(6, CLEARED, NONE);
    assert_eq!(
        out[..4],
        [
            Decision::Counted(1),
            Decision::Counted(2),
            Decision::Counted(3),
            Decision::NoFallback(3),
        ]
    );
    // From there it is a build in no trial, held as after a storm.
    assert_eq!(
        out[4..],
        [
            Decision::Held {
                resets: STORM_LIMIT + 1
            },
            Decision::Held {
                resets: STORM_LIMIT + 2
            },
        ]
    );

    // KM4 sees it held and past its trial: no deadline resets it.
    let mut register = 0;
    for _ in 0..4 {
        register = decide(&MARKER, &CLEARED, &NONE, bump(register), &ID).1;
    }
    assert!(past_trial(register, &ID));
    assert_eq!(Storm::of(resets(register, &ID)), Storm::Held);

    // Once its hold ends it runs as a normal build, its resets a storm's.
    let freed = released(register, &ID);
    let (decision, value) = decide(&MARKER, &CLEARED, &NONE, bump(freed), &ID);
    assert_eq!(decision, Decision::NoFallback(LIMIT));
    assert_eq!(Storm::of(resets(value, &ID)), Storm::Normal);
    assert!(past_trial(value, &ID));
}

#[test]
fn running_bank_0_runs_no_trial_and_clears_the_trial_count() {
    // Register layout: tag in bits 24-31, storm count 20-23, trial count
    // (or, with bit 19, the storm's level) 16-19, KM0 boot count 0-15.
    let register = (0x5a << 24) | (2 << 16) | 7;
    let (decision, value) = decide(&CLEARED, &MARKER, &NONE, register, &ID);
    assert_eq!(decision, Decision::NotBank1 { resets: 0 });
    assert_eq!(
        value,
        (0x5a << 24) | (1 << 20) | (STORM_STATE << 16) | 7,
        "this boot counts"
    );
    assert!(!past_trial(register, &ID));
    assert!(past_trial(value, &ID));
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
    let full = (0x5a << 24) | (STORM_LIMIT << 20) | (STORM_STATE << 16) | 4;
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
fn a_storm_with_nothing_to_fall_back_to_holds() {
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
fn holds_double_with_each_storm_in_a_row_and_stable_boots_step_them_down() {
    assert_eq!(hold_ms(0), STABLE_MS);
    assert_eq!(hold_ms(1), 2 * STABLE_MS);
    assert_eq!(hold_ms(MAX_LEVEL), STABLE_MS << MAX_LEVEL);
    assert_eq!(hold_ms(MAX_LEVEL + 3), hold_ms(MAX_LEVEL), "capped");

    // A held boot that ran its hold: the owner opens, the count starts
    // again, and the next storm holds a level longer.
    let held = (0x5a << 24) | ((STORM_LIMIT + 1) << 20) | (STORM_STATE << 16) | 3;
    assert_eq!(Storm::of(resets(held, &ID)), Storm::Held);
    assert_eq!(level(held, &ID), 0);
    let freed = released(held, &ID);
    let (decision, value) = decide(&CLEARED, &MARKER, &NONE, bump(freed), &ID);
    assert_eq!(decision, Decision::NotBank1 { resets: 0 });
    assert_eq!(level(value, &ID), 1);

    // Up to the cap, never past it.
    let mut register = freed;
    for _ in 0..10 {
        register = released(register, &ID);
    }
    assert_eq!(level(register, &ID), MAX_LEVEL);

    // Each stable boot steps it down; a reboot keeps the level but ends
    // the burst.
    let calmer = settled(register, &ID);
    assert_eq!(level(calmer, &ID), MAX_LEVEL - 1);
    assert_eq!(resets(bump(calmer), &ID), 0);
    let rebooted = deliberate(held);
    let (decision, _) = decide(&CLEARED, &MARKER, &NONE, bump(rebooted), &ID);
    assert_eq!(decision, Decision::NotBank1 { resets: 0 });
}

#[test]
fn a_halt_ends_in_a_held_boot_a_level_longer() {
    let halted = (0x5a << 24) | ((HALT_LIMIT + 1) << 20) | (STORM_STATE << 16) | 3;
    assert_eq!(Storm::of(resets(halted, &ID)), Storm::Halted);
    let over = halt_over(halted, &ID);
    let (decision, value) = decide(&CLEARED, &MARKER, &NONE, bump(over), &ID);
    assert_eq!(
        decision,
        Decision::Held {
            resets: STORM_LIMIT
        }
    );
    assert_eq!(Storm::of(resets(value, &ID)), Storm::Held);
    assert_eq!(level(value, &ID), 1);
}

#[test]
fn trial_counts_never_become_storm_counts() {
    // Two failed trial boots, then the build confirms on the third.
    let mut register = 0;
    for _ in 0..3 {
        register = bump(register);
        register = decide(&MARKER, &MARKER, &NONE, register, &ID).1;
    }
    assert!(!past_trial(register, &ID));
    let record = confirm_record(&ID, 0);
    let (decision, value) = decide(&MARKER, &MARKER, &record, bump(register), &ID);
    assert_eq!(decision, Decision::Confirmed { resets: 0 });
    assert_eq!(level(value, &ID), 0, "the trial count is no level");
    assert_eq!((value >> 16) & 0xf, STORM_STATE);
}

/// The whole board, minute by minute: the loader's choice, KM0's decision
/// and the give-way's marker write, then KM4, which halts for `HALT_MS`
/// or resets a minute into the boot (a fault at every boot, on either
/// bank), with a power cut every `cut_every` minutes. The minute of every
/// reset, and whether bank 1 gave way.
fn faulting_board(cut_every: Option<u32>, minutes: u32) -> (Vec<u32>, bool) {
    let record = confirm_record(&ID, 0);
    let mut bank1 = MARKER;
    let mut register = 0;
    let mut resets_at = Vec::new();
    let mut minute = 0;
    while minute < minutes {
        if cut_every.is_some_and(|n| minute % n == 0) && minute != 0 {
            register = 0;
        }
        register = bump(register);
        let (decision, value) = decide(&bank1, &MARKER, &record, register, &ID);
        register = value;
        if matches!(decision, Decision::StormGiveWay { .. }) {
            bank1 = CLEARED;
            resets_at.push(minute);
            continue;
        }
        if Storm::of(resets(register, &ID)) == Storm::Halted {
            minute += HALT_MS / 60_000;
            register = halt_over(register, &ID);
        } else {
            minute += 1;
        }
        resets_at.push(minute);
    }
    (resets_at, bank1 == CLEARED)
}

#[test]
fn a_build_that_faults_at_every_boot_resets_only_a_few_times_an_hour() {
    // Bank 1's storm gives way; bank 0's holds, then halts for an hour,
    // then holds again: the board never stops trying, and never resets
    // fast for long.
    let (resets, gave_way) = faulting_board(None, 48 * 60);
    assert!(gave_way);
    let first_halt = STORM_LIMIT + 1 + HALT_LIMIT;
    // After the first burst, each hour sees the held boots' few resets.
    let late = resets.iter().filter(|&&m| m >= 2 * 60).count();
    let hours = 46;
    assert!(
        late <= hours * (HALT_LIMIT - STORM_LIMIT + 1) as usize,
        "{late} resets in {hours} h"
    );
    let early = resets.iter().filter(|&&m| m < 60).count();
    assert!(early <= first_halt as usize + 1, "{early}");

    // Power cuts only restart the counts: however they fall, no hour sees
    // more resets than one burst, and a cut mid-storm never loses the
    // give-way already made.
    for cut_every in 2..=20 {
        let (resets, gave_way) = faulting_board(Some(cut_every), 24 * 60);
        assert_eq!(gave_way, cut_every > STORM_LIMIT, "cut every {cut_every}");
        if cut_every > first_halt {
            for hour in 0..24 {
                let n = resets.iter().filter(|&&m| m / 60 == hour).count();
                assert!(n <= 2 * first_halt as usize, "hour {hour}: {n}");
            }
        }
    }
}

/// The handover erases the confirmation on bank 0, where it decides nothing:
/// erased, torn or whole, bank 0 boots the same, so a cut through that erase
/// changes no boot. The same build in bank 1 afterwards runs a real trial,
/// whatever count the boots before left under its tag.
#[test]
fn a_forgotten_confirmation_changes_only_the_next_bank_1_boot() {
    let whole = confirm_record(&ID, 5);
    let mut torn = whole;
    torn[20..].fill(0xff);

    let mut register = 0;
    for record in [&whole, &torn, &NONE] {
        let (decision, next) = decide(&CLEARED, &MARKER, record, register, &ID);
        assert_eq!(decision, Decision::NotBank1 { resets: 0 }, "{record:?}");
        register = deliberate(next);
    }

    for record in [&torn, &NONE] {
        let (decision, _) = decide(&MARKER, &MARKER, record, register, &ID);
        assert_eq!(decision, Decision::Counted(1));
    }
    let (decision, _) = decide(&MARKER, &MARKER, &whole, register, &ID);
    assert_eq!(decision, Decision::Confirmed { resets: 0 });
}
