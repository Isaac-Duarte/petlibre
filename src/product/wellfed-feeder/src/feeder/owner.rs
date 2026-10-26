//! The panel's owner, in the loop.
//!
//! Each step it reads the button service's gestures and a queued remote lock
//! change, steps the pairing and lock policies ([`pairing`](wellfed_device_api::pairing)
//! and [`panel`](crate::panel)) and shows the lock on its indicator. Pairing
//! sees each gesture first and the panel gets only those it leaves, so a
//! Dispense press that confirms a pairing, or that is part of setup's code,
//! feeds nothing. An allowed Reset hold queues the key reset for the
//! settings writer. An allowed Lock and Mic hold starts BLE setup mode
//! ([`setup`](super::setup)), which runs in this loop until it reboots. A
//! lock change made here (a button, the idle relock) goes to the event
//! history; a client's is recorded by the device API, with who made it.
//!
//! The lock indicator is on, as stock drives it, while the panel is locked,
//! and off while the `lights` setting is off. A level is accepted when it
//! reads back (`Platform::indicator_check`). A refused or unverified level
//! drives every indicator off and the indicator stays off for the rest of
//! the boot; the lock itself keeps working.

use core::fmt;

use wellfed_core::buttons::Gesture;
use wellfed_proto::api::{EventBody, Source as EventSource};
use wellfed_session::noise::Keypair;

use super::events;
use super::platform::{Check, Platform};
use super::setup::{self, Setup};
use crate::panel::{Action, Config, Event, Panel, Source};
use wellfed_device_api::mailbox::Lock;
use wellfed_device_api::mailbox::{ClientChange, Requested};
use wellfed_device_api::pairing::{self, Pairing, Path, Refusal};

/// Between steps; a gesture waits at most this long in the queue.
pub const POLL_MS: u32 = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record<D> {
    /// The first step: the lock the panel starts with.
    Start {
        lock: Lock,
    },
    Panel(Event),
    Pairing(pairing::Event),
    /// An allowed Reset hold asked the settings writer for the key reset.
    Reset {
        requested: Requested,
    },
    /// The indicator failed and stays off for the rest of the boot.
    Indicator {
        /// The indicator's pad.
        led: &'static str,
        /// How its level read back; `None` when the request was refused.
        check: Option<Check>,
    },
    /// BLE setup mode.
    Setup(setup::Record<D>),
}

/// One record as the body of its line: `PLPANEL1` for the panel's,
/// `PLSET1` (or the driver's own tag) for setup mode's.
pub fn describe<D: fmt::Display>(record: Record<D>, f: &mut impl fmt::Write) -> fmt::Result {
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
        Record::Indicator { led, check } => write!(
            f,
            "indicator led={led} stop={}",
            match check {
                None => "refused",
                Some(Check::Held) => "verified",
                Some(Check::InputMismatch) => "input-mismatch",
                Some(Check::Abort) => "abort",
            }
        ),
    }
}

