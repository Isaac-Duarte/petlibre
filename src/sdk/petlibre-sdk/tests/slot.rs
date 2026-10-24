//! `Slot` ownership on the SDK fake, where each host thread is one task.

use std::thread;

use petlibre_sdk::Slot;

#[test]
fn the_first_task_to_enter_owns_the_slot() {
    static SLOT: Slot<u32> = Slot::new();

    *SLOT.enter_or_init(|| 1).unwrap() += 1;
    assert_eq!(*SLOT.enter().unwrap(), 2);

    let other = thread::spawn(|| SLOT.enter().is_none() && SLOT.enter_or_init(|| 9).is_none());
    assert!(other.join().unwrap());
    assert_eq!(*SLOT.enter().unwrap(), 2);
}

#[test]
fn a_second_entry_is_refused_while_the_first_guard_lives() {
    static SLOT: Slot<u32> = Slot::filled(7);

    let guard = SLOT.enter().unwrap();
    assert!(SLOT.enter().is_none());
    assert!(!SLOT.release());

    drop(guard);
    assert_eq!(*SLOT.enter().unwrap(), 7);
}

#[test]
fn an_empty_slot_stays_empty_until_a_constructor_fills_it() {
    static SLOT: Slot<u32> = Slot::new();

    assert!(SLOT.enter().is_none());
    assert!(SLOT.enter_or_try_init(|| None).is_none());
    assert_eq!(*SLOT.enter_or_try_init(|| Some(3)).unwrap(), 3);
    assert_eq!(*SLOT.enter_or_init(|| 4).unwrap(), 3);
}

#[test]
fn entering_an_empty_slot_leaves_it_to_the_task_that_fills_it() {
    static SLOT: Slot<u32> = Slot::new();

    // The consumer runs first and finds nothing; the producer still fills.
    let early = thread::spawn(|| SLOT.enter().is_none());
    assert!(early.join().unwrap());
    assert_eq!(*SLOT.enter_or_init(|| 8).unwrap(), 8);
}

#[test]
fn release_hands_the_slot_to_the_next_task_only() {
    static SLOT: Slot<u32> = Slot::new();

    drop(SLOT.enter_or_init(|| 5));
    let stranger = thread::spawn(|| SLOT.release());
    assert!(!stranger.join().unwrap());
    assert!(SLOT.release());

    let next = thread::spawn(|| {
        let mut value = SLOT.enter().unwrap();
        *value += 1;
        *value
    });
    assert_eq!(next.join().unwrap(), 6);
    assert!(SLOT.enter().is_none());
}
