//! The loop's panel owner against the fake GPIO model: it starts locked with
//! the lock indicator on, follows Lock holds, remote changes and the idle
//! relock, keeps the indicator off while the lights are off, and stops
//! driving the indicator for good when outputs stop reading back stopped.

use petlibre_feeder::panel::{Action, Event, Lock, Source};
use petlibre_fw_feeder::owner::{Inputs, Owner, POLL_MS, Record};
use petlibre_sdk::buttons::{Button, Buttons, Gesture};
use petlibre_sdk::hal::leds::{self, Led};
use petlibre_sdk::hal::sdk::{self, fake};
use petlibre_sdk::hal::stopped;

fn ready() -> Owner {
    fake::reset_hardware();
    assert!(stopped::initialize().verified());
    assert!(leds::initialize());
    Owner::default()
}

/// A step at `now_ms`, with whatever records it made.
fn step(
    owner: &mut Owner,
    now_ms: u32,
    gestures: &[Gesture],
    remote: Option<bool>,
    lights: bool,
) -> (u32, Vec<Record>) {
    let mut records = Vec::new();
    let delay = owner.step(
        Inputs {
            now_ms,
            gestures: gestures.iter().copied(),
            remote,
            lights,
        },
        |record| records.push(record),
    );
    (delay, records)
}

/// Steps from `now_ms` until the indicator settles, so it reads back.
fn settle(owner: &mut Owner, now_ms: u32, lights: bool) -> u32 {
    let (delay, _) = step(owner, now_ms, &[], None, lights);
    let now = now_ms + delay;
    let (delay, records) = step(owner, now, &[], None, lights);
    assert_eq!(records, []);
    assert_eq!(delay, POLL_MS);
    now
}

fn lit() -> bool {
    leds::observe(Led::Lock).latch_high
}

fn lock_hold() -> Gesture {
    Gesture::Hold {
        chord: Buttons::of(Button::Lock),
        level: 1,
    }
}

#[test]
fn starts_locked_with_the_indicator_on() {
    let mut owner = ready();

    let (delay, records) = step(&mut owner, 0, &[], None, true);
    assert_eq!(records, [Record::Start { lock: Lock::Locked }]);
    assert_eq!(delay, leds::SETTLE_MS, "the level settles before readback");
    assert_eq!(owner.lock(), Some(Lock::Locked));

    let (delay, records) = step(&mut owner, leds::SETTLE_MS, &[], None, true);
    assert_eq!(records, []);
    assert_eq!(delay, POLL_MS);
    assert!(lit());
}

#[test]
fn a_lock_hold_unlocks_and_the_idle_timer_relocks() {
    let mut owner = ready();
    let now = settle(&mut owner, 0, true);

    let dispense = Gesture::Down {
        chord: Buttons::of(Button::Dispense),
    };
    let (_, records) = step(&mut owner, now + 100, &[dispense], None, true);
    assert_eq!(
        records,
        [Record::Panel(Event::Asked {
            action: Action::Feed,
            allowed: false
        })]
    );

    let dispensed = Gesture::Up {
        chord: Buttons::of(Button::Dispense),
        level: 0,
        held_ms: 100,
    };
    let lock_down = Gesture::Down {
        chord: Buttons::of(Button::Lock),
    };
    step(&mut owner, now + 200, &[dispensed, lock_down], None, true);
    let (_, records) = step(&mut owner, now + 5_200, &[lock_hold()], None, true);
    assert_eq!(
        records,
        [Record::Panel(Event::Changed {
            lock: Lock::Unlocked,
            source: Source::Button
        })]
    );
    let now = settle(&mut owner, now + 5_200, true);
    assert!(!lit());

    // The held Lock button holds off the relock until its release.
    let release = Gesture::Up {
        chord: Buttons::of(Button::Lock),
        level: 1,
        held_ms: 5_400,
    };
    step(&mut owner, now + 200, &[release], None, true);
    let (_, records) = step(&mut owner, now + 200 + 30_000, &[], None, true);
    assert_eq!(
        records,
        [Record::Panel(Event::Changed {
            lock: Lock::Locked,
            source: Source::Idle
        })]
    );
    settle(&mut owner, now + 200 + 30_000, true);
    assert!(lit());
}

#[test]
fn a_remote_change_applies_and_a_repeat_is_silent() {
    let mut owner = ready();
    let now = settle(&mut owner, 0, true);

    let (_, records) = step(&mut owner, now + 100, &[], Some(false), true);
    assert_eq!(
        records,
        [Record::Panel(Event::Changed {
            lock: Lock::Unlocked,
            source: Source::Remote
        })]
    );
    assert_eq!(owner.lock(), Some(Lock::Unlocked));

    let now = settle(&mut owner, now + 100, true);
    let (_, records) = step(&mut owner, now + 100, &[], Some(false), true);
    assert_eq!(records, []);
}

#[test]
fn the_indicator_stays_off_while_the_lights_are_off() {
    let mut owner = ready();
    settle(&mut owner, 0, false);
    assert_eq!(owner.lock(), Some(Lock::Locked));
    assert!(!lit());

    settle(&mut owner, 100, true);
    assert!(lit());
}

#[test]
fn a_refused_level_turns_the_indicator_off_for_the_boot() {
    let mut owner = ready();
    let now = settle(&mut owner, 0, true);

    // Motor STOP lost: the next level is refused, and nothing retries.
    fake::with(|h| h.output[0] &= !sdk::MOTOR_A_BIT);
    let (_, records) = step(&mut owner, now + 100, &[], Some(false), true);
    assert_eq!(
        records,
        [
            Record::Panel(Event::Changed {
                lock: Lock::Unlocked,
                source: Source::Remote
            }),
            Record::Indicator { decision: None },
        ]
    );
    assert!(!lit());

    // The lock still works; the indicator is not driven again.
    let (delay, records) = step(&mut owner, now + 200, &[lock_hold()], None, true);
    assert_eq!(
        records,
        [Record::Panel(Event::Changed {
            lock: Lock::Locked,
            source: Source::Button
        })]
    );
    assert_eq!(delay, POLL_MS);
    assert!(!lit());
}

#[test]
fn records_read_as_uart_lines() {
    let line = |record| {
        let mut text = String::new();
        petlibre_fw_feeder::owner::describe(record, &mut text).unwrap();
        text
    };
    assert_eq!(
        line(Record::Start { lock: Lock::Locked }),
        "start lock=locked"
    );
    assert_eq!(
        line(Record::Panel(Event::Changed {
            lock: Lock::Unlocked,
            source: Source::Remote
        })),
        "lock lock=unlocked source=remote"
    );
    assert_eq!(
        line(Record::Panel(Event::Asked {
            action: Action::Reset,
            allowed: false
        })),
        "ask action=reset result=refused"
    );
    assert_eq!(
        line(Record::Indicator { decision: None }),
        "indicator led=PA4 stop=refused"
    );
}
