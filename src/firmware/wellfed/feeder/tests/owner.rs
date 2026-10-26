//! The loop's panel owner against the fake GPIO model: it starts locked with
//! the lock indicator on, follows Lock holds, remote changes and the idle
//! relock, keeps the indicator off while the lights are off, and stops
//! driving the indicator for good when outputs stop reading back stopped.
//! Pairing sees gestures first: the recovery chord opens its window only on
//! an unlocked panel, an unlocked Reset hold queues the key reset, and an
//! unlocked Lock and Mic hold starts BLE setup mode.

use wellfed_feeder::pairing;
use wellfed_feeder::panel::{Action, Event, Lock, Source};
use wellfed_feeder_runtime::keys::{self, Change, KeyResult, Requested};
use wellfed_fw_feeder::owner::{Inputs, Owner, POLL_MS, Record};
use wellfed_fw_feeder::setup;
use wellfed_sdk::buttons::{Button, Buttons, Gesture};
use wellfed_sdk::hal::leds::{self, Led};
use wellfed_sdk::hal::plaf103;
use wellfed_sdk::hal::sdk::{self, fake};
use wellfed_sdk::hal::stopped;

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
            identity: None,
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
    fake::with(|h| h.output[0] &= !plaf103::pins::MOTOR_A_BIT);
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
        wellfed_fw_feeder::owner::describe(record, &mut text).unwrap();
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
        line(Record::Pairing(pairing::Event::Refused {
            path: pairing::Path::Recovery,
            refusal: pairing::Refusal::Locked,
        })),
        "pair refused path=recovery refusal=locked"
    );
    assert_eq!(
        line(Record::Pairing(pairing::Event::RecoveryOpened)),
        "pair recovery state=opened"
    );
    assert_eq!(
        line(Record::Reset {
            requested: Requested::Queued
        }),
        "reset keys=queued"
    );
    assert_eq!(
        line(Record::Indicator { decision: None }),
        "indicator led=PA4 stop=refused"
    );
}

/// Pairing and the key reset reach process-wide mailboxes.
static KEYS: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn down(buttons: &[Button]) -> Gesture {
    Gesture::Down {
        chord: chord(buttons),
    }
}

fn hold(buttons: &[Button]) -> Gesture {
    Gesture::Hold {
        chord: chord(buttons),
        level: 1,
    }
}

fn chord(buttons: &[Button]) -> Buttons {
    buttons
        .iter()
        .fold(Buttons::from_bits(0), |chord, &button| chord.with(button))
}

/// An owner stepped once, then unlocked by a Lock hold.
fn unlocked() -> (Owner, u32) {
    let mut owner = ready();
    let now = settle(&mut owner, 0, true);
    step(&mut owner, now, &[lock_hold()], None, true);
    assert_eq!(owner.lock(), Some(Lock::Unlocked));
    (owner, now)
}

#[test]
fn lock_then_dispense_held_opens_recovery_without_a_feed() {
    let _guard = KEYS.lock().unwrap_or_else(|e| e.into_inner());
    let (mut owner, now) = unlocked();

    let gestures = [
        down(&[Button::Lock]),
        down(&[Button::Lock, Button::Dispense]),
        hold(&[Button::Lock, Button::Dispense]),
    ];
    let (_, records) = step(&mut owner, now + 100, &gestures, None, true);
    assert_eq!(
        records,
        [Record::Pairing(pairing::Event::RecoveryOpened)],
        "no feed asked and no lock change"
    );
    assert_eq!(owner.lock(), Some(Lock::Unlocked));

    let closes = now + 100 + pairing::Config::DEFAULT.recovery_ms;
    let (_, records) = step(&mut owner, closes, &[], None, true);
    assert!(
        records.contains(&Record::Pairing(pairing::Event::RecoveryClosed)),
        "{records:?}"
    );
}

#[test]
fn a_locked_panel_refuses_recovery() {
    let _guard = KEYS.lock().unwrap_or_else(|e| e.into_inner());
    let mut owner = ready();
    let now = settle(&mut owner, 0, true);

    let (_, records) = step(
        &mut owner,
        now,
        &[hold(&[Button::Lock, Button::Dispense])],
        None,
        true,
    );
    assert_eq!(
        records,
        [Record::Pairing(pairing::Event::Refused {
            path: pairing::Path::Recovery,
            refusal: pairing::Refusal::Locked,
        })]
    );
}

#[test]
fn an_unlocked_reset_hold_queues_the_key_reset() {
    let _guard = KEYS.lock().unwrap_or_else(|e| e.into_inner());
    keys::reset();
    let mut owner = ready();
    let now = settle(&mut owner, 0, true);

    let (_, records) = step(&mut owner, now, &[hold(&[Button::Reset])], None, true);
    assert_eq!(
        records,
        [Record::Panel(Event::Asked {
            action: Action::Reset,
            allowed: false,
        })],
        "a locked panel keeps the keys"
    );
    assert_eq!(keys::pending(), None);

    step(&mut owner, now, &[lock_hold()], None, true);
    let (_, records) = step(&mut owner, now, &[hold(&[Button::Reset])], None, true);
    assert_eq!(
        records,
        [
            Record::Panel(Event::Asked {
                action: Action::Reset,
                allowed: true,
            }),
            Record::Reset {
                requested: Requested::Queued
            },
        ]
    );
    assert_eq!(keys::pending(), Some(Change::Reset));

    let (_, records) = step(&mut owner, now, &[hold(&[Button::Reset])], None, true);
    assert_eq!(
        records[1],
        Record::Reset {
            requested: Requested::Busy
        },
        "one reset waits for the writer at a time"
    );
    keys::finish(KeyResult::Done);
}

#[test]
fn lock_and_mic_held_start_setup_mode_only_when_unlocked() {
    let mut owner = ready();
    let now = settle(&mut owner, 0, true);
    let (_, records) = step(
        &mut owner,
        now,
        &[hold(&[Button::Lock, Button::Mic])],
        None,
        true,
    );
    assert_eq!(
        records,
        [Record::Pairing(pairing::Event::Refused {
            path: pairing::Path::Setup,
            refusal: pairing::Refusal::Locked,
        })]
    );

    step(&mut owner, now, &[lock_hold()], None, true);
    let (_, records) = step(
        &mut owner,
        now,
        &[hold(&[Button::Lock, Button::Mic])],
        None,
        true,
    );
    assert_eq!(
        records[..2],
        [
            Record::Pairing(pairing::Event::SetupOpened),
            Record::Setup(setup::Record::Start),
        ]
    );
    assert!(
        sdk::fake::with(|h| h.ble_started.is_some()),
        "the stack starts"
    );
    assert!(!owner.reboot_due(now));
}
