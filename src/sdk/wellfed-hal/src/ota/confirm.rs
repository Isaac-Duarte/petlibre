//! Confirmation on KM4, normal role (booted from bank 1).
//!
//! KM0 counts this image's boots until a record in storage sector
//! `trial::CONFIRM_SECTOR` names its build identity (`trial.rs`). Once the
//! image has stayed healthy for `HEALTHY_MS` (the glue's test: the network
//! check passed, outputs verified stopped, startup probes finished), this
//! arms the flash-lock hooks like the updater (`arming.rs`). The firmware's
//! feeding owner opens on its journal only once the hooks are armed, so the
//! record waits after arming until the owner has proved it can feed
//! ([`Feeding::proven`]: opened, at rest with its motor, sensor and current
//! checks passing), with the network still healthy. A build whose owner
//! cannot open never confirms: the trial deadline, or the supervisor's stop
//! on an owner that failed to open, resets it, and KM0 counts the reset.
//! A firmware with no feeding owner has nothing to prove.
//!
//! Then this raises the update store's floor to its version if bank 0
//! activated it (`ota::update`), erases the sector, programs the record and
//! reads it back. Arming and the write each take the write turn
//! (`interlock`), so neither runs while a feed may move the outputs, and
//! each sets the attempt marker only for itself: the wait for the owner
//! between them leaves it clear, so a reset in that wait never makes the
//! next boot skip arming. The floor goes first: the confirmation sector is
//! erased before each write, and a store write that fails leaves the image
//! unconfirmed rather than confirmed with the floor behind it. An image
//! already confirmed does nothing.
//!
//! On a trial boot no feed starts before this write: the owner opens only
//! once this has armed the hooks (the settings writer starts only after the
//! confirmation), and the firmware refuses feeds while the build is
//! unconfirmed (`wellfed_sdk::confirming`). So a feed never delays the
//! confirmation towards the trial deadline. A torn or failed write leaves
//! the image unconfirmed, which only keeps KM0 counting; the next boot
//! finds the floor already raised and writes the record again.

use core::fmt;

use wellfed_core::named_enum;

use wellfed_core::ota::{self, CONFIRM_LEN};

use crate::flash::park::Mmio;
use crate::flash_lock::arming::{self, Arming, Hooks, Poll};
use crate::flash_lock::provider::Provider;
use crate::interlock::{INTERLOCK, WRITER_POLL_MS, WriteStep, WriteTurn};
use crate::ota::update::Floor;
use crate::sdk;
use crate::supervision::supervise::Feeding;
use crate::trial::{self, CONFIRM_SECTOR};

pub const POLL_MS: u32 = 500;
const _: () = assert!(POLL_MS <= WRITER_POLL_MS);
/// Continuous health before confirming.
pub const HEALTHY_MS: u32 = 30_000;

/// The storage region by sector, as `sdk::storage_*` take it.
pub trait StorageFlash {
    fn erase(&mut self, sector: u32) -> bool;
    fn program(&mut self, sector: u32, offset: u32, data: &[u8]) -> bool;
    fn read(&mut self, sector: u32, offset: u32, bytes: &mut [u8]) -> bool;
}

/// Storage through the SDK wrappers, refused unless the hooks are armed.
pub struct HookedStorage<H>(core::marker::PhantomData<H>);

impl<H> HookedStorage<H> {
    pub const fn new() -> Self {
        Self(core::marker::PhantomData)
    }
}

impl<H> Default for HookedStorage<H> {
    fn default() -> Self {
        Self::new()
    }
}

impl<H: Hooks> StorageFlash for HookedStorage<H> {
    fn erase(&mut self, sector: u32) -> bool {
        // SAFETY: the hooks are armed; the confirmation owns its sector.
        H::armed() && unsafe { sdk::storage_erase(sector) }
    }
    fn program(&mut self, sector: u32, offset: u32, data: &[u8]) -> bool {
        // SAFETY: as in `erase`.
        H::armed() && unsafe { sdk::storage_program(sector, offset, data) }
    }
    fn read(&mut self, sector: u32, offset: u32, bytes: &mut [u8]) -> bool {
        sdk::storage_read(sector, offset, bytes)
    }
}

named_enum! {
    /// Where the confirmation failed (`error stage=`).
    pub enum ErrorStage {
        Read = "read",
        Write = "write",
        /// The update store's floor could not be raised.
        Floor = "floor",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    /// The record already names this build.
    Already,
    Arming(arming::Event),
    Confirmed {
        healthy_ms: u32,
        /// The version bank 0 activated this build as; 0 for a build
        /// installed another way.
        version: u32,
    },
    Error {
        stage: ErrorStage,
    },
}

/// The body of a `PLCNF1` line.
pub fn describe(record: Record, f: &mut impl fmt::Write) -> fmt::Result {
    match record {
        Record::Already => f.write_str("confirmed=1 already=1"),
        Record::Arming(arming::Event::Ready { reply, inactive }) => {
            write!(
                f,
                "ready reply={} inactive={}",
                reply.code(),
                u8::from(inactive)
            )
        }
        Record::Arming(arming::Event::Timeout) => f.write_str("error stage=timeout"),
        Record::Arming(arming::Event::Skip { marker }) => write!(f, "skip marker={marker:#x}"),
        Record::Arming(arming::Event::Armed {
            watchdog,
            installed,
        }) => write!(
            f,
            "armed watchdog={} installed={}",
            u8::from(watchdog),
            u8::from(installed)
        ),
        Record::Arming(arming::Event::Clock) => f.write_str("error stage=clock"),
        Record::Confirmed {
            healthy_ms,
            version,
        } => write!(f, "confirmed=1 healthy_ms={healthy_ms} version={version}"),
        Record::Error { stage } => write!(f, "error stage={stage}"),
    }
}

/// `delay_ms == 0` ends the task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub delay_ms: u32,
    pub record: Option<Record>,
}

