//! The loop's panel owner against the fake GPIO model: it starts locked,
//! follows Lock holds, remote changes and the idle relock (the lights that
//! show the lock are `lights.rs`'s). Pairing sees gestures first: the recovery chord opens its window only on
//! an unlocked panel, an unlocked Reset hold queues the key reset, and an
//! unlocked Lock and Mic hold starts BLE setup mode.

use std::sync::{Mutex, MutexGuard};

use wellfed_device_api::mailbox::Lock;
use wellfed_device_api::mailbox::SettingsView;
use wellfed_device_api::pairing;
use wellfed_feeder::feeder::owner::{Owner, POLL_MS, Record, ResetRefusal};
use wellfed_feeder::feeder::setup;
use wellfed_feeder::panel::{Action, Event, Source};
use wellfed_feeder_runtime::keys::{self, Change, KeyResult, Requested};
use wellfed_fw_feeder::platform::{Driver, Plaf103};
use wellfed_sdk::buttons::{Button, Buttons, Gesture};
use wellfed_sdk::hal::leds;
use wellfed_sdk::hal::sdk::{self, fake};
use wellfed_sdk::hal::{buttons, settings, stopped};

/// The button queue, the settings and the key mailbox are process-wide.
static SERIAL: Mutex<()> = Mutex::new(());

/// The panel's owner on the board's platform.
struct Rig {
    _serial: MutexGuard<'static, ()>,
    owner: Owner,
    platform: Plaf103,
    /// A feed is queued or runs.
    feeding: bool,
}

impl Rig {
    fn lock(&self) -> Option<Lock> {
        self.owner.lock()
    }

    fn reboot_due(&self, now_ms: u32) -> bool {
        self.owner.reboot_due(now_ms)
    }
}

fn ready() -> Rig {
    let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    fake::reset_hardware();
    buttons::reset();
    settings::reset();
    keys::reset();
    assert!(stopped::initialize().verified());
    assert!(leds::initialize());
    Rig {
        _serial: serial,
        owner: Owner::default(),
        platform: Plaf103::new(),
        feeding: false,
    }
}

/// A step at `now_ms` with `gestures` queued by the button service and the
/// `lights` setting, with whatever records it made.
fn step(
    rig: &mut Rig,
    now_ms: u32,
    gestures: &[Gesture],
    remote: Option<bool>,
    lights: bool,
) -> (u32, Vec<Record<Driver>>) {
    for &gesture in gestures {
        buttons::inject(gesture);
    }
    settings::publish(SettingsView {
        lights,
        sound: true,
        volume: 50,
        generation: 1,
        writable: true,
        ..Default::default()
    });
    let mut records = Vec::new();
    let delay = rig.owner.step(
        &mut rig.platform,
        now_ms,
        rig.feeding,
        remote,
        None,
        |record| {
            records.push(record);
        },
    );
    (delay, records)
}

/// Steps twice from `now_ms` with nothing queued, a poll apart.
fn settle(owner: &mut Rig, now_ms: u32, lights: bool) -> u32 {
    let (delay, _) = step(owner, now_ms, &[], None, lights);
    let now = now_ms + delay;
    let (delay, records) = step(owner, now, &[], None, lights);
    assert_eq!(records, []);
    assert_eq!(delay, POLL_MS);
    now
}

fn lock_hold() -> Gesture {
    Gesture::Hold {
        chord: Buttons::of(Button::Lock),
        level: 1,
    }
}

#[test]
fn starts_locked() {
    let mut owner = ready();

    let (delay, records) = step(&mut owner, 0, &[], None, true);
    assert_eq!(records, [Record::Start { lock: Lock::Locked }]);
    assert_eq!(delay, POLL_MS);
    assert_eq!(owner.lock(), Some(Lock::Locked));

    let (delay, records) = step(&mut owner, POLL_MS, &[], None, true);
    assert_eq!(records, []);
    assert_eq!(delay, POLL_MS);
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
    assert_eq!(owner.lock(), Some(Lock::Unlocked));

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
    assert_eq!(owner.lock(), Some(Lock::Locked));
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
fn records_read_as_uart_lines() {
    let line = |record: Record<Driver>| {
        let mut text = String::new();
        wellfed_feeder::feeder::owner::describe(record, &mut text).unwrap();
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
        line(Record::ResetRefused {
            reason: ResetRefusal::Feeding
        }),
        "reset refused=feeding"
    );
    assert_eq!(
        line(Record::ResetWritten {
            result: KeyResult::Done
        }),
        "reset result=done"
    );
}

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
fn unlocked() -> (Rig, u32) {
    let mut owner = ready();
    let now = settle(&mut owner, 0, true);
    step(&mut owner, now, &[lock_hold()], None, true);
    assert_eq!(owner.lock(), Some(Lock::Unlocked));
    (owner, now)
}

#[test]
fn lock_then_dispense_held_opens_recovery_without_a_feed() {
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
    assert!(!owner.reboot_due(now), "nothing marked yet");

    // The writer marked the reset: the loop restarts to finish it.
    keys::finish(KeyResult::Done);
    let (_, records) = step(&mut owner, now, &[], None, true);
    assert_eq!(
        records,
        [Record::ResetWritten {
            result: KeyResult::Done
        }]
    );
    assert!(owner.reboot_due(now));
}

#[test]
fn a_reset_hold_during_a_feed_is_refused_and_a_failed_one_does_not_restart() {
    let mut owner = ready();
    let now = settle(&mut owner, 0, true);
    step(&mut owner, now, &[lock_hold()], None, true);

    owner.feeding = true;
    let (_, records) = step(&mut owner, now, &[hold(&[Button::Reset])], None, true);
    assert_eq!(
        records,
        [
            Record::Panel(Event::Asked {
                action: Action::Reset,
                allowed: true,
            }),
            Record::ResetRefused {
                reason: ResetRefusal::Feeding
            },
        ]
    );
    assert_eq!(keys::pending(), None, "nothing queued for the writer");

    owner.feeding = false;
    step(&mut owner, now, &[hold(&[Button::Reset])], None, true);
    assert_eq!(keys::pending(), Some(Change::Reset));
    keys::finish(KeyResult::Io);
    let (_, records) = step(&mut owner, now, &[], None, true);
    assert_eq!(
        records,
        [Record::ResetWritten {
            result: KeyResult::Io
        }]
    );
    assert!(!owner.reboot_due(now), "an unmarked reset restarts nothing");
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
