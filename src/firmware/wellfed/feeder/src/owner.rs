//! The firmware's one loop task: the panel's owner.
//!
//! Each step it reads the button service's gestures and a queued remote lock
//! change, steps the pairing and lock policies (`wellfed_feeder::pairing`
//! and `panel`) and shows the lock on its indicator. Pairing sees each
//! gesture first and the panel gets only those it leaves, so a Dispense
//! press that confirms a pairing, or that is part of setup's code, feeds
//! nothing. An allowed Reset hold queues
//! FP15's key reset for the settings writer (`wellfed_hal::keys`). An
//! allowed Lock and Mic hold starts BLE setup mode (`setup`), which runs in
//! this loop until it reboots. The product's other owners (feeding, the
//! device API) join this loop as they land, so the firmware stays within its
//! two tasks: this loop and the button service.
//!
//! The lock indicator (PA4) is on, as stock drives it, while the panel is
//! locked, and off while the `lights` setting is off. Port A has no input
//! readback, so a level is accepted when latch, pad, directions and stopped
//! outputs agree. A refused or unverified level drives every indicator LOW
//! and the indicator stays off for the rest of the boot; the lock itself
//! keeps working.

use core::fmt;

use wellfed_core::noise::Keypair;
use wellfed_feeder::pairing::{self, Pairing, Path, Refusal};
use wellfed_feeder::panel::{Action, Config, Event, Lock, Panel};
use wellfed_sdk::buttons::Gesture;
use wellfed_sdk::hal::keys::{self, Change, Requested};
use wellfed_sdk::hal::leds::{self, Decision, Led};

use crate::setup::{self, Setup};

/// Between steps; a gesture waits at most this long in the queue.
pub const POLL_MS: u32 = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    /// The first step: the lock the panel starts with.
    Start {
        lock: Lock,
    },
    Panel(Event),
    Pairing(pairing::Event),
    /// An allowed Reset hold asked the settings writer for FP15's key reset.
    Reset {
        requested: Requested,
    },
    /// The indicator failed and stays off for the rest of the boot.
    Indicator {
        decision: Option<Decision>,
    },
    /// BLE setup mode (`PLSET1`, and `PLBLE1` for its link).
    Setup(setup::Record),
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
        Record::Pairing(event) => pair(event, f),
        Record::Reset { requested } => write!(f, "reset keys={requested}"),
        Record::Setup(record) => setup::describe(record, f),
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

fn pair(event: pairing::Event, f: &mut impl fmt::Write) -> fmt::Result {
    use pairing::Event as E;
    match event {
        E::Confirming { path } => write!(f, "pair confirming path={path}"),
        // How many presses, never which: the record must not carry the code.
        E::Keyed { path, entered } => write!(f, "pair {path} keyed={entered}"),
        E::Swallowed { path } => write!(f, "pair {path} swallowed"),
        E::Entered { path, .. } => write!(f, "pair {path} state=entered"),
        E::Admit { path, role } => write!(f, "pair admit path={path} role={role}"),
        E::Invited { id } => write!(f, "pair invited id={id}"),
        E::RecoveryOpened => f.write_str("pair recovery state=opened"),
        E::RecoveryClosed => f.write_str("pair recovery state=closed"),
        E::SetupOpened => f.write_str("pair setup state=opened"),
        E::Refused { path, refusal } => {
            write!(f, "pair refused path={path} refusal={refusal}")
        }
    }
}

/// What the step reads from the rest of the firmware.
pub struct Inputs<'a, G> {
    pub now_ms: u32,
    /// The button service's queued gestures.
    pub gestures: G,
    /// A queued remote change: true to lock.
    pub remote: Option<bool>,
    /// The `lights` setting; true until the settings load.
    pub lights: bool,
    /// The feeder's identity, once the settings writer has published it.
    pub identity: Option<&'a Keypair>,
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

pub struct Owner {
    panel: Option<Panel>,
    pairing: Pairing,
    indicator: Indicator,
    /// BLE setup mode, from the gesture that starts it until the reboot
    /// that ends it.
    setup: Setup,
    /// The recovery offer's decision, until the device API takes it.
    recovery: Option<pairing::Event>,
}

impl fmt::Debug for Owner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Owner")
            .field("panel", &self.panel)
            .field("pairing", &self.pairing)
            .field("setup", &self.setup.is_on())
            .finish_non_exhaustive()
    }
}

