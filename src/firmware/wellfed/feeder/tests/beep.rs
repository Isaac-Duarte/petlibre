//! The panel's beep on the board's platform, against the fake GPIO model:
//! a press drives PA25 HIGH for 200 ms when sound may play, and a muted
//! feeder (sound off, volume 0) stays silent.

use std::sync::{Mutex, MutexGuard};

use wellfed_core::clock::wall::{Origin, Sync};
use wellfed_core::settings::Window;
use wellfed_device_api::mailbox::SettingsView;
use wellfed_feeder::feeder::beep::{BEEP_MS, Beeper};
use wellfed_feeder::feeder::{Feeder, Feeds, Record as PassRecord};
use wellfed_fw_feeder::platform::{Driver, Plaf103};
use wellfed_proto::api::{Blocked, Refusal, Source};
use wellfed_sdk::buttons::{Button, Buttons, Gesture};
use wellfed_sdk::hal::sdk::fake;
use wellfed_sdk::hal::{buttons, buzzer, clock, leds, settings, stopped};

/// The settings and the button queue are process-wide.
static SERIAL: Mutex<()> = Mutex::new(());

fn start() -> MutexGuard<'static, ()> {
    let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    fake::reset_hardware();
    settings::reset();
    clock::reset();
    buttons::reset();
    assert!(stopped::initialize().verified());
    assert!(leds::initialize());
    serial
}

fn publish(sound: bool, volume: u8) {
    settings::publish(SettingsView {
        lights: true,
        sound,
        volume,
        generation: 1,
        writable: true,
        ..Default::default()
    });
}

fn beeping() -> bool {
    let level = buzzer::level();
    level.output && level.latch_high
}

#[test]
fn a_beep_lasts_200_ms_and_a_press_during_it_restarts_it() {
    let _serial = start();
    let mut platform = Plaf103::new();
    let mut beep = Beeper::new();

    assert_eq!(beep.step(&mut platform, 0, false, true, true), u32::MAX);
    assert!(!beeping(), "nothing drives PA25 before a press");

    assert_eq!(beep.step(&mut platform, 0, true, true, true), BEEP_MS);
    assert!(beeping());
    assert_eq!(beep.step(&mut platform, 150, true, true, true), BEEP_MS);
    assert_eq!(beep.step(&mut platform, 300, false, true, true), 50);
    assert!(beeping());

    assert_eq!(beep.step(&mut platform, 350, false, true, true), u32::MAX);
    assert!(!beeping());
}

#[test]
fn a_press_while_sound_may_not_play_is_silent() {
    let _serial = start();
    let mut platform = Plaf103::new();
    let mut beep = Beeper::new();

    assert_eq!(beep.step(&mut platform, 0, true, false, true), u32::MAX);
    assert!(!beeping());
    assert!(!beep.sounding());
}

/// A feeder that is never asked to feed: the beep test presses Lock.
struct Idle;

impl Feeds for Idle {
    fn feeding(&self) -> bool {
        false
    }

    fn busy(&self) -> bool {
        false
    }

    fn blocked(&self) -> Option<Blocked> {
        None
    }

    fn submit(&mut self, _: u8, _: Source) -> Result<u32, Refusal> {
        Err(Refusal::Fault)
    }

    fn reserved(&self, _: u32) -> Option<Result<(), Refusal>> {
        None
    }

    fn cancel(&self, _: u32) -> bool {
        false
    }
}

/// Press Lock once in the loop at 0 ms, with sound `sound` at `volume`;
/// whether PA25 is HIGH at 100 ms and at 300 ms.
fn press_in_the_loop(sound: bool, volume: u8) -> (bool, bool) {
    let _serial = start();
    publish(sound, volume);
    let mut feeder = Box::new(Feeder::new(Plaf103::new()));
    buttons::inject(Gesture::Down {
        chord: Buttons::of(Button::Lock),
    });

    let mut at = [false; 2];
    for now in (0..=300).step_by(10) {
        fake::with(|h| h.uptime_ms = now);
        feeder.pass(now, &mut Idle, |_: PassRecord<Driver>| {});
        match now {
            100 => at[0] = beeping(),
            300 => at[1] = beeping(),
            _ => {}
        }
    }
    (at[0], at[1])
}

#[test]
fn every_press_beeps_unless_sound_is_muted() {
    assert_eq!(press_in_the_loop(true, 50), (true, false));
    assert_eq!(press_in_the_loop(false, 50), (false, false));
    assert_eq!(press_in_the_loop(true, 0), (false, false));
}

#[test]
fn the_line_is_enabled_only_at_rest_and_a_press_before_is_silent() {
    let _serial = start();
    let mut platform = Plaf103::new();
    let mut beep = Beeper::new();

    // A feed runs from boot: PA25 is left an input, untouched.
    assert_eq!(beep.step(&mut platform, 0, true, true, false), u32::MAX);
    assert!(!buzzer::level().output);

    assert_eq!(beep.step(&mut platform, 10, false, true, true), u32::MAX);
    assert!(buzzer::level().output && !beeping(), "enabled LOW at rest");
    assert_eq!(beep.step(&mut platform, 20, true, true, false), BEEP_MS);
    assert!(beeping(), "once enabled, a press during a feed beeps");
}

#[test]
fn a_press_outside_the_sound_window_is_silent() {
    let _serial = start();
    settings::publish(SettingsView {
        lights: true,
        sound: true,
        volume: 50,
        // 08:00-20:00 local.
        sound_window: Window::Between {
            start: 8 * 60,
            end: 20 * 60,
        },
        generation: 1,
        writable: true,
        ..Default::default()
    });
    // 23:00 local at UTC-6.
    clock::sync(
        Sync {
            unix_ms: 1_793_250_000_000,
            offset_s: -6 * 3600,
            next_change_s: 0,
            next_offset_s: -6 * 3600,
        },
        Origin::Owner,
    )
    .unwrap();
    let mut feeder = Box::new(Feeder::new(Plaf103::new()));
    buttons::inject(Gesture::Down {
        chord: Buttons::of(Button::Lock),
    });

    let mut beeped = false;
    for now in (0..=300).step_by(10) {
        fake::with(|h| h.uptime_ms = now);
        feeder.pass(now, &mut Idle, |_: PassRecord<Driver>| {});
        beeped |= beeping();
    }
    assert!(!beeped);
}

#[test]
fn a_press_before_the_settings_load_is_silent() {
    let _serial = start();
    let mut feeder = Box::new(Feeder::new(Plaf103::new()));
    buttons::inject(Gesture::Down {
        chord: Buttons::of(Button::Lock),
    });

    let mut beeped = false;
    for now in (0..=300).step_by(10) {
        fake::with(|h| h.uptime_ms = now);
        feeder.pass(now, &mut Idle, |_: PassRecord<Driver>| {});
        beeped |= beeping();
    }
    assert!(!beeped);
}
