//! Brownout boots against the storm count: one brownout is forgiven, never
//! two boots in a row, so a brownout bit stuck set (the loader leaves it
//! when it finds no flag) cannot defeat the storm's bounds; a trial counts
//! a brownout boot like any other; and a brownout in each storm state.

use wellfed_core::ota::{CONFIRM_LEN, MARKER, confirm_record};
use wellfed_hal_common::trial::{
    Decision, FORGIVEN, HALT_LIMIT, Reset, STORM_LIMIT, STORM_STATE, Storm, decide_after,
    deliberate, halt_over, resets, settled,
};

const ID: [u8; 32] = [0x5a; 32];
const CLEARED: [u8; 8] = *b"\x001958711";
const NONE: [u8; CONFIRM_LEN] = [0xff; CONFIRM_LEN];

/// One KM0 boot as `wellfed_amebad_trial_decide` runs it, KM0's own boot
/// count bumped first.
fn boot(
    register: u32,
    bank1: &[u8; 8],
    bank0: &[u8; 8],
    record: &[u8; CONFIRM_LEN],
    reset: Reset,
) -> (Decision, u32) {
    let bumped = (register & 0xffff_0000) | ((register + 1) & 0xffff);
    decide_after(bank1, bank0, record, bumped, &ID, reset)
}

/// Boot from `register` once per entry of `causes`, every boot ending in a
/// reset nobody asked for, stopping at a give-way.
fn boots(
    mut register: u32,
    bank1: &[u8; 8],
    bank0: &[u8; 8],
    record: &[u8; CONFIRM_LEN],
    causes: impl IntoIterator<Item = Reset>,
) -> (Vec<Decision>, u32) {
    let mut out = Vec::new();
    for reset in causes {
        let (decision, value) = boot(register, bank1, bank0, record, reset);
        out.push(decision);
        register = value;
        if matches!(decision, Decision::GiveWay | Decision::StormGiveWay { .. }) {
            break;
        }
    }
    (out, register)
}

/// The power-on, then `n` boots each reading the brownout bit set.
fn stuck(n: usize) -> impl Iterator<Item = Reset> {
    core::iter::once(Reset::Other).chain(core::iter::repeat_n(Reset::Brownout, n))
}

#[test]
fn a_brownout_bit_stuck_set_still_lets_a_fault_give_way_within_twice_the_limit() {
    let record = confirm_record(&ID, 0);
    let (out, _) = boots(
        0,
        &MARKER,
        &MARKER,
        &record,
        stuck(4 * STORM_LIMIT as usize),
    );

    // Every other boot is forgiven: the count rises one in two.
    let confirmed = |resets| Decision::Confirmed { resets };
    let mut expected = vec![confirmed(0), confirmed(0)];
    for resets in 1..STORM_LIMIT {
        expected.extend([confirmed(resets), confirmed(resets)]);
    }
    expected.push(Decision::StormGiveWay {
        resets: STORM_LIMIT,
    });
    assert_eq!(out, expected);
    assert_eq!(out.len(), 2 * STORM_LIMIT as usize + 1);
}

#[test]
fn a_brownout_bit_stuck_set_on_bank_0_still_holds_and_then_halts() {
    let mut register = 0;
    let mut held_at = None;
    let mut halted_at = None;
    for (n, reset) in stuck(4 * HALT_LIMIT as usize).enumerate() {
        let (decision, value) = boot(register, &CLEARED, &MARKER, &NONE, reset);
        register = value;
        if matches!(decision, Decision::Held { .. }) && held_at.is_none() {
            held_at = Some(n);
        }
        if Storm::of(resets(value, &ID)) == Storm::Halted {
            halted_at = Some(n);
            break;
        }
    }

    assert_eq!(held_at, Some(2 * STORM_LIMIT as usize));
    assert_eq!(halted_at, Some(2 * HALT_LIMIT as usize));
}

#[test]
fn a_brownout_between_faults_is_forgiven_but_never_two_in_a_row() {
    let record = confirm_record(&ID, 0);
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
    let (out, _) = boots(0, &MARKER, &MARKER, &record, causes);

    let confirmed = |resets| Decision::Confirmed { resets };
    assert_eq!(
        out,
        [
            confirmed(0),
            confirmed(1),
            confirmed(1),
            confirmed(2),
            confirmed(2),
            // The second brownout in a row is not forgiven.
            confirmed(3),
            confirmed(4),
            Decision::StormGiveWay {
                resets: STORM_LIMIT
            },
        ]
    );
}

