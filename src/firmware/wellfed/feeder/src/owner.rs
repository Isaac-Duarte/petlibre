//! The firmware's one loop task: the panel's owner.
//!
//! Each step it reads the button service's gestures and a queued remote lock
//! change, steps the lock policy (`wellfed_feeder::panel`) and shows the lock
//! on its indicator. The product's other owners (feeding, the device API)
//! join this loop as they land, so the firmware stays within its two tasks:
//! this loop and the button service.
//!
//! The lock indicator (PA4) is on, as stock drives it, while the panel is
//! locked, and off while the `lights` setting is off. Port A has no input
//! readback, so a level is accepted when latch, pad, directions and stopped
//! outputs agree. A refused or unverified level drives every indicator LOW
//! and the indicator stays off for the rest of the boot; the lock itself
//! keeps working.

use core::fmt;

use wellfed_feeder::panel::{Config, Event, Lock, Panel};
use wellfed_sdk::buttons::Gesture;
use wellfed_sdk::hal::leds::{self, Decision, Led};

/// Between steps; a gesture waits at most this long in the queue.
pub const POLL_MS: u32 = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    /// The first step: the lock the panel starts with.
    Start {
        lock: Lock,
    },
    Panel(Event),
    /// The indicator failed and stays off for the rest of the boot.
    Indicator {
        decision: Option<Decision>,
    },
}

/// One record as the body of a `PLPANEL1` line.
pub fn describe(record: Record, f: &mut impl fmt::Write) -> fmt::Result {
    match record {
        Record::Start { lock } => write!(f, "start lock={lock}"),
        Record::Panel(Event::Changed { lock, source }) => {
            write!(f, "lock lock={lock} source={source}")
        }
        Record::Panel(Event::Asked { action, allowed }) => write!(
            f,
            "ask action={action} result={}",
            if allowed { "allowed" } else { "refused" }
        ),
        Record::Indicator { decision } => write!(
            f,
            "indicator led={} stop={}",
            Led::Lock.name(),
            match decision {
                None => "refused",
                Some(Decision::Verified | Decision::OutputOnly) => "verified",
                Some(Decision::InputMismatch) => "input-mismatch",
                Some(Decision::Abort) => "abort",
            }
        ),
    }
}

/// What the step reads from the rest of the firmware.
pub struct Inputs<G> {
    pub now_ms: u32,
    /// The button service's queued gestures.
    pub gestures: G,
    /// A queued remote change: true to lock.
    pub remote: Option<bool>,
    /// The `lights` setting; true until the settings load.
    pub lights: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Indicator {
    /// Holding a verified level.
    Shown(bool),
    /// A level was requested at this time and is settling.
    Settling {
        high: bool,
        since_ms: u32,
    },
    Failed,
}

#[derive(Debug)]
pub struct Owner {
    panel: Option<Panel>,
    indicator: Indicator,
}

impl Default for Owner {
    fn default() -> Self {
        Self {
            panel: None,
            // The runtime starts every indicator LOW.
            indicator: Indicator::Shown(false),
        }
    }
}

impl Owner {
    /// The lock now, once the first step has run.
    pub fn lock(&self) -> Option<Lock> {
        self.panel.as_ref().map(Panel::lock)
    }

    /// One step; `record` receives what happened, in order.
    pub fn step<G: Iterator<Item = Gesture>>(
        &mut self,
        inputs: Inputs<G>,
        mut record: impl FnMut(Record),
    ) -> u32 {
        let now = inputs.now_ms;
        let panel = self.panel.get_or_insert_with(|| {
            let panel = Panel::new(Config::STOCK, now);
            record(Record::Start { lock: panel.lock() });
            panel
        });

        for gesture in inputs.gestures {
            if let Some(event) = panel.gesture(now, gesture) {
                record(Record::Panel(event));
            }
        }
        let remote = inputs
            .remote
            .and_then(|locked| panel.remote(now, Lock::from_locked(locked)));
        for event in [remote, panel.tick(now)].into_iter().flatten() {
            record(Record::Panel(event));
        }

        let want = panel.lock().locked() && inputs.lights;
        self.show(now, want, &mut record)
    }

    /// Moves the indicator one stage towards `want`: request, then after
    /// the settle time verify. Returns the delay to the next step.
    fn show(&mut self, now: u32, want: bool, record: &mut impl FnMut(Record)) -> u32 {
        match self.indicator {
            Indicator::Failed => POLL_MS,
            Indicator::Shown(high) if high == want => POLL_MS,
            Indicator::Shown(_) => {
                if !leds::request(Led::Lock, want) {
                    return self.fail(None, record);
                }
                self.indicator = Indicator::Settling {
                    high: want,
                    since_ms: now,
                };
                leds::SETTLE_MS
            }
            Indicator::Settling { high, since_ms } => {
                if now.wrapping_sub(since_ms) < leds::SETTLE_MS {
                    return leds::SETTLE_MS;
                }
                let decision = leds::observe(Led::Lock).decision(high);
                if !matches!(decision, Decision::Verified | Decision::OutputOnly) {
                    return self.fail(Some(decision), record);
                }
                self.indicator = Indicator::Shown(high);
                POLL_MS
            }
        }
    }

    fn fail(&mut self, decision: Option<Decision>, record: &mut impl FnMut(Record)) -> u32 {
        let _ = leds::all_low();
        self.indicator = Indicator::Failed;
        record(Record::Indicator { decision });
        POLL_MS
    }
}
