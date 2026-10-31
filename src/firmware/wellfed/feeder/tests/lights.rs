//! The panel's lights on the board's platform, against the fake GPIO model:
//! each feeder state shows on its pad as the provisional mapping says, the
//! higher state hides the lower and its end reveals it, the lights setting
//! darkens them all, a feed's running motor neither stops the lights nor is
//! touched by them, and a light that fails turns them all off for the boot.

use std::sync::{Mutex, MutexGuard};

use wellfed_core::clock::wall::{Origin, Sync};
use wellfed_core::indicator::{Check, Failure, Light};
use wellfed_core::settings::Window;
use wellfed_device_api::mailbox::SettingsView;
use wellfed_feeder::feeder::lights::{Lights, REFUSED_MS, Record, State, States, describe};
use wellfed_feeder::feeder::{Feeder, Feeds, Record as PassRecord};
use wellfed_feeder_runtime::events;
use wellfed_fw_feeder::platform::{Driver, Plaf103};
use wellfed_proto::api::{Blocked, EventBody, FoodLevel, Refusal, Source};
use wellfed_sdk::hal::leds::{self, Led};
use wellfed_sdk::hal::plaf103::pins::{IR_PA_BIT, MOTOR_A_BIT};
use wellfed_sdk::hal::sdk::fake;
use wellfed_sdk::hal::{buttons, clock, settings, stopped};

/// The settings and the button queue are process-wide.
static SERIAL: Mutex<()> = Mutex::new(());

struct Rig {
    _serial: MutexGuard<'static, ()>,
    lights: Lights,
    platform: Plaf103,
    now: u32,
    records: Vec<Record>,
}

impl Rig {
    fn new() -> Self {
        let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        fake::reset_hardware();
        settings::reset();
        clock::reset();
        buttons::reset();
        assert!(stopped::initialize().verified());
        assert!(leds::initialize());
        Self {
            _serial: serial,
            lights: Lights::new(),
            platform: Plaf103::new(),
            now: 0,
            records: Vec::new(),
        }
    }

    /// Step at the current time; the delay asked for.
    fn step(&mut self, states: States) -> u32 {
        let records = &mut self.records;
        self.lights
            .step(&mut self.platform, self.now, states, |r| records.push(r))
    }

    /// Step through `ms` as a loop would: at each asked delay, at most a
    /// poll apart. Each light's level after every step.
    fn run(&mut self, states: States, ms: u32) -> Vec<(u32, [bool; 4])> {
        let end = self.now + ms;
        let mut seen = Vec::new();
        while self.now < end {
            let delay = self.step(states);
            seen.push((self.now, lit()));
            self.now += delay.min(20);
        }
        seen
    }

    fn take(&mut self) -> Vec<Record> {
        std::mem::take(&mut self.records)
    }
}

/// Each light's latch, in `Light` order: connection, food, lock, fault.
fn lit() -> [bool; 4] {
    [Led::Connection, Led::Food, Led::Lock, Led::Fault].map(|led| leds::observe(led).latch_high)
}

fn on(light: Light) -> bool {
    lit()[light.index()]
}

const IDLE: States = States {
    fault: false,
    updating: false,
    setup: false,
    pairing: false,
    battery: false,
    feeding: false,
    low_food: false,
    locked: false,
    online: false,
    lights: true,
};

fn shows(light: Light, state: Option<State>) -> Record {
    Record::Shows { light, state }
}

#[test]
fn a_locked_panel_lights_the_lock_steady_and_the_setting_darkens_it() {
    let mut rig = Rig::new();
    let locked = States {
        locked: true,
        ..IDLE
    };

    assert_eq!(rig.step(locked), leds::SETTLE_MS, "the level settles first");
    assert_eq!(rig.take(), [shows(Light::Lock, Some(State::Locked))]);
    assert_eq!(lit(), [false, false, true, false]);
    rig.now += leds::SETTLE_MS;
    assert_eq!(
        rig.step(locked),
        u32::MAX,
        "a steady light asks for nothing"
    );

    let dark = States {
        lights: false,
        ..locked
    };
    rig.run(dark, 100);
    assert_eq!(lit(), [false; 4]);
    assert_eq!(rig.take(), [], "the request still holds under the setting");

    rig.run(locked, 100);
    assert!(on(Light::Lock));
}

