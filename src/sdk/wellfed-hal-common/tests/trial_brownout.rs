//! Brownout boots against the storm count: a supply that browns out never
//! makes a storm, a fault's resets still do with brownouts between them,
//! and a trial counts a brownout boot like any other.

use wellfed_core::ota::{CONFIRM_LEN, MARKER, confirm_record};
use wellfed_hal_common::trial::{
    Decision, Reset, STORM_LIMIT, STORM_STATE, decide, forgiven, resets,
};

const ID: [u8; 32] = [0x5a; 32];
const CLEARED: [u8; 8] = *b"\x001958711";
const NONE: [u8; CONFIRM_LEN] = [0xff; CONFIRM_LEN];

/// One KM0 boot as `wellfed_amebad_trial_decide` runs it: KM0's own boot
/// count bumped, the brownout taken back, then the decision.
fn boot(
    register: u32,
    bank1: &[u8; 8],
    bank0: &[u8; 8],
    record: &[u8; CONFIRM_LEN],
    reset: Reset,
) -> (Decision, u32) {
    let bumped = (register & 0xffff_0000) | ((register + 1) & 0xffff);
    decide(bank1, bank0, record, forgiven(bumped, &ID, reset), &ID)
}

#[test]
fn a_confirmed_bank_1_that_keeps_browning_out_never_gives_way() {
    let record = confirm_record(&ID, 0);
    let mut register = 0;

    // The first boot is a power-on; every later one follows a brownout
    // within seconds of the boot before it (a pack failing at idle).
    let (decision, value) = boot(register, &MARKER, &MARKER, &record, Reset::Other);
    assert_eq!(decision, Decision::Confirmed { resets: 0 });
    register = value;
    for _ in 0..(4 * STORM_LIMIT) {
        let (decision, value) = boot(register, &MARKER, &MARKER, &record, Reset::Brownout);
        assert_eq!(decision, Decision::Confirmed { resets: 0 });
        assert_eq!(resets(value, &ID), 0, "KM4 sees no early resets");
        register = value;
    }
}

#[test]
fn bank_0_browning_out_is_never_held() {
    let mut register = 0;
    for _ in 0..(4 * STORM_LIMIT) {
        let (decision, value) = boot(register, &CLEARED, &MARKER, &NONE, Reset::Brownout);
        assert_eq!(decision, Decision::NotBank1 { resets: 0 });
        register = value;
    }
}

#[test]
fn a_faults_resets_still_make_a_storm_with_brownouts_between_them() {
    let record = confirm_record(&ID, 0);
    let mut register = 0;
    let mut decisions = Vec::new();

    // Power-on, then the cause of each later reset in turn: the faults
    // count, the brownouts neither count nor end the burst.
    let causes = [
        Reset::Other,
        Reset::Other,
        Reset::Brownout,
        Reset::Other,
        Reset::Brownout,
        Reset::Brownout,
        Reset::Other,
        Reset::Other,
    ];
    for reset in causes {
        let (decision, value) = boot(register, &MARKER, &MARKER, &record, reset);
        decisions.push(decision);
        register = value;
    }

    let confirmed = |resets| Decision::Confirmed { resets };
    assert_eq!(
        decisions,
        [
            confirmed(0),
            confirmed(1),
            confirmed(1),
            confirmed(2),
            confirmed(2),
            confirmed(2),
            confirmed(3),
            confirmed(4),
        ]
    );
    let (decision, _) = boot(register, &MARKER, &MARKER, &record, Reset::Other);
    assert_eq!(
        decision,
        Decision::StormGiveWay {
            resets: STORM_LIMIT
        }
    );
}

#[test]
fn a_trial_counts_a_brownout_boot_and_still_gives_way() {
    let mut register = 0;
    let mut decisions = Vec::new();
    for _ in 0..4 {
        let (decision, value) = boot(register, &MARKER, &MARKER, &NONE, Reset::Brownout);
        decisions.push(decision);
        register = value;
    }
    assert_eq!(
        decisions,
        [
            Decision::Counted(1),
            Decision::Counted(2),
            Decision::Counted(3),
            Decision::GiveWay
        ]
    );
}

#[test]
fn forgiving_moves_only_this_images_storm_count() {
    // Storm state at level 2, three resets pending, KM0's count 40.
    let register = (0x5a << 24) | (3 << 20) | ((STORM_STATE | 2) << 16) | 40;
    assert_eq!(
        forgiven(register, &ID, Reset::Brownout),
        (0x5a << 24) | (2 << 20) | ((STORM_STATE | 2) << 16) | 40
    );
    assert_eq!(forgiven(register, &ID, Reset::Other), register);

    // Nothing pending, a trial's count, or another image's register: as is.
    let settled = (0x5a << 24) | ((STORM_STATE | 2) << 16) | 40;
    let trial = (0x5a << 24) | (2 << 16) | 40;
    let other = (0x11 << 24) | (3 << 20) | (STORM_STATE << 16) | 40;
    for register in [settled, trial, other, 0] {
        assert_eq!(forgiven(register, &ID, Reset::Brownout), register);
    }
}