impl Default for Owner {
    fn default() -> Self {
        Self::new()
    }
}

impl Owner {
    /// Const, so the firmware builds it in its static: setup mode's state
    /// would not fit the loop's stack twice over.
    pub const fn new() -> Self {
        Self {
            panel: None,
            pairing: Pairing::new(pairing::Config::DEFAULT),
            // The runtime starts every indicator LOW.
            indicator: Indicator::Shown(false),
            setup: Setup::off(),
            recovery: None,
        }
    }

    /// Setup mode has ended and asks for the reboot that drops BLE.
    pub fn reboot_due(&self, now_ms: u32) -> bool {
        self.setup.reboot_due(now_ms)
    }

    /// The LAN recovery offer's decision this step (its code keyed in, or
    /// its timeout), for the device API's recovery session.
    pub fn take_recovery(&mut self) -> Option<pairing::Event> {
        self.recovery.take()
    }

    /// The pairing policy, which the device API also steps (invites).
    pub fn pairing(&mut self) -> &mut Pairing {
        &mut self.pairing
    }

    /// The lock now, once the first step has run.
    pub fn lock(&self) -> Option<Lock> {
        self.panel.as_ref().map(Panel::lock)
    }

    /// One step; `record` receives what happened, in order.
    pub fn step<G: Iterator<Item = Gesture>>(
        &mut self,
        inputs: Inputs<'_, G>,
        mut record: impl FnMut(Record),
    ) -> u32 {
        let now = inputs.now_ms;
        let panel = self.panel.get_or_insert_with(|| {
            let panel = Panel::new(Config::STOCK, now);
            record(Record::Start { lock: panel.lock() });
            panel
        });

        // A setup offer's decision, for setup mode's session.
        let mut decided = None;
        for gesture in inputs.gestures {
            if let Some(event) = self.pairing.gesture(now, gesture, panel.lock()) {
                panel.noted(now, gesture);
                record(Record::Pairing(event));
                if matches!(
                    event,
                    pairing::Event::Keyed { .. } | pairing::Event::Entered { .. }
                ) && self.setup.is_on()
                {
                    self.setup.pressed();
                }
                if event == pairing::Event::SetupOpened && !self.setup.is_on() {
                    self.setup.start(now);
                    record(Record::Setup(setup::Record::Start));
                }
                decided = decided.or(setup_decision(event));
                self.recovery = self.recovery.or(recovery_decision(event));
                continue;
            }
            let Some(event) = panel.gesture(now, gesture) else {
                continue;
            };
            record(Record::Panel(event));
            if event
                == (Event::Asked {
                    action: Action::Reset,
                    allowed: true,
                })
            {
                let requested = keys::request(Change::Reset);
                record(Record::Reset { requested });
            }
        }
        let remote = inputs
            .remote
            .and_then(|locked| panel.remote(now, Lock::from_locked(locked)));
        for event in [remote, panel.tick(now)].into_iter().flatten() {
            record(Record::Panel(event));
        }
        while let Some(event) = self.pairing.tick(now) {
            record(Record::Pairing(event));
            decided = decided.or(setup_decision(event));
            self.recovery = self.recovery.or(recovery_decision(event));
        }

        let want = panel.lock().locked() && inputs.lights;
        let delay = self.show(now, want, &mut record);
        if !self.setup.is_on() {
            return delay;
        }
        let mut setup_record = |r| record(Record::Setup(r));
        let setup_delay = self.setup.step(
            now,
            &mut self.pairing,
            decided,
            inputs.identity,
            &mut setup_record,
        );
        delay.min(setup_delay.max(1))
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

/// Pairing's answer to setup mode's pending offer: the code keyed in, or
/// its timeout. A locked panel refusing the setup gesture is not one.
fn setup_decision(event: pairing::Event) -> Option<pairing::Event> {
    decision(event, Path::Setup)
}

/// The same for the LAN recovery session the device API runs.
pub fn recovery_decision(event: pairing::Event) -> Option<pairing::Event> {
    decision(event, Path::Recovery)
}

fn decision(event: pairing::Event, path: Path) -> Option<pairing::Event> {
    match event {
        pairing::Event::Entered { path: p, .. } if p == path => Some(event),
        pairing::Event::Refused {
            path: p,
            refusal: Refusal::Unconfirmed,
        } if p == path => Some(event),
        _ => None,
    }
}
