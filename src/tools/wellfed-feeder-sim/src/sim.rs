//! [`Sim`], one simulated feeder: the product's loop
//! ([`wellfed_feeder::feeder::Feeder`]) on the simulator's platform
//! ([`SimPlatform`]), then the tasks the firmware runs beside its loop: the
//! settings writer's turn and the update stage's.
//!
//! A boot first runs the loader's choice and KM0's trial ([`Banks::boot`]);
//! a new build in trial confirms itself at once, raising the floor as the
//! firmware's confirmation does, unless the banks say it never will, in
//! which case it resets after [`TRIAL_MS`] as the supervisor's deadline
//! makes it. Button presses come from the caller ([`Sim::gesture`]) and wait
//! for the next pass, as the button service's queued gestures do.
//!
//! Nothing here reads a clock: every call takes the time, so tests can run
//! virtual time and the driver can run real time through the same code.

use std::cell::RefCell;
use std::convert::Infallible;
use std::fmt;
use std::net::Ipv4Addr;
use std::rc::Rc;

use wellfed_board_plaf103::storage;
use wellfed_core::buttons::Gesture;
use wellfed_core::ota::{self, CONFIRM_LEN};
use wellfed_core::store::slot::SlotIo;
use wellfed_core::store::update::{State, UpdateStore};
use wellfed_device_api::mailbox::KeyResult;
use wellfed_feeder::feeder::{self, Feeder, NoFeeds};
use wellfed_feeder::panel;
use wellfed_hal::ota::mailbox;
use wellfed_hal::ota::update::{self as update, Floor, Update};
use wellfed_session::noise::Keypair;
use tokio::net::{TcpStream, UdpSocket};

use crate::banks::{Banks, Boot, Bus, Trial};
use crate::flash::{Flash, Region};
use crate::platform::{DeviceError, Random, SimPlatform};

/// A build that never confirms resets this long into each trial boot: the
/// firmware's supervisor waits ten minutes; the simulator does not.
pub const TRIAL_MS: u32 = 2_000;

/// What one step did, for the simulator's log. No record carries a key or
/// a secret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    /// The feeder's loop, as its firmware logs it.
    Feeder(feeder::Record<Infallible>),
    /// The boot: the bank the loader chose, and whether its build is in
    /// trial and confirmed itself.
    Boot {
        bank: u8,
        trial: bool,
        confirmed: Option<Floor>,
    },
    /// The writer's turn applied a client store change.
    Keys(KeyResult),
    Update(update::Record),
}

impl Record {
    /// A panel event the loop logged.
    pub fn panel(&self) -> Option<panel::Event> {
        match self {
            Self::Feeder(feeder::Record::Owner(feeder::owner::Record::Panel(event))) => {
                Some(*event)
            }
            _ => None,
        }
    }
}

/// The record lines the feeder would log, and the simulator's own.
impl fmt::Display for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Feeder(record) => write!(f, "{} {record}", record.tag()),
            Self::Boot {
                bank,
                trial,
                confirmed,
            } => write!(
                f,
                "boot bank={bank} trial={} confirmed={confirmed:?}",
                u8::from(*trial)
            ),
            Self::Keys(result) => write!(f, "keys result={result}"),
            Self::Update(record) => {
                write!(f, "{} ", record.tag())?;
                update::describe(*record, f)
            }
        }
    }
}

pub struct Sim {
    feeder: Box<Feeder<SimPlatform>>,
    booted: bool,
    banks: Rc<RefCell<Banks>>,
    bank: u8,
    stage: Box<Update<Region>>,
    /// When a build that never confirms resets.
    trial_reset: Option<u32>,
    /// What the boot did, for the first pass's log.
    boot: Record,
}

impl fmt::Debug for Sim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sim")
            .field("platform", self.feeder.platform())
            .finish_non_exhaustive()
    }
}

