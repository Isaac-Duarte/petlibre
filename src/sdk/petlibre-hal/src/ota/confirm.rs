//! FW11 confirmation on KM4, normal role (booted from bank 1).
//!
//! KM0 counts this image's boots until a record in storage sector
//! `trial::CONFIRM_SECTOR` names its build identity (`trial.rs`). Once the
//! image has stayed healthy for `HEALTHY_MS` (the glue's test: transport
//! ready, outputs verified stopped, startup probes finished), this arms the
//! flash-lock hooks like the updater (`arming.rs`), erases the sector,
//! programs the record and reads it back. An image already confirmed does
//! nothing. A torn or failed write leaves the image unconfirmed, which only
//! keeps KM0 counting.

use core::fmt;

use petlibre_core::named_enum;

use petlibre_core::ota::{self, CONFIRM_LEN};

use crate::flash::park::Mmio;
use crate::flash_lock::arming::{self, Arming, Hooks, Poll};
use crate::flash_lock::provider::Provider;
use crate::sdk;
use crate::trial::{self, CONFIRM_SECTOR};

pub const POLL_MS: u32 = 500;
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
        H::armed() && sdk::storage_erase(sector)
    }
    fn program(&mut self, sector: u32, offset: u32, data: &[u8]) -> bool {
        H::armed() && sdk::storage_program(sector, offset, data)
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
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    /// The record already names this build.
    Already,
    Arming(arming::Event),
    Confirmed {
        healthy_ms: u32,
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
            write!(f, "ready reply={} inactive={}", reply.0, u8::from(inactive))
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
        Record::Confirmed { healthy_ms } => write!(f, "confirmed=1 healthy_ms={healthy_ms}"),
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
    Write,
    Done,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Confirm {
    stage: Stage,
    arming: Arming,
    healthy_since: Option<u32>,
    healthy_ms: u32,
}

impl Confirm {
    fn end(&mut self, record: Record) -> Step {
        self.stage = Stage::Done;
        next(0, Some(record))
    }

    /// One task iteration. `healthy` is the glue's current health test and
    /// `now` uptime in milliseconds.
    ///
    /// # Safety
    /// `provider` is valid for the program's lifetime, stays at a fixed
    /// address and is not otherwise borrowed.
    pub unsafe fn step<M: Mmio, H: Hooks>(
        &mut self,
        provider: *mut Provider,
        flash: &mut impl StorageFlash,
        healthy: bool,
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

            // SAFETY: the caller's contract.
            Stage::Arm => match unsafe { self.arming.step::<M, H>(provider) } {
                Poll::Pending(event) => next(arming::POLL_MS, event.map(Record::Arming)),
                Poll::Armed(event) => {
                    self.stage = Stage::Write;
                    next(arming::POLL_MS, Some(Record::Arming(event)))
                }
                Poll::Failed(event) => self.end(Record::Arming(event)),
            },

            Stage::Write => {
                let record = ota::confirm_record(&trial::identity(), 0);
                let mut readback = [0u8; CONFIRM_LEN];
                let ok = flash.erase(CONFIRM_SECTOR)
                    && flash.program(CONFIRM_SECTOR, 0, &record)
                    && flash.read(CONFIRM_SECTOR, 0, &mut readback)
                    && readback == record;

                // The attempt ended in order either way, so a later boot must not skip.
                arming::clear_attempt();
                if !ok {
                    return self.end(Record::Error {
                        stage: ErrorStage::Write,
                    });
                }
                self.end(Record::Confirmed {
                    healthy_ms: self.healthy_ms,
                })
            }

            Stage::Done => next(0, None),
        }
    }
}
