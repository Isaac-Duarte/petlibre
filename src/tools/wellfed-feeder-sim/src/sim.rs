//! [`Sim`], one simulated feeder: the device API server, the panel and
//! pairing policy, and the stores, stepped in the firmware loop's order.
//!
//! One pass does what the loop task does (`src/firmware/wellfed/feeder`):
//! the boot event once, the panel's queued remote lock and its idle relock,
//! the pairing timers, the client table if the writer changed it, one
//! server step, then the writer's turn, the update stage's (the service
//! task's) and the live samples.
//!
//! A boot first runs the loader's choice and KM0's trial ([`Banks::boot`]);
//! a new build in trial confirms itself at once, raising the floor as the
//! firmware's confirmation does, unless the banks say it never will, in
//! which case it resets after [`TRIAL_MS`] as the supervisor's deadline
//! makes it. Button presses
//! come from the caller ([`Sim::gesture`]) and go to pairing first, then to
//! the panel, as the button service's gestures do.
//!
//! Nothing here reads a clock: every call takes the time, so tests can run
//! virtual time and the driver can run real time through the same code.

use std::cell::RefCell;
use std::rc::Rc;

use wellfed_board_plaf103::storage;
use wellfed_core::buttons::Gesture;
use wellfed_core::ota::{self, CONFIRM_LEN};
use wellfed_core::store::slot::SlotIo;
use wellfed_core::store::update::{State, UpdateStore};
use wellfed_feeder::api::{self as server, Pass, Server};
use wellfed_feeder::pairing::{self, Pairing, Path, Refusal};
use wellfed_feeder::panel::{self, Action, Config, Lock, Panel, Source};
use wellfed_feeder::store::clients::Role;
use wellfed_hal::ota::mailbox;
use wellfed_hal::ota::update::{self as update, Floor, Update};
use wellfed_proto::api::{EventBody, Metric, Sample, Source as EventSource};
use wellfed_session::noise::Keypair;

use crate::banks::{Banks, Bus, Trial};
use crate::device::{self, DeviceError, Random, SimDevice};
use crate::flash::{Flash, Region};

/// A build that never confirms resets this long into each trial boot: the
/// firmware's supervisor waits ten minutes; the simulator does not.
pub const TRIAL_MS: u32 = 2_000;

/// The feeder samples its supply this often.
const POWER_MS: u32 = 5_000;
const RSSI_MS: u32 = 30_000;

/// What one step did, for the simulator's log. No record carries a key or
/// a secret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Server(server::Record),
    Panel(panel::Event),
    Pairing(pairing::Event),
    /// A Reset hold on an unlocked panel.
    Reset(Reset),
    /// The boot: the bank the loader chose, and whether its build is in
    /// trial and confirmed itself.
    Boot {
        bank: u8,
        trial: bool,
        confirmed: Option<Floor>,
    },
    Update(update::Record),
}

/// How a Reset hold ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reset {
    /// A new identity, and no clients.
    Done,
    /// The store or the generator failed; nothing changed.
    Failed(DeviceError),
}

pub struct Sim {
    server: Box<Server>,
    device: SimDevice,
    pairing: Pairing,
    panel: Panel,
    identity: Keypair,
    clients_seen: u32,
    /// Pairing's decision on a LAN recovery offer, for the next server step.
    recovery: Option<pairing::Event>,
    booted: bool,
    next_power: u32,
    next_rssi: u32,
    banks: Rc<RefCell<Banks>>,
    bank: u8,
    stage: Box<Update<Region>>,
    /// When a build that never confirms resets.
    trial_reset: Option<u32>,
    /// What the boot did, for the first pass's log.
    boot: Record,
}

impl std::fmt::Debug for Sim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sim")
            .field("server", &self.server)
            .field("device", &self.device)
            .finish_non_exhaustive()
    }
}