#[test]
fn a_feed_blinks_the_food_light_while_the_motor_runs() {
    let mut rig = Rig::new();
    let feeding = States {
        feeding: true,
        ..IDLE
    };

    // The motor driven and the outlet emitter lit, as in a feed.
    fake::with(|h| h.output[0] = (h.output[0] & !MOTOR_A_BIT) | IR_PA_BIT);
    let outputs = fake::with(|h| h.output[0] & stopped::PA_MASK);

    let seen = rig.run(feeding, 2_000);
    let food: Vec<bool> = seen.iter().map(|(_, l)| l[Light::Food.index()]).collect();
    let edges = food.windows(2).filter(|w| w[0] != w[1]).count();
    assert_eq!(edges, 3, "500/500 ms over 2 s");
    for (now, levels) in &seen {
        let phase = now % 1_000;
        // Within a settle of each edge the latch already follows it.
        if phase % 500 >= leds::SETTLE_MS {
            assert_eq!(levels[Light::Food.index()], phase < 500, "at {now}");
        }
    }
    assert_eq!(rig.take(), [shows(Light::Food, Some(State::Feeding))]);
    fake::with(|h| assert_eq!(h.output[0] & stopped::PA_MASK, outputs));

    rig.run(IDLE, 100);
    assert!(!on(Light::Food));
    assert_eq!(rig.take(), [shows(Light::Food, None)]);
}

#[test]
fn a_fault_hides_the_battery_blink_and_its_end_reveals_it() {
    let mut rig = Rig::new();
    let battery = States {
        battery: true,
        ..IDLE
    };
    let both = States {
        fault: true,
        ..battery
    };

    rig.run(battery, 100);
    assert!(on(Light::Fault), "the 1 s blink starts lit");
    rig.run(both, 2_500);
    assert!(on(Light::Fault), "steady through the battery's dark phases");
    rig.run(battery, 1_500);

    assert_eq!(
        rig.take(),
        [
            shows(Light::Fault, Some(State::Battery)),
            shows(Light::Fault, Some(State::Fault)),
            shows(Light::Fault, Some(State::Battery)),
        ]
    );
}

#[test]
fn a_pending_offer_blinks_over_the_lock_and_setup_over_the_address() {
    let mut rig = Rig::new();
    let base = States {
        locked: true,
        online: true,
        ..IDLE
    };
    rig.run(base, 100);
    assert_eq!(lit(), [true, false, true, false]);

    let offer = States {
        pairing: true,
        setup: true,
        ..base
    };
    let seen = rig.run(offer, 1_000);
    let toggles = |light: Light| {
        seen.windows(2)
            .filter(|w| w[0].1[light.index()] != w[1].1[light.index()])
            .count()
    };
    assert!(toggles(Light::Lock) >= 3, "250/250 ms");
    assert!(toggles(Light::Connection) >= 1, "500/500 ms");

    rig.run(base, 100);
    assert_eq!(lit(), [true, false, true, false]);
    assert_eq!(
        rig.take(),
        [
            shows(Light::Connection, Some(State::Online)),
            shows(Light::Lock, Some(State::Locked)),
            shows(Light::Connection, Some(State::Setup)),
            shows(Light::Lock, Some(State::Pairing)),
            shows(Light::Connection, Some(State::Online)),
            shows(Light::Lock, Some(State::Locked)),
        ]
    );
}

#[test]
fn a_refused_press_flashes_the_fault_light_for_three_seconds() {
    let mut rig = Rig::new();
    rig.run(IDLE, 100);

    let records = &mut rig.records;
    rig.lights.refused(rig.now, |r| records.push(r));
    let start = rig.now;
    let seen = rig.run(IDLE, REFUSED_MS + 200);
    let lit_at: Vec<u32> = seen
        .iter()
        .filter(|(_, l)| l[Light::Fault.index()])
        .map(|(now, _)| now - start)
        .collect();
    assert!(lit_at.first() == Some(&0));
    assert!(lit_at.iter().all(|&t| t < REFUSED_MS));
    assert!(!on(Light::Fault));
    assert_eq!(
        rig.take(),
        [
            shows(Light::Fault, Some(State::Refused)),
            shows(Light::Fault, None)
        ]
    );
}

