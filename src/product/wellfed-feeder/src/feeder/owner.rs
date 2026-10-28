//! The panel's owner, in the loop.
//!
//! Each step it reads the button service's gestures and a queued remote lock
//! change, steps the pairing and lock policies ([`pairing`](wellfed_device_api::pairing)
//! and [`panel`](crate::panel)); the lights ([`lights`](super::lights))
//! show the lock and a pending offer from what it holds. Pairing
//! sees each gesture first and the panel gets only those it leaves, so a
//! Dispense press that confirms a pairing, or that is part of setup's code,
//! feeds nothing. An allowed Reset hold queues the factory reset for the
//! settings writer, or is refused while a feed is queued or runs; once the
//! writer has marked it, the loop restarts the feeder (waiting for any feed
//! in flight) and the next boot finishes it. An allowed Lock and Mic hold
//! starts BLE setup mode ([`setup`](super::setup)), which runs in this loop
//! until it reboots. A
//! lock change made here (a button, the idle relock) goes to the event
//! history; a client's is recorded by the device API, with who made it.

use core::fmt;

use wellfed_core::buttons::Gesture;
use wellfed_proto::api::{EventBody, Source as EventSource};
use wellfed_session::noise::Keypair;

use super::events;
use super::platform::Platform;
use super::setup::{self, Setup};
use crate::panel::{Action, Config, Event, Panel, Source};
use wellfed_core::named_enum;
use wellfed_device_api::mailbox::Lock;
use wellfed_device_api::mailbox::{ClientChange, KeyResult, Requested};
use wellfed_device_api::pairing::{self, Pairing, Path, Refusal};

named_enum! {
    /// Why an allowed Reset hold started no reset.
    pub enum ResetRefusal {
        /// A feed is queued or runs: the reset's restart would cut it, so
        /// the hold is refused rather than deferred past the feed.
        Feeding = "feeding",
        /// An install is under way: its restart into a trial boot would
        /// meet the reset's marker.
        Updating = "updating",
    }
}

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
    /// An allowed Reset hold asked the settings writer for the factory
    /// reset.
    Reset {
        requested: Requested,
    },
    /// An allowed Reset hold that started nothing.
    ResetRefused {
        reason: ResetRefusal,
    },
    /// The writer's answer to the reset: `Done` once its marker is down and
    /// the clients forgotten, and the feeder restarts to finish it.
    ResetWritten {
        result: KeyResult,
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
        Record::ResetRefused { reason } => write!(f, "reset refused={reason}"),
        Record::ResetWritten { result } => write!(f, "reset result={result}"),
        Record::Setup(record) => setup::describe(record, f),
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

pub struct Owner {
    panel: Option<Panel>,
    pairing: Pairing,
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
    /// The panel allowed a Dispense press this step: one feed to queue.
    dispense: bool,
    /// A button went down this step: one beep, as stock beeps every press.
    pressed: bool,
    /// The factory reset's ticket in the client mailbox, until the writer
    /// answers it.
    reset_ticket: Option<u32>,
    /// The writer marked the factory reset: restart to finish it.
    reset_restart: bool,
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
            setup: Setup::off(),
            recovery: None,
            signer: None,
            presses: 0,
            dispense: false,
            pressed: false,
            reset_ticket: None,
            reset_restart: false,
        }
    }

    /// A Dispense press the panel allowed, taken once: the loop queues its
    /// feed through the path a client's `Feed` takes. Presses in one step
    /// ask for one feed; the owner would refuse a second while the first
    /// runs anyway.
    pub fn take_dispense(&mut self) -> bool {
        core::mem::take(&mut self.dispense)
    }

    /// A button went down since the last call, whatever the press then
    /// does (locked, refused or a code digit alike): the loop beeps once.
    pub fn take_pressed(&mut self) -> bool {
        core::mem::take(&mut self.pressed)
    }

    /// Gestures that ended this boot (every chord released, whatever it
    /// did).
    pub const fn presses(&self) -> u32 {
        self.presses
    }

    /// Setup mode has ended and asks for the reboot that drops BLE, or the
    /// factory reset is marked and the next boot finishes it. The loop
    /// waits for a feed in flight either way.
    pub fn reboot_due(&self, now_ms: u32) -> bool {
        self.reset_restart || self.setup.reboot_due(now_ms)
    }

    /// A factory reset is queued for the writer or marked and waits for its
    /// restart: no install may start meanwhile.
    pub const fn reset_pending(&self) -> bool {
        self.reset_ticket.is_some() || self.reset_restart
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

    /// One step at `now`: `refusal` is why a Reset hold would be refused
    /// now (a feed queued or running, an install), `remote` is a queued
    /// remote change (true to
    /// lock), `identity` the feeder's once the writer has published it;
    /// `record` receives what happened, in order.
    pub fn step<P: Platform>(
        &mut self,
        platform: &mut P,
        now: u32,
        refusal: Option<ResetRefusal>,
        remote: Option<bool>,
        identity: Option<&Keypair>,
        mut record: impl FnMut(Record<P::Record>),
    ) -> u32 {
        let panel = self.panel.get_or_insert_with(|| {
            let panel = Panel::new(Config::STOCK, now);
            record(Record::Start { lock: panel.lock() });
            panel
        });

        // A setup offer's decision, for setup mode's session.
        let mut decided = None;
        while let Some(gesture) = platform.gesture() {
            if matches!(gesture, Gesture::Down { .. }) {
                self.pressed = true;
            }
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
                    action: Action::Feed,
                    allowed: true,
                })
            {
                self.dispense = true;
            }
            if event
                == (Event::Asked {
                    action: Action::Reset,
                    allowed: true,
                })
            {
                reset_hold(platform, refusal, &mut self.reset_ticket, &mut record);
            }
        }
        if let Some(ticket) = self.reset_ticket
            && let Some(result) = platform.keys_outcome(ticket)
        {
            self.reset_ticket = None;
            self.reset_restart = result == KeyResult::Done;
            record(Record::ResetWritten { result });
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

        let delay = POLL_MS;
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
}

/// An allowed Reset hold: the factory reset, queued for the settings
/// writer unless `refusal` names why not; `ticket` takes its ticket.
fn reset_hold<P: Platform>(
    platform: &mut P,
    refusal: Option<ResetRefusal>,
    ticket: &mut Option<u32>,
    record: &mut impl FnMut(Record<P::Record>),
) {
    if let Some(reason) = refusal {
        record(Record::ResetRefused { reason });
        return;
    }

    let requested = platform.keys_request(ClientChange::Reset);
    if requested == Requested::Queued {
        *ticket = Some(platform.keys_ticket());
    }
    record(Record::Reset { requested });
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