impl Sim {
    /// The feeder as it boots from `flash` at `now_ms`, for the
    /// `boot_count`th time: the panel locked, the clock invalid, no sessions.
    pub fn open(
        flash: &Rc<Flash>,
        banks: &Rc<RefCell<Banks>>,
        update_key: Option<[u8; 32]>,
        random: Random,
        now_ms: u32,
        boot_count: u32,
    ) -> Result<Self, DeviceError> {
        let (bank, trial, build) = boot(flash, &mut banks.borrow_mut(), update_key);
        // The mailbox and the interlock are RAM: a restart forgets every
        // request and chunk, and the write turn a give-way kept.
        mailbox::reset();
        wellfed_hal::interlock::INTERLOCK.reset();
        let store = Region::new(flash, storage::UPDATE);
        let mut stage = Box::new(Update::new(bank, build, update_key, store));
        let confirmed = (trial && banks.borrow().trial == Trial::Confirms)
            .then(|| confirm(flash, &mut stage, &build));
        let trial_reset = (trial && confirmed.is_none()).then(|| now_ms.wrapping_add(TRIAL_MS));

        let device = SimDevice::open(flash, random, boot_count, bank, build)?;
        let identity = keypair(&device)?;
        Ok(Self {
            server: Box::new(Server::new()),
            device,
            pairing: Pairing::new(pairing::Config::DEFAULT),
            panel: Panel::new(Config::STOCK, now_ms),
            identity,
            clients_seen: 0,
            recovery: None,
            booted: false,
            next_power: now_ms,
            next_rssi: now_ms,
            banks: Rc::clone(banks),
            bank,
            stage,
            trial_reset,
            boot: Record::Boot {
                bank,
                trial,
                confirmed,
            },
        })
    }

    /// The feeder's identity public key.
    pub fn identity(&self) -> [u8; 32] {
        *self.identity.public()
    }

    pub fn device(&mut self) -> &mut SimDevice {
        &mut self.device
    }

    /// The server, for the driver to move each connection's bytes.
    pub(crate) fn server(&mut self) -> &mut Server {
        &mut self.server
    }

    /// Whether the server asked for a restart, taken once: the driver drops
    /// every connection and opens the feeder again from its flash.
    pub fn take_reboot(&mut self) -> bool {
        self.device.take_reboot()
    }

    /// One gesture from the buttons: pairing takes it first (a code being
    /// keyed in, the recovery or setup hold), otherwise the panel.
    pub fn gesture(&mut self, now_ms: u32, gesture: Gesture, mut record: impl FnMut(Record)) {
        self.device.set_now(now_ms);
        if let Some(event) = self.pairing.gesture(now_ms, gesture, self.panel.lock()) {
            self.panel.noted(now_ms, gesture);
            record(Record::Pairing(event));
            self.recovery = self.recovery.or(recovery_decision(event));
            return;
        }
        let Some(event) = self.panel.gesture(now_ms, gesture) else {
            return;
        };
        record(Record::Panel(event));
        self.panel_event(now_ms, event);

        if event
            == (panel::Event::Asked {
                action: Action::Reset,
                allowed: true,
            })
        {
            record(Record::Reset(self.reset()));
        }
    }

    /// The Reset hold: a new identity and no clients, as the writer's reset.
    fn reset(&mut self) -> Reset {
        let result = self.device.reset().and_then(|()| keypair(&self.device));
        match result {
            Ok(identity) => {
                self.identity = identity;
                Reset::Done
            }
            Err(error) => Reset::Failed(error),
        }
    }

    /// A panel change made here, not by a client (whose change the server
    /// records with who made it), goes to the history.
    fn panel_event(&mut self, now_ms: u32, event: panel::Event) {
        let panel::Event::Changed { lock, source } = event else {
            return;
        };
        if source == Source::Remote {
            return;
        }
        let by = match source {
            Source::Button => EventSource::Panel,
            Source::Idle | Source::Remote => EventSource::Feeder,
        };
        let body = EventBody::Lock {
            locked: lock.locked(),
            by,
        };
        self.device.record_event(now_ms, body);
    }

    /// One pass of the loop at `now_ms`. Returns true while work is
    /// waiting, so the driver should pass again soon.
    pub fn pass(&mut self, now_ms: u32, mut record: impl FnMut(Record)) -> bool {
        self.device.set_now(now_ms);
        if !self.booted {
            self.booted = true;
            record(self.boot);
            let body = EventBody::Boot {
                bank: self.bank,
                reset_flags: 0,
            };
            self.device.record_event(now_ms, body);
        }

        let remote = self
            .device
            .take_lock()
            .and_then(|locked| self.panel.remote(now_ms, Lock::from_locked(locked)));
        for event in [remote, self.panel.tick(now_ms)].into_iter().flatten() {
            record(Record::Panel(event));
            self.panel_event(now_ms, event);
        }
        while let Some(event) = self.pairing.tick(now_ms) {
            record(Record::Pairing(event));
            self.recovery = self.recovery.or(recovery_decision(event));
        }
        self.device.publish_lock(self.panel.lock().locked());

        self.refresh_clients(&mut record);
        let busy = self.server.step(
            Pass {
                now_ms,
                device: &mut self.device,
                pairing: &mut self.pairing,
                identity: Some(&self.identity),
                // No feeding owner yet.
                feeding: false,
                recovery: self.recovery.take(),
                signer: None,
            },
            |r| record(Record::Server(r)),
        );
        self.device.write();
        let updating = self.update(now_ms, &mut record);
        self.samples(now_ms);
        busy || updating
    }