#[test]
fn a_failed_light_turns_them_all_off_for_the_boot() {
    let mut rig = Rig::new();
    let shown = States {
        locked: true,
        online: true,
        ..IDLE
    };
    rig.run(shown, 100);
    rig.take();

    // PB22 is no longer an output: the next write is refused.
    fake::with(|h| h.direction[1] &= !leds::PB_MASK);
    let fault = States {
        fault: true,
        ..shown
    };
    assert_eq!(rig.step(fault), u32::MAX);
    assert_eq!(
        rig.take(),
        [
            shows(Light::Fault, Some(State::Fault)),
            Record::Failed(Failure {
                light: Light::Fault,
                check: None
            }),
        ]
    );
    assert_eq!(lit(), [false; 4]);

    // Nothing is driven again.
    fake::with(|h| h.direction[1] |= leds::PB_MASK);
    rig.run(IDLE, 200);
    rig.run(shown, 200);
    assert_eq!(lit(), [false; 4]);
    assert!(rig.take().iter().all(|r| matches!(r, Record::Shows { .. })));
}

#[test]
fn records_read_as_uart_lines() {
    let line = |record| {
        let mut text = String::new();
        describe(record, &mut text).unwrap();
        text
    };
    assert_eq!(
        line(shows(Light::Connection, Some(State::Setup))),
        "show light=connection state=setup"
    );
    assert_eq!(line(shows(Light::Food, None)), "show light=food state=none");
    assert_eq!(
        line(Record::Failed(Failure {
            light: Light::Lock,
            check: Some(Check::Abort)
        })),
        "fail light=lock check=abort"
    );
}

/// A feeding owner held on a fault.
struct Faulted;

impl Feeds for Faulted {
    fn feeding(&self) -> bool {
        false
    }

    fn busy(&self) -> bool {
        false
    }

    fn blocked(&self) -> Option<Blocked> {
        Some(Blocked::Fault)
    }

    fn submit(&mut self, _: u8, _: Source) -> Result<u32, Refusal> {
        Err(Refusal::Fault)
    }

    fn reserved(&self, _: u32) -> Option<Result<(), Refusal>> {
        Some(Err(Refusal::Fault))
    }

    fn cancel(&self, _: u32) -> bool {
        false
    }
}

#[test]
fn the_loop_shows_the_lock_and_a_held_fault() {
    let rig = Rig::new();
    settings::publish(SettingsView {
        lights: true,
        sound: false,
        volume: 50,
        generation: 1,
        writable: true,
        ..Default::default()
    });
    let mut feeder = Box::new(Feeder::new(Plaf103::new()));
    let mut lines = Vec::new();

    for now in (0..200).step_by(10) {
        fake::with(|h| h.uptime_ms = now);
        feeder.pass(now, &mut Faulted, |r: PassRecord<Driver>| {
            lines.push(format!("{} {r}", r.tag()));
        });
    }

    assert!(on(Light::Lock) && on(Light::Fault));
    assert!(lines.contains(&"PLIND1 show light=lock state=locked".to_owned()));
    assert!(lines.contains(&"PLIND1 show light=fault state=fault".to_owned()));
    drop(rig);
}

/// An owner at rest with nothing to feed, whose food beam reads `food`.
struct AtRest {
    food: Option<bool>,
}

impl Feeds for AtRest {
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
        Err(Refusal::Busy)
    }

    fn reserved(&self, _: u32) -> Option<Result<(), Refusal>> {
        Some(Err(Refusal::Busy))
    }

    fn cancel(&self, _: u32) -> bool {
        false
    }

    fn food(&self) -> Option<bool> {
        self.food
    }
}

