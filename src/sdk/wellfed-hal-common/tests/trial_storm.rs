//! The storm count against the trial: a count another build left under a
//! shared tag never skips a trial, and a bank 0 that turns valid ends a
//! spent trial's run; and a fault that lets held boots through their hold
//! backs off and steps down again.

use wellfed_core::ota::{CONFIRM_LEN, MARKER};
use wellfed_hal_common::trial::{
    Decision, LIMIT, MAX_LEVEL, STORM_LIMIT, STORM_STATE, Storm, decide, deliberate, fresh,
    hold_ms, level, past_trial, released, resets, settled,
};

const ID: [u8; 32] = [0x5a; 32];
const CLEARED: [u8; 8] = *b"\x001958711";
const NONE: [u8; CONFIRM_LEN] = [0xff; CONFIRM_LEN];

fn bump(register: u32) -> u32 {
    (register & 0xffff_0000) | ((register + 1) & 0xffff)
}

/// Boot an unconfirmed bank 1 `n` times from `register`, every boot ending
/// in a reset nobody asked for.
fn trial_boots(mut register: u32, bank0: [u8; 8], n: usize) -> (Vec<Decision>, u32) {
    let mut out = Vec::new();
    for _ in 0..n {
        let (decision, value) = decide(&MARKER, &bank0, &NONE, bump(register), &ID);
        register = value;
        out.push(decision);
        if decision == Decision::GiveWay {
            break;
        }
    }
    (out, register)
}

#[test]
fn a_count_under_a_shared_tag_never_skips_a_new_builds_trial() {
    // Bank 0 ran with the same tag (a reinstall of the same identity, or
    // one identity byte in 256): storm state, a level and a count, then a
    // reboot that kept the low nibble.
    let left = (0x5a << 24) | (3 << 20) | ((STORM_STATE | 2) << 16) | 40;
    for register in [left, deliberate(left)] {
        assert!(past_trial(register, &ID));
        let (out, _) = trial_boots(register, MARKER, 6);
        assert_eq!(
            out,
            [
                Decision::Counted(1),
                Decision::Counted(2),
                Decision::Counted(3),
                Decision::GiveWay
            ],
            "{register:#x}"
        );
    }

    // The activation's restart leaves no count at all.
    assert_eq!(fresh(left), 40);
    let (decision, value) = decide(&MARKER, &MARKER, &NONE, bump(fresh(left)), &ID);
    assert_eq!(decision, Decision::Counted(1));
    assert!(!past_trial(value, &ID));
}

#[test]
fn a_spent_trial_goes_back_on_trial_once_bank_0_is_valid_again() {
    // Bank 0 invalid: the trial runs out and the build runs on, held.
    let (out, register) = trial_boots(0, CLEARED, 5);
    assert_eq!(out[3], Decision::NoFallback(LIMIT));
    assert!(past_trial(register, &ID));

    // Bank 0 is valid again (reinstalled over UART): a fresh trial, which
    // gives way after its three boots, not at once.
    let (out, _) = trial_boots(register, MARKER, 6);
    assert_eq!(
        out,
        [
            Decision::Counted(1),
            Decision::Counted(2),
            Decision::Counted(3),
            Decision::GiveWay
        ]
    );
}

/// Bank 0 (no fallback), minute by minute: every normal boot faults a
/// minute in, and every held boot lasts its hold, is released, and faults
/// a minute later. The minute of every reset and the highest level held.
fn released_boots_fault(minutes: u32) -> (Vec<u32>, u32) {
    let mut register = 0;
    let mut minute = 0;
    let mut resets_at = Vec::new();
    let mut highest = 0;
    while minute < minutes {
        register = decide(&CLEARED, &MARKER, &NONE, bump(register), &ID).1;
        match Storm::of(resets(register, &ID)) {
            Storm::Held => {
                let held = level(register, &ID);
                highest = highest.max(held);
                minute += hold_ms(held) / 60_000;
                register = released(register, &ID);
                minute += 1;
            }
            Storm::Normal => minute += 1,
            Storm::Halted => panic!("held boots that last their hold never halt"),
        }
        resets_at.push(minute);
    }
    (resets_at, highest)
}

#[test]
fn a_fault_that_lets_held_boots_through_backs_off_and_steps_down_again() {
    let (resets_at, highest) = released_boots_fault(48 * 60);
    assert_eq!(highest, MAX_LEVEL, "the holds grew to the cap");

    // Once at the cap, a cycle is one burst of early resets and the
    // released boot's, every 160 minutes or more: a few an hour.
    let late = resets_at.iter().filter(|&&m| m >= 12 * 60).count();
    let cycles = (36 * 60) / (hold_ms(MAX_LEVEL) / 60_000);
    assert!(
        late <= (cycles as usize + 1) * (STORM_LIMIT as usize + 1),
        "{late} resets in 36 h"
    );

    // The fault clears: each stable boot takes a level back, to none.
    let mut register = (0x5a << 24) | ((STORM_STATE | MAX_LEVEL) << 16) | 9;
    for expected in (0..MAX_LEVEL).rev() {
        register = settled(bump(register), &ID);
        assert_eq!(level(register, &ID), expected);
    }
    register = settled(bump(register), &ID);
    assert_eq!(level(register, &ID), 0);
}