impl Sim {
    /// The feeder as it boots from `flash` at `now_ms`, for the
    /// `boot_count`th time: the panel locked, the clock invalid, no sessions.
    /// With `mdns`, it advertises the device API there as the given address.
    pub fn open(
        flash: &Rc<Flash>,
        banks: &Rc<RefCell<Banks>>,
        update_key: Option<[u8; 32]>,
        random: Random,
        now_ms: u32,
        boot_count: u32,
        mdns: Option<(UdpSocket, Ipv4Addr)>,
    ) -> Result<Self, DeviceError> {
        let (Boot { bank, trial }, build) = boot(flash, &mut banks.borrow_mut(), update_key);
        // The mailbox and the interlock are RAM: a restart forgets every
        // request and chunk, and the write turn a give-way kept.
        mailbox::reset();
        wellfed_hal::interlock::INTERLOCK.reset();
        let store = Region::new(flash, storage::UPDATE);
        let mut stage = Box::new(Update::new(bank, build, update_key, store));
        let confirmed = (trial && banks.borrow().trial == Trial::Confirms)
            .then(|| confirm(flash, &mut stage, &build));
        let trial_reset = (trial && confirmed.is_none()).then(|| now_ms.wrapping_add(TRIAL_MS));

        let platform = SimPlatform::open(flash, random, boot_count, bank, build, mdns)?;
        Ok(Self {
            feeder: Box::new(Feeder::new(platform)),
            booted: false,
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

    /// The feeder's identity public key, as the client store holds it.
    pub fn identity(&self) -> Result<[u8; 32], DeviceError> {
        let secret = self
            .feeder
            .platform()
            .identity()
            .ok_or(DeviceError::NoIdentity)?;
        Ok(*Keypair::from_secret(*secret).public())
    }

    pub fn platform(&mut self) -> &mut SimPlatform {
        self.feeder.platform_mut()
    }

    /// Whether the feeder asked for a restart, taken once: the driver drops
    /// every connection and opens the feeder again from its flash.
    pub fn take_reboot(&mut self) -> bool {
        self.platform().take_reboot()
    }

    /// One gesture from the buttons, for the next pass: pairing takes it
    /// first (a code being keyed in, the recovery or setup hold), otherwise
    /// the panel.
    pub fn gesture(&mut self, gesture: Gesture) {
        self.platform().gesture_in(gesture);
    }

    /// Whether the serve loop should accept connections.
    pub(crate) fn listening(&self) -> bool {
        self.feeder.platform().listening()
    }

    /// A connection the serve loop accepted, for the next pass.
    pub(crate) fn accepted(&mut self, stream: TcpStream) {
        self.platform().accepted(stream);
    }

    /// The serve loop's accept failed.
    pub(crate) fn accept_failed(&mut self) {
        self.platform().accept_failed();
    }

    /// One pass of the loop at `now_ms`, then the writer's turn and the
    /// update stage's. The delay until the next pass.
    pub fn pass(&mut self, now_ms: u32, mut record: impl FnMut(Record)) -> u32 {
        // Every record line goes into the log tail too, as the firmware's
        // `log` copies what it prints.
        let tail = self.feeder.platform().log_tail();
        let mut record = |r: Record| {
            tail.push(r.to_string().as_bytes());
            record(r);
        };
        if !self.booted {
            self.booted = true;
            record(self.boot);
        }

        let delay = self
            .feeder
            .pass(now_ms, &mut NoFeeds, |r| record(Record::Feeder(r)));
        if let Some(result) = self.platform().write() {
            record(Record::Keys(result));
        }
        let updating = self.update(now_ms, &mut record);
        if updating { 1 } else { delay }
    }

    /// The update stage's turn, as the service task gives it; whether it
    /// has work waiting.
    fn update(&mut self, now_ms: u32, record: &mut impl FnMut(Record)) -> bool {
        if self
            .trial_reset
            .is_some_and(|at| wellfed_core::deadline_passed(now_ms, at))
        {
            self.platform().request_reboot();
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
            self.feeder.platform_mut().request_reboot();
        }
        step.delay_ms <= update::BUSY_MS
    }
}

/// The loader's choice and KM0's trial for this boot: the bank, whether its
/// build is in trial, and the build. A build bank 0 activated in bank 1 is
/// the one the update store names.
fn boot(flash: &Rc<Flash>, banks: &mut Banks, key: Option<[u8; 32]>) -> (Boot, [u8; 32]) {
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
    let booted = banks.boot(confirmed);
    (booted, banks.build(booted.bank))
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