/// With no feed in the boot, a reading at rest lights low food and one of
/// food clears it, each change one `Food` event however often it reads.
#[test]
fn a_reading_at_rest_lights_low_food_and_a_refill_clears_it() {
    let rig = Rig::new();
    settings::publish(SettingsView {
        lights: true,
        ..Default::default()
    });
    events::reset();
    let mut feeder = Box::new(Feeder::new(Plaf103::new()));
    let mut owner = AtRest { food: None };
    let run = |feeder: &mut Feeder<Plaf103>, from: u32, owner: &mut AtRest| {
        for now in (from..from + 200).step_by(10) {
            fake::with(|h| h.uptime_ms = now);
            feeder.pass(now, owner, |_: PassRecord<Driver>| {});
        }
    };
    let food_events = || {
        let mut levels = Vec::new();
        while let Some(queued) = events::pending() {
            let body: EventBody = wellfed_proto::decode_padded(&queued.body).unwrap();
            if let EventBody::Food { level } = body {
                levels.push(level);
            }
            events::finish_pending(None);
        }
        levels
    };

    run(&mut feeder, 0, &mut owner);
    assert!(!on(Light::Food), "no reading yet");
    assert_eq!(food_events(), []);

    owner.food = Some(false);
    run(&mut feeder, 1_000, &mut owner);
    assert!(on(Light::Food), "low with no feed");
    assert_eq!(food_events(), [FoodLevel::Low]);

    run(&mut feeder, 2_000, &mut owner);
    assert_eq!(food_events(), [], "no repeat");

    owner.food = Some(true);
    run(&mut feeder, 3_000, &mut owner);
    assert!(!on(Light::Food), "a refill clears it");
    assert_eq!(food_events(), [FoodLevel::Present]);
    drop(rig);
}

/// Run the loop's passes over `ms`, 10 ms apart from `from`.
fn passes(feeder: &mut Feeder<Plaf103>, from: u32, ms: u32) {
    for now in (from..from + ms).step_by(10) {
        fake::with(|h| h.uptime_ms = now);
        feeder.pass(now, &mut Faulted, |_: PassRecord<Driver>| {});
    }
}

#[test]
fn the_lights_window_darkens_the_panel_outside_it() {
    let rig = Rig::new();
    // 07:00-22:00 local.
    let view = SettingsView {
        lights: true,
        sound: false,
        volume: 50,
        lights_window: Window::Between {
            start: 7 * 60,
            end: 22 * 60,
        },
        generation: 1,
        writable: true,
        ..Default::default()
    };
    settings::publish(view);
    let mut feeder = Box::new(Feeder::new(Plaf103::new()));

    // No clock yet: the window is open, so the fault and the lock show.
    passes(&mut feeder, 0, 200);
    assert!(on(Light::Lock) && on(Light::Fault));

    // 23:00 local at UTC-6: outside the window, every light dark.
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
    passes(&mut feeder, 200, 200);
    assert_eq!(lit(), [false; 4]);

    settings::publish(SettingsView {
        lights_window: Window::Always,
        ..view
    });
    passes(&mut feeder, 400, 200);
    assert!(on(Light::Lock) && on(Light::Fault));
    drop(rig);
}

/// Low food holds the light steady at rest, a feed's blink shows over it, and
/// a reading of food turns it off.
#[test]
fn low_food_is_steady_under_a_feed_s_blink() {
    let mut rig = Rig::new();
    let low = States {
        low_food: true,
        ..IDLE
    };
    assert!(
        rig.run(low, 1_200)
            .iter()
            .all(|(_, lit)| lit[Light::Food.index()])
    );

    let feeding = States {
        feeding: true,
        ..low
    };
    let seen = rig.run(feeding, 1_200);
    assert!(seen.iter().any(|(_, lit)| lit[Light::Food.index()]));
    assert!(seen.iter().any(|(_, lit)| !lit[Light::Food.index()]));

    rig.run(low, 20);
    assert!(on(Light::Food));
    rig.run(IDLE, 20);
    assert!(!on(Light::Food));
}
