//! When the loop steps the feeding owner: once the wait it asked for has
//! passed, however often the loop passes meanwhile.

use wellfed_fw_feeder::feeding::Cadence;

#[test]
fn the_owner_is_due_once_its_wait_has_passed_since_its_last_step() {
    let mut cadence = Cadence::default();
    assert_eq!(cadence.wait(0), None, "never stepped: due now");

    cadence.stepped(100, Some(20));
    // A busy loop passes every millisecond: none of them steps the owner.
    assert_eq!(cadence.wait(101), Some(19));
    assert_eq!(cadence.wait(119), Some(1));
    assert_eq!(cadence.wait(120), None);
    assert_eq!(cadence.wait(150), None, "late is still due");
}

#[test]
fn an_owner_that_asked_no_wait_is_stepped_on_the_next_pass() {
    let mut cadence = Cadence::default();
    cadence.stepped(100, Some(20));
    cadence.stepped(120, None);

    assert_eq!(cadence.wait(121), None);
}

#[test]
fn the_wait_holds_across_the_uptime_wrapping() {
    let mut cadence = Cadence::default();
    cadence.stepped(u32::MAX - 5, Some(20));

    assert_eq!(cadence.wait(u32::MAX), Some(15));
    assert_eq!(cadence.wait(13), Some(1));
    assert_eq!(cadence.wait(14), None);
}

#[test]
fn an_owner_that_asked_no_time_is_due_at_once() {
    let mut cadence = Cadence::default();
    cadence.stepped(100, Some(0));

    assert_eq!(cadence.wait(100), None);
}
