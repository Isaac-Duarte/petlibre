//! FP11: the panel's child lock, as stock 1.1.15 keeps it.
//!
//! Stock's panel task runs every 50 ticks and counts visits (static analysis
//! and emulation of the stock handlers, [stock behavior]):
//! - Lock held for 100 visits (about 5 s) toggles the lock, once per hold.
//! - An unlocked panel relocks after 600 idle visits (about 30 s).
//! - A locked panel ignores Dispense, Mic and the Reset hold.
//!
//! This keeps those rules over the SDK's gestures. The Lock hold is the
//! button service's first hold threshold ([`BUTTONS`]), so it works whether
//! the panel is locked or not: the lock never locks the owner out of
//! unlocking. A gesture in progress holds off the relock, which then counts
//! from the release. A signed request may lock or unlock from the network;
//! an unlock from there relocks on the same timer, as a button unlock does.
//!
//! The panel starts locked at every boot. Stock's boot state is not
//! recovered, and starting locked means an unattended reboot never leaves
//! Dispense live.
//!
//! What an allowed action does (a feed, a tune, a reset) belongs to its own
//! owner; this module only decides whether the panel may ask.
//!
//! [stock behavior]: ../../../../docs/devices/plaf103/stock/README.md#panel-buttons-and-child-lock

use petlibre_core::buttons::{Button, Buttons, Config as ButtonsConfig, Gesture};
use petlibre_core::{deadline_passed, interval_valid, named_enum};

/// The button service's thresholds for the panel: one hold, at stock's
/// 5 s (100 visits of 50 ms).
pub const BUTTONS: ButtonsConfig = ButtonsConfig {
    debounce_ms: 30,
    holds_ms: [5_000, 0, 0],
    hold_count: 1,
};

/// How the panel locks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// The hold level (of [`BUTTONS`]) at which Lock toggles and Reset acts.
    pub hold_level: u8,
    /// Idle time before an unlocked panel relocks.
    pub relock_ms: u32,
}

impl Config {
    /// Stock's: 600 idle visits of 50 ms.
    pub const STOCK: Self = Self {
        hold_level: 1,
        relock_ms: 30_000,
    };

    pub const fn valid(&self) -> bool {
        self.hold_level > 0 && interval_valid(self.relock_ms)
    }
}

named_enum! {
    pub enum Lock {
        Locked = "locked",
        Unlocked = "unlocked",
    }
}

impl Lock {
    pub const fn from_locked(locked: bool) -> Self {
        if locked { Self::Locked } else { Self::Unlocked }
    }

    pub const fn locked(self) -> bool {
        matches!(self, Self::Locked)
    }
}

named_enum! {
    /// What changed the lock.
    pub enum Source {
        Button = "button",
        Idle = "idle",
        Remote = "remote",
    }
}

named_enum! {
    /// What the panel asks for.
    pub enum Action {
        /// Dispense pressed: one manual feed (FP05).
        Feed = "feed",
        /// Mic pressed: stock's audio selection (FP13).
        Mic = "mic",
        /// Reset held: stock's factory restore (FP15).
        Reset = "reset",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Changed {
        lock: Lock,
        source: Source,
    },
    /// The panel asked for `action`; `allowed` is false while locked.
    Asked {
        action: Action,
        allowed: bool,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct Panel {
    config: Config,
    lock: Lock,
    /// The last activity: a gesture event, or a remote change.
    active_ms: u32,
    /// A gesture is in progress (no `Up` yet).
    pressed: bool,
}

impl Panel {
    /// The panel at boot: locked.
    pub const fn new(config: Config, now_ms: u32) -> Self {
        Self {
            config,
            lock: Lock::Locked,
            active_ms: now_ms,
            pressed: false,
        }
    }

    pub const fn lock(&self) -> Lock {
        self.lock
    }

    /// One gesture from the button service.
    pub fn gesture(&mut self, now_ms: u32, gesture: Gesture) -> Option<Event> {
        self.active_ms = now_ms;
        self.pressed = !matches!(gesture, Gesture::Up { .. });

        let held = |level: u8| level == self.config.hold_level;
        let action = match gesture {
            Gesture::Hold { chord, level } if alone(chord, Button::Lock) && held(level) => {
                let lock = Lock::from_locked(!self.lock.locked());
                return Some(self.set(lock, Source::Button));
            }
            Gesture::Down { chord } if alone(chord, Button::Dispense) => Action::Feed,
            Gesture::Down { chord } if alone(chord, Button::Mic) => Action::Mic,
            Gesture::Hold { chord, level } if alone(chord, Button::Reset) && held(level) => {
                Action::Reset
            }
            _ => return None,
        };

        Some(Event::Asked {
            action,
            allowed: !self.lock.locked(),
        })
    }

    /// A signed lock or unlock from the network; `None` if already so.
    pub fn remote(&mut self, now_ms: u32, lock: Lock) -> Option<Event> {
        self.active_ms = now_ms;
        (lock != self.lock).then(|| self.set(lock, Source::Remote))
    }

    /// Relocks an unlocked panel left idle for the configured time.
    pub fn tick(&mut self, now_ms: u32) -> Option<Event> {
        let idle = !self.pressed
            && deadline_passed(now_ms, self.active_ms.wrapping_add(self.config.relock_ms));
        (self.lock == Lock::Unlocked && idle).then(|| self.set(Lock::Locked, Source::Idle))
    }

    fn set(&mut self, lock: Lock, source: Source) -> Event {
        self.lock = lock;
        Event::Changed { lock, source }
    }
}

fn alone(chord: Buttons, button: Button) -> bool {
    chord == Buttons::of(button)
}
