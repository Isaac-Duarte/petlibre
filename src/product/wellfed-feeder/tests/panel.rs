//! The lock rules against synthetic gestures: the Lock hold toggles, a
//! locked panel refuses Dispense, Mic and the Reset hold, and an idle unlocked
//! panel relocks on stock's timer, counted from the last release.

use wellfed_core::buttons::{Button, Buttons, Gesture};
use wellfed_feeder::panel::{Action, BUTTONS, Config, Event, Lock, Panel, Source};

const RELOCK: u32 = Config::STOCK.relock_ms;

fn chord(button: Button) -> Buttons {
    Buttons::of(button)
}

fn down(button: Button) -> Gesture {
    Gesture::Down {
        chord: chord(button),
    }
}

fn hold(button: Button) -> Gesture {
    Gesture::Hold {
        chord: chord(button),
        level: 1,
    }
}

fn up(button: Button, level: u8) -> Gesture {
    Gesture::Up {
        chord: chord(button),
        level,
        held_ms: 100,
    }
}

fn asked(action: Action, allowed: bool) -> Option<Event> {
    Some(Event::Asked { action, allowed })
}

fn changed(lock: Lock, source: Source) -> Option<Event> {
    Some(Event::Changed { lock, source })
}

/// A full Lock hold from `at`: down, the hold at 5 s, then the release.
fn hold_lock(panel: &mut Panel, at: u32) -> Option<Event> {
    assert_eq!(panel.gesture(at, down(Button::Lock)), None);
    let event = panel.gesture(at + 5_000, hold(Button::Lock));
    assert_eq!(panel.gesture(at + 5_200, up(Button::Lock, 1)), None);
    event
}

#[test]
fn the_configuration_is_stocks_and_valid() {
    assert!(Config::STOCK.valid());
    assert!(BUTTONS.valid());
    assert_eq!(BUTTONS.holds_ms[0], 5_000);
    assert!(
        !Config {
            hold_level: 0,
            ..Config::STOCK
        }
        .valid()
    );
}

#[test]
fn a_locked_panel_refuses_every_action_but_the_lock_hold() {
    let mut panel = Panel::new(Config::STOCK, 0);
    assert_eq!(panel.lock(), Lock::Locked, "boots locked");

    assert_eq!(
        panel.gesture(10, down(Button::Dispense)),
        asked(Action::Feed, false)
    );
    assert_eq!(panel.gesture(20, up(Button::Dispense, 0)), None);
    assert_eq!(
        panel.gesture(30, down(Button::Mic)),
        asked(Action::Mic, false)
    );
    assert_eq!(panel.gesture(40, up(Button::Mic, 0)), None);
    assert_eq!(panel.gesture(50, down(Button::Reset)), None);
    assert_eq!(
        panel.gesture(5_050, hold(Button::Reset)),
        asked(Action::Reset, false)
    );
    assert_eq!(panel.gesture(5_100, up(Button::Reset, 1)), None);

    assert_eq!(
        hold_lock(&mut panel, 6_000),
        changed(Lock::Unlocked, Source::Button)
    );
    assert_eq!(
        panel.gesture(11_300, down(Button::Dispense)),
        asked(Action::Feed, true)
    );
    assert_eq!(panel.gesture(11_400, up(Button::Dispense, 0)), None);
}

#[test]
fn the_lock_hold_toggles_once_per_hold() {
    let mut panel = Panel::new(Config::STOCK, 0);
    assert_eq!(
        hold_lock(&mut panel, 0),
        changed(Lock::Unlocked, Source::Button)
    );
    assert_eq!(
        hold_lock(&mut panel, 6_000),
        changed(Lock::Locked, Source::Button)
    );

    // A short press, a level the panel does not use, and a chord do nothing.
    assert_eq!(panel.gesture(12_000, down(Button::Lock)), None);
    assert_eq!(panel.gesture(12_100, up(Button::Lock, 0)), None);
    let both = chord(Button::Lock).with(Button::Dispense);
    assert_eq!(panel.gesture(13_000, Gesture::Down { chord: both }), None);
    assert_eq!(
        panel.gesture(
            18_000,
            Gesture::Hold {
                chord: both,
                level: 1
            }
        ),
        None
    );
    assert_eq!(
        panel.gesture(
            18_100,
            Gesture::Hold {
                chord: chord(Button::Lock),
                level: 2
            }
        ),
        None
    );
    assert_eq!(panel.lock(), Lock::Locked);
}

#[test]
fn an_idle_unlocked_panel_relocks_after_thirty_seconds() {
    let mut panel = Panel::new(Config::STOCK, 0);
    assert_eq!(panel.tick(RELOCK * 2), None, "already locked");

    hold_lock(&mut panel, 1_000);
    let released = 6_200;
    assert_eq!(panel.tick(released + RELOCK - 1), None);
    assert_eq!(
        panel.tick(released + RELOCK),
        changed(Lock::Locked, Source::Idle)
    );
    assert_eq!(panel.tick(released + RELOCK + 1), None);
}

#[test]
fn activity_restarts_the_relock_and_a_held_button_holds_it_off() {
    let mut panel = Panel::new(Config::STOCK, 0);
    hold_lock(&mut panel, 0);

    // A Mic press at 20 s restarts the 30 s.
    panel.gesture(20_000, down(Button::Mic));
    panel.gesture(20_100, up(Button::Mic, 0));
    assert_eq!(panel.tick(50_099), None);

    // Dispense held past the timer: no relock until it is let go.
    panel.gesture(50_000, down(Button::Dispense));
    assert_eq!(panel.tick(90_000), None);
    panel.gesture(90_000, up(Button::Dispense, 1));
    assert_eq!(panel.tick(119_999), None);
    assert_eq!(panel.tick(120_000), changed(Lock::Locked, Source::Idle));
}

#[test]
fn presses_pairing_took_hold_off_the_relock() {
    let mut panel = Panel::new(Config::STOCK, 0);
    hold_lock(&mut panel, 0);

    // A code keyed in from 20 s: its presses act on nothing, but count.
    panel.noted(20_000, down(Button::Reset));
    assert_eq!(panel.tick(40_000), None, "held");
    panel.noted(40_000, up(Button::Reset, 0));
    assert_eq!(panel.tick(40_000 + RELOCK - 1), None);
    assert_eq!(
        panel.tick(40_000 + RELOCK),
        changed(Lock::Locked, Source::Idle)
    );
}

#[test]
fn a_remote_unlock_relocks_on_the_same_timer() {
    let mut panel = Panel::new(Config::STOCK, 0);
    assert_eq!(panel.remote(1_000, Lock::Locked), None, "already locked");
    assert_eq!(
        panel.remote(2_000, Lock::Unlocked),
        changed(Lock::Unlocked, Source::Remote)
    );
    assert_eq!(panel.tick(2_000 + RELOCK - 1), None);
    assert_eq!(
        panel.tick(2_000 + RELOCK),
        changed(Lock::Locked, Source::Idle)
    );

    panel.remote(40_000, Lock::Unlocked);
    assert_eq!(
        panel.remote(41_000, Lock::Locked),
        changed(Lock::Locked, Source::Remote)
    );
}

#[test]
fn the_relock_survives_the_millisecond_clock_wrapping() {
    let start = u32::MAX - 10_000;
    let mut panel = Panel::new(Config::STOCK, start);
    panel.remote(start, Lock::Unlocked);
    assert_eq!(panel.tick(start.wrapping_add(RELOCK - 1)), None);
    assert_eq!(
        panel.tick(start.wrapping_add(RELOCK)),
        changed(Lock::Locked, Source::Idle)
    );
}
