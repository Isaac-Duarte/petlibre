//! The wrapping-clock helpers the ports use in place of literal half-range
//! checks: each must agree with the expression it replaced for every input.

use wellfed_core::{HALF_RANGE, deadline_passed, interval_valid, until};

const EDGES: [u32; 12] = [
    0,
    1,
    2,
    0x7fff_fffe,
    0x7fff_ffff,
    0x8000_0000,
    0x8000_0001,
    0x8000_0002,
    0xc000_0000,
    0xffff_fffe,
    0xffff_ffff,
    1000,
];

#[test]
fn interval_valid_matches_the_literal_checks() {
    for value in EDGES {
        let replaced = value == 0 || value >= 0x8000_0000;
        assert_eq!(!interval_valid(value), replaced, "{value:#x}");
        assert_eq!(value >= HALF_RANGE, value >= 0x8000_0000, "{value:#x}");
    }
}

#[test]
fn deadline_passed_matches_the_signed_difference() {
    for now in EDGES {
        for deadline in EDGES {
            for (now, deadline) in [(now, deadline), (now.wrapping_add(7), deadline)] {
                let signed = (now.wrapping_sub(deadline) as i32) >= 0;
                assert_eq!(
                    deadline_passed(now, deadline),
                    signed,
                    "{now:#x} {deadline:#x}"
                );
                assert_eq!(
                    !deadline_passed(now, deadline),
                    (now.wrapping_sub(deadline) as i32) < 0,
                    "{now:#x} {deadline:#x}"
                );
            }
        }
    }
}

#[test]
fn a_deadline_ahead_is_pending_until_reached() {
    let deadline = u32::MAX - 5;
    assert!(!deadline_passed(deadline - 1, deadline));
    assert!(deadline_passed(deadline, deadline));
    assert!(deadline_passed(deadline.wrapping_add(10), deadline));
}

#[test]
fn until_is_zero_once_a_deadline_passes_and_never_wraps() {
    for now in EDGES {
        for ahead in [1, 5, 1000, HALF_RANGE - 1] {
            assert_eq!(
                until(now, now.wrapping_add(ahead)),
                ahead,
                "{now:#x} +{ahead}"
            );
        }
        for behind in [0, 1, 5, 1000, HALF_RANGE - 1] {
            assert_eq!(
                until(now, now.wrapping_sub(behind)),
                0,
                "{now:#x} -{behind}"
            );
        }
    }
}