/// A pairing event's body, which the device API logs for its own too.
pub fn pair(event: pairing::Event, f: &mut impl fmt::Write) -> fmt::Result {
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
    /// An update key's decision (its Dispense press, or the timeout), until
    /// the device API takes it.
    signer: Option<pairing::Event>,
    /// Gestures that ended this boot, for the statistics.
    presses: u32,
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
            // Every indicator starts off.
            indicator: Indicator::Shown(false),
            setup: Setup::off(),
            recovery: None,
            signer: None,
            presses: 0,
        }
    }

    /// Gestures that ended this boot (every chord released, whatever it
    /// did).
    pub const fn presses(&self) -> u32 {
        self.presses
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

    /// The update-key offer's decision this step, for the device API's
    /// session that waits for it.
    pub fn take_signer(&mut self) -> Option<pairing::Event> {
        self.signer.take()
    }

    /// The pairing policy, which the device API also steps (invites).
    pub fn pairing(&mut self) -> &mut Pairing {
        &mut self.pairing
    }

    /// The lock now, once the first step has run.
    pub fn lock(&self) -> Option<Lock> {
        self.panel.as_ref().map(Panel::lock)
    }

    /// Whether BLE setup mode runs.
    pub fn setup_on(&self) -> bool {
        self.setup.is_on()
    }

    /// One step at `now`: `remote` is a queued remote change (true to
    /// lock), `identity` the feeder's once the writer has published it;
    /// `record` receives what happened, in order.
    pub fn step<P: Platform>(
        &mut self,
        platform: &mut P,
        now: u32,
        remote: Option<bool>,
        identity: Option<&Keypair>,
        mut record: impl FnMut(Record<P::Record>),
    ) -> u32 {
        // True until the settings load.
        let lights = platform.settings().is_none_or(|view| view.lights);
        let panel = self.panel.get_or_insert_with(|| {
            let panel = Panel::new(Config::STOCK, now);
            record(Record::Start { lock: panel.lock() });
            panel
        });

        // A setup offer's decision, for setup mode's session.
        let mut decided = None;
        while let Some(gesture) = platform.gesture() {
            if matches!(gesture, Gesture::Up { .. }) {
                self.presses = self.presses.saturating_add(1);
            }
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
                    self.setup.start(platform, now);
                    record(Record::Setup(setup::Record::Start));
                }
                decided = decided.or(setup_decision(event));
                self.recovery = self.recovery.or(recovery_decision(event));
                self.signer = self.signer.or(signer_decision(event));
                continue;
            }
            let Some(event) = panel.gesture(now, gesture) else {
                continue;
            };
            panel_event(platform, now, event, &mut record);
            if event
                == (Event::Asked {
                    action: Action::Reset,
                    allowed: true,
                })
            {
                let requested = platform.keys_request(ClientChange::Reset);
                record(Record::Reset { requested });
            }
        }
        let remote = remote.and_then(|locked| panel.remote(now, Lock::from_locked(locked)));
        for event in [remote, panel.tick(now)].into_iter().flatten() {
            panel_event(platform, now, event, &mut record);
        }
        while let Some(event) = self.pairing.tick(now) {
            record(Record::Pairing(event));
            decided = decided.or(setup_decision(event));
            self.recovery = self.recovery.or(recovery_decision(event));
            self.signer = self.signer.or(signer_decision(event));
        }

        let want = panel.lock().locked() && lights;
        let delay = self.show(platform, now, want, &mut record);
        if !self.setup.is_on() {
            return delay;
        }
        let mut setup_record = |r| record(Record::Setup(r));
        let setup_delay = self.setup.step(
            platform,
            now,
            &mut self.pairing,
            decided,
            identity,
            &mut setup_record,
        );
        delay.min(setup_delay.max(1))
    }

    /// Moves the indicator one stage towards `want`: request, then after
    /// the settle time verify. Returns the delay to the next step.
    fn show<P: Platform>(
        &mut self,
        platform: &mut P,
        now: u32,
        want: bool,
        record: &mut impl FnMut(Record<P::Record>),
    ) -> u32 {
        match self.indicator {
            Indicator::Failed => POLL_MS,
            Indicator::Shown(high) if high == want => POLL_MS,
            Indicator::Shown(_) => {
                if !platform.indicator(want) {
                    return self.fail(platform, None, record);
                }
                self.indicator = Indicator::Settling {
                    high: want,
                    since_ms: now,
                };
                P::INDICATOR_SETTLE_MS
            }
            Indicator::Settling { high, since_ms } => {
                if now.wrapping_sub(since_ms) < P::INDICATOR_SETTLE_MS {
                    return P::INDICATOR_SETTLE_MS;
                }
                let check = platform.indicator_check(high);
                if check != Check::Held {
                    return self.fail(platform, Some(check), record);
                }
                self.indicator = Indicator::Shown(high);
                POLL_MS
            }
        }
    }

    fn fail<P: Platform>(
        &mut self,
        platform: &mut P,
        check: Option<Check>,
        record: &mut impl FnMut(Record<P::Record>),
    ) -> u32 {
        platform.indicators_off();
        self.indicator = Indicator::Failed;
        record(Record::Indicator {
            led: P::LOCK_INDICATOR,
            check,
        });
        POLL_MS
    }
}

/// A panel event: a lock change made here goes to the history (a client's
/// is the device API's to record, with who made it), then to the log.
fn panel_event<P: Platform>(
    platform: &mut P,
    now: u32,
    event: Event,
    record: &mut impl FnMut(Record<P::Record>),
) {
    if let Event::Changed { lock, source } = event
        && source != Source::Remote
    {
        let by = match source {
            Source::Button => EventSource::Panel,
            Source::Idle | Source::Remote => EventSource::Feeder,
        };
        let body = EventBody::Lock {
            locked: lock.locked(),
            by,
        };
        events::record(platform, now, body);
    }
    record(Record::Panel(event));
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

/// An update key's Dispense press, or its timeout, for the session that
/// offered it.
pub fn signer_decision(event: pairing::Event) -> Option<pairing::Event> {
    match event {
        pairing::Event::Admit {
            path: Path::Signer, ..
        } => Some(event),
        _ => decision(event, Path::Signer),
    }
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
