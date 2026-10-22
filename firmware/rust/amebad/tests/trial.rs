//! FW11 KM0 boot-trial decisions: no flash or register access here.

use wellfed_amebad::trial::{Decision, LIMIT, decide};
use wellfed_core::ota::{CONFIRM_LEN, MARKER, confirm_record};

const ID: [u8; 32] = [0x5a; 32];
const CLEARED: [u8; 8] = *b"\x001958711";
const NONE: [u8; CONFIRM_LEN] = [0xff; CONFIRM_LEN];

/// Boot repeatedly without power loss; KM0's own boot count is bumped too.
fn boots(n: usize, bank0: [u8; 8], record: [u8; CONFIRM_LEN]) -> Vec<Decision> {
    let mut register = 0;
    let mut out = Vec::new();
    for _ in 0..n {
        register = (register & 0xffff_0000) | ((register + 1) & 0xffff);
        let (decision, value) = decide(&MARKER, &bank0, &record, register, &ID);
        assert_eq!(value & 0xffff, register & 0xffff, "the boot count is kept");
        register = value;
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
fn a_confirmed_image_is_never_counted() {
    let record = confirm_record(&ID, 0);
    assert!(
        boots(10, MARKER, record)
            .iter()
            .all(|d| *d == Decision::Confirmed)
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
fn running_bank_0_runs_no_trial_and_clears_the_count() {
    let register = (0x5a << 24) | (2 << 16) | 7;
    assert_eq!(
        decide(&CLEARED, &MARKER, &NONE, register, &ID),
        (Decision::NotBank1, 7)
    );
}

#[test]
fn a_count_from_another_image_does_not_carry_over() {
    let register = (0x11 << 24) | (3 << 16) | 9;
    let (decision, value) = decide(&MARKER, &MARKER, &NONE, register, &ID);
    assert_eq!(decision, Decision::Counted(1));
    assert_eq!(value, (0x5a << 24) | (1 << 16) | 9);
}

#[test]
fn codes_carry_the_count() {
    assert_eq!(Decision::Counted(2).code(), 0x202);
    assert_eq!(Decision::GiveWay.code(), 3);
    assert_eq!(Decision::NoFallback(3).code(), 0x304);
}