fn next(delay_ms: u32, record: Option<Record>) -> Step {
    Step { delay_ms, record }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Stage {
    #[default]
    Check,
    Healthy,
    Arm,
    /// Armed: the feeding owner may open; wait for its proof.
    Proving,
    Write,
    Done,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Confirm {
    stage: Stage,
    arming: Arming,
    healthy_since: Option<u32>,
    healthy_ms: u32,
    /// The write's wait for the write turn.
    turn: WriteTurn,
    /// The write turn is held, from before arming to the write's end.
    holding: bool,
}

impl Confirm {
    /// The write turn, unless already held; false while a feed holds the
    /// outputs.
    fn take_turn(&mut self, now: u32) -> bool {
        if !self.holding {
            let stopped = || crate::stopped::observe().verified();
            self.holding = self.turn.begin(&INTERLOCK, now, stopped) != WriteStep::Wait;
        }
        self.holding
    }

    fn give_turn(&mut self) {
        if core::mem::take(&mut self.holding) {
            INTERLOCK.end_write();
        }
    }

    fn end(&mut self, record: Record) -> Step {
        self.stage = Stage::Done;
        next(0, Some(record))
    }

    /// One task iteration. `healthy` is the glue's current health test,
    /// `feeding` what the firmware's feeding owner has shown, `now` uptime
    /// in milliseconds, and `raise` raises the update store's floor (the
    /// update stage's `raise_floor`) just before the record.
    ///
    /// # Safety
    /// `provider` is valid for the program's lifetime, stays at a fixed
    /// address and is not otherwise borrowed.
    pub unsafe fn step<M: Mmio, H: Hooks>(
        &mut self,
        provider: *mut Provider,
        flash: &mut impl StorageFlash,
        raise: impl FnOnce() -> Floor,
        healthy: bool,
        feeding: Feeding,
        now: u32,
    ) -> Step {
        match self.stage {
            Stage::Check => {
                let mut record = [0u8; CONFIRM_LEN];
                if !flash.read(CONFIRM_SECTOR, 0, &mut record) {
                    return self.end(Record::Error {
                        stage: ErrorStage::Read,
                    });
                }
                if ota::confirmed(&record, &trial::identity()).is_some() {
                    return self.end(Record::Already);
                }
                self.stage = Stage::Healthy;
                next(POLL_MS, None)
            }

            Stage::Healthy => {
                if !healthy {
                    self.healthy_since = None;
                    return next(POLL_MS, None);
                }

                let since = *self.healthy_since.get_or_insert(now);
                self.healthy_ms = now.wrapping_sub(since);
                if self.healthy_ms >= HEALTHY_MS {
                    self.stage = Stage::Arm;
                }
                next(POLL_MS, None)
            }

            Stage::Arm => {
                // The turn before arming: arming sets the attempt marker,
                // which must not stay set through a wait for a feed.
                if !self.take_turn(now) {
                    return next(POLL_MS, None);
                }
                // SAFETY: the caller's contract.
                match unsafe { self.arming.step::<M, H>(provider) } {
                    Poll::Pending(event) => next(arming::POLL_MS, event.map(Record::Arming)),
                    Poll::Armed(event) => {
                        // Armed for the rest of the boot; the write marks
                        // its own attempt, as every store write does.
                        arming::clear_attempt();
                        self.give_turn();
                        self.stage = Stage::Proving;
                        next(arming::POLL_MS, Some(Record::Arming(event)))
                    }
                    Poll::Failed(event) => {
                        self.give_turn();
                        self.end(Record::Arming(event))
                    }
                }
            }

            // The network must still be healthy when the owner has proved
            // itself: the record vouches for both.
            Stage::Proving => {
                if healthy && feeding.proven() {
                    self.stage = Stage::Write;
                    return next(arming::POLL_MS, None);
                }
                next(POLL_MS, None)
            }

            Stage::Write => {
                if !self.take_turn(now) {
                    return next(POLL_MS, None);
                }
                arming::set_attempt();
                let version = match raise() {
                    Floor::Raised { version } => version,
                    Floor::Unchanged => 0,
                    Floor::Failed => {
                        arming::clear_attempt();
                        self.give_turn();
                        return self.end(Record::Error {
                            stage: ErrorStage::Floor,
                        });
                    }
                };
                let record = ota::confirm_record(&trial::identity(), version);
                let mut readback = [0u8; CONFIRM_LEN];
                let ok = flash.erase(CONFIRM_SECTOR)
                    && flash.program(CONFIRM_SECTOR, 0, &record)
                    && flash.read(CONFIRM_SECTOR, 0, &mut readback)
                    && readback == record;

                // The attempt ended in order either way, so a later boot must not skip.
                arming::clear_attempt();
                self.give_turn();
                if !ok {
                    return self.end(Record::Error {
                        stage: ErrorStage::Write,
                    });
                }
                self.end(Record::Confirmed {
                    healthy_ms: self.healthy_ms,
                    version,
                })
            }

            Stage::Done => next(0, None),
        }
    }
}