    /// The update stage's turn, as the service task gives it; whether it
    /// has work waiting.
    fn update(&mut self, now_ms: u32, record: &mut impl FnMut(Record)) -> bool {
        if self
            .trial_reset
            .is_some_and(|at| wellfed_core::deadline_passed(now_ms, at))
        {
            self.device.request_reboot();
        }
        let mut banks = self.banks.borrow_mut();
        let mut bus = Bus {
            banks: &mut banks,
            bank: self.bank,
        };
        let step = self.stage.step(&mut bus, now_ms);
        if let Some(event) = step.record {
            record(Record::Update(event));
        }
        if step.restart {
            self.device.request_reboot();
        }
        step.delay_ms <= update::BUSY_MS
    }

    /// Copy the writer's client table into the server when it changed, so a
    /// revoked key's sessions close.
    fn refresh_clients(&mut self, record: &mut impl FnMut(Record)) {
        let (table, generation) = self.device.table();
        if generation == self.clients_seen {
            return;
        }
        self.clients_seen = generation;
        *self.server.clients_table().0 = table;
        self.server
            .clients_changed(generation, |r| record(Record::Server(r)));
        // The update stage reads the owners' update keys where the
        // firmware's settings writer publishes them.
        wellfed_hal::ota::signers::publish(
            table
                .iter()
                .flatten()
                .filter(|client| client.role == Role::UpdateSigner)
                .map(|client| client.key),
        );
    }

    fn samples(&mut self, now_ms: u32) {
        if wellfed_core::deadline_passed(now_ms, self.next_power) {
            self.next_power = now_ms.wrapping_add(POWER_MS);
            self.server.sample(Sample {
                at_ms: now_ms,
                metric: Metric::Power(device::POWER),
            });
        }
        if wellfed_core::deadline_passed(now_ms, self.next_rssi) {
            self.next_rssi = now_ms.wrapping_add(RSSI_MS);
            self.server.sample(Sample {
                at_ms: now_ms,
                metric: Metric::Rssi(device::RSSI),
            });
        }
    }
}

/// The loader's choice and KM0's trial for this boot: the bank, whether its
/// build is in trial, and the build. A build bank 0 activated in bank 1 is
/// the one the update store names.
fn boot(flash: &Rc<Flash>, banks: &mut Banks, key: Option<[u8; 32]>) -> (u8, bool, [u8; 32]) {
    let mut store = UpdateStore::default();
    store.open(Region::new(flash, storage::UPDATE));
    if store.record().bank0.is_none()
        && let Some(bank0) = banks.bank0_record(key)
    {
        // As if bank 0 had booted once: bank 1 gives way only to it.
        let _ = store.set_bank0(bank0);
    }
    if banks.bank1_valid()
        && let State::Activated(candidate) = store.record().state
    {
        banks.bank1_build = candidate.build_id;
    }

    let mut record = [0u8; CONFIRM_LEN];
    let confirmed = Region::new(flash, storage::CONFIRM).read(0, 0, &mut record)
        && ota::confirmed(&record, &banks.bank1_build).is_some();
    let (bank, trial) = banks.boot(confirmed);
    (bank, trial, banks.build(bank))
}

/// The build confirms: the floor first, then the record, as the
/// firmware's confirmation writes them.
fn confirm(flash: &Rc<Flash>, stage: &mut Update<Region>, build: &[u8; 32]) -> Floor {
    let floor = stage.raise_floor();
    let version = match floor {
        Floor::Raised { version } => version,
        Floor::Unchanged => 0,
        Floor::Failed => return floor,
    };
    let record = ota::confirm_record(build, version);
    let sector = Region::new(flash, storage::CONFIRM);
    if !(sector.erase(0) && sector.program(0, 0, &record)) {
        return Floor::Failed;
    }
    floor
}

fn keypair(device: &SimDevice) -> Result<Keypair, DeviceError> {
    let secret = device.identity().ok_or(DeviceError::NoIdentity)?;
    Ok(Keypair::from_secret(*secret))
}

/// The recovery offer's decision, which the server's recovery session waits
/// for: its code keyed in, or its time run out.
fn recovery_decision(event: pairing::Event) -> Option<pairing::Event> {
    match event {
        pairing::Event::Entered {
            path: Path::Recovery,
            ..
        }
        | pairing::Event::Refused {
            path: Path::Recovery,
            refusal: Refusal::Unconfirmed,
        } => Some(event),
        _ => None,
    }
}