#[test]
fn a_trial_counts_a_brownout_boot_and_still_gives_way() {
    let (out, _) = boots(0, &MARKER, &MARKER, &NONE, [Reset::Brownout; 4]);
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

#[test]
fn a_brownout_in_a_held_boot_keeps_it_held() {
    // Bank 0 held at five resets: KM0 stamped six for the held boot.
    let held = (0x5a << 24) | (6 << 20) | ((STORM_STATE | 1) << 16) | 30;
    let (decision, value) = boot(held, &CLEARED, &MARKER, &NONE, Reset::Brownout);
    assert_eq!(decision, Decision::Held { resets: 5 });
    assert_ne!(value & FORGIVEN, 0);

    let (decision, value) = boot(value, &CLEARED, &MARKER, &NONE, Reset::Brownout);
    assert_eq!(decision, Decision::Held { resets: 6 });
    assert_eq!(value & FORGIVEN, 0);
}

#[test]
fn a_brownout_in_a_halted_boot_keeps_it_halted_and_never_forgives_the_halts_own_count() {
    let halted = (0x5a << 24) | ((HALT_LIMIT + 1) << 20) | ((STORM_STATE | 2) << 16) | 30;
    let (decision, value) = boot(halted, &CLEARED, &MARKER, &NONE, Reset::Brownout);
    assert_eq!(decision, Decision::Held { resets: HALT_LIMIT });
    assert_eq!(Storm::of(resets(value, &ID)), Storm::Halted);

    // The halt's own reset leaves a held count, which a stale brownout
    // reading may not take back.
    let over = halt_over(value, &ID);
    let (decision, value) = boot(over, &CLEARED, &MARKER, &NONE, Reset::Brownout);
    assert_eq!(
        decision,
        Decision::Held {
            resets: STORM_LIMIT
        }
    );
    assert_eq!(Storm::of(resets(value, &ID)), Storm::Held);
}

#[test]
fn a_brownout_after_a_trial_with_no_fallback_ran_out_stays_held() {
    let (out, register) = boots(0, &MARKER, &CLEARED, &NONE, [Reset::Other; 4]);
    assert_eq!(out[3], Decision::NoFallback(3));

    let (decision, _) = boot(register, &MARKER, &CLEARED, &NONE, Reset::Brownout);
    assert_eq!(
        decision,
        Decision::Held {
            resets: STORM_LIMIT
        }
    );
}

#[test]
fn a_brownout_after_a_settled_or_deliberate_boot_has_nothing_to_forgive() {
    let pending = (0x5a << 24) | (3 << 20) | (STORM_STATE << 16) | 30;
    for register in [settled(pending, &ID), deliberate(pending)] {
        let (decision, value) = boot(register, &CLEARED, &MARKER, &NONE, Reset::Brownout);
        assert_eq!(decision, Decision::NotBank1 { resets: 0 }, "{register:#x}");
        assert_eq!(resets(value, &ID), 0);
        assert_eq!(value & FORGIVEN, 0, "nothing was forgiven");
    }
}

#[test]
fn the_mark_is_this_images_and_cleared_by_any_count() {
    // Another image's register carrying the mark: its count is not ours,
    // and nothing is forgiven or carried over.
    let other = FORGIVEN | (0x11 << 24) | (3 << 20) | (STORM_STATE << 16) | 30;
    let (decision, value) = boot(other, &CLEARED, &MARKER, &NONE, Reset::Brownout);
    assert_eq!(decision, Decision::NotBank1 { resets: 0 });
    assert_eq!(value & FORGIVEN, 0);

    // A fault after a forgiven boot clears the mark: the next brownout
    // may be forgiven again.
    let pending = (0x5a << 24) | (2 << 20) | (STORM_STATE << 16) | 30;
    let (_, forgave) = boot(pending, &CLEARED, &MARKER, &NONE, Reset::Brownout);
    let (_, fault) = boot(forgave, &CLEARED, &MARKER, &NONE, Reset::Other);
    assert_eq!(fault & FORGIVEN, 0);
    let (decision, _) = boot(fault, &CLEARED, &MARKER, &NONE, Reset::Brownout);
    assert_eq!(decision, Decision::NotBank1 { resets: 2 });
}
