//! Bank 0's install of one admitted offer, sans-IO: which chunks it wants,
//! what each chunk does, and when it stops.
//!
//! The client pushes chunks over the device API, so a chunk may come twice
//! (a retry after a lost answer, or a state read just before the chunk
//! landed). A chunk the install already holds is answered and dropped; a
//! chunk it does not want, or one of the wrong length, stops the install,
//! as an out-of-order answer did when the updater pulled. Every other rule
//! is the [`Writer`]'s: sectors in order, each read back, the whole image's
//! SHA-256 checked, and the marker handed out last.
//!
//! The port runs each [`Program`] inside a flash lease and reports its
//! readback; on [`Install::ready`] it records the activation in the update
//! store, then programs the marker and reads it back.

use super::offer::{Admitted, KeyStatement, OfferError, StatementError};
use super::{MAX_CHUNK, Program, SECTOR_BYTES, WriteError, Writer};
use crate::store::update::UpdateError;

/// A minute without a chunk ends the install with bank 1 invalid.
pub const IDLE_MS: u32 = 60_000;
/// Chunks wanted at once: one sector's.
pub const WANTED: usize = (SECTOR_BYTES / MAX_CHUNK) as usize;

/// Why an install stopped before activation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    Timeout,
    Cancelled,
    /// A chunk the install did not want, or of the wrong length.
    Order,
    /// The image does not start with the loader's marker.
    Marker,
    /// A sector did not read back.
    Write,
    /// The whole image's SHA-256 differs from the manifest's.
    Hash,
    /// The marker did not read back.
    Activate,
    /// The activation could not be recorded.
    Store,
}

crate::named_enum! {
    /// The `stage=` word of an install's stop record (`PLOTA1 error`). Not
    /// one word per [`Stop`]: the words predate the variants, and the bench
    /// matches the words.
    pub enum StopStage {
        Timeout = "timeout",
        Cancelled = "cancelled",
        Order = "order",
        /// [`Stop::Marker`]: the image lacks the loader's marker.
        Image = "image",
        Write = "write",
        Hash = "hash",
        /// [`Stop::Activate`]: the marker did not read back.
        Marker = "marker",
        Store = "store",
    }
}

impl Stop {
    pub const fn stage(self) -> StopStage {
        match self {
            Self::Timeout => StopStage::Timeout,
            Self::Cancelled => StopStage::Cancelled,
            Self::Order => StopStage::Order,
            Self::Marker => StopStage::Image,
            Self::Write => StopStage::Write,
            Self::Hash => StopStage::Hash,
            Self::Activate => StopStage::Marker,
            Self::Store => StopStage::Store,
        }
    }

    fn of(error: WriteError) -> Self {
        match error {
            WriteError::Order | WriteError::State => Self::Order,
            WriteError::Marker => Self::Marker,
            WriteError::Readback => Self::Write,
            WriteError::Hash => Self::Hash,
        }
    }
}

/// Where an update stands, as the feeder publishes it to its clients.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    /// An offer's signatures or bank 0's image are being checked.
    Checking,
    /// Bank 1 recorded the offer and restarts into bank 0.
    GivingWay,
    Receiving,
    /// Bank 1's marker is written; the feeder restarts into it.
    Activated,
}

/// The update's state, published after every step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub phase: Phase,
    pub bank: u8,
    pub min_version: u32,
    pub digest: Option<[u8; 32]>,
    pub version: u32,
    pub image_len: u32,
    pub written: u32,
    pub wanted: [Option<u32>; WANTED],
    /// Why the last install stopped, if it did.
    pub stopped: Option<Stop>,
}

impl Progress {
    pub const fn idle(bank: u8, min_version: u32) -> Self {
        Self {
            phase: Phase::Idle,
            bank,
            min_version,
            digest: None,
            version: 0,
            image_len: 0,
            written: 0,
            wanted: [None; WANTED],
            stopped: None,
        }
    }

    /// The same, naming the manifest `admitted`.
    pub const fn of(self, phase: Phase, admitted: &Admitted) -> Self {
        Self {
            phase,
            digest: Some(admitted.digest),
            version: admitted.signed.manifest.version,
            image_len: admitted.signed.manifest.image_len,
            ..self
        }
    }
}

/// Why an update request was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    Offer(OfferError),
    Statement(StatementError),
    Store(UpdateError),
    /// The store cannot be read: no floor to check against.
    Unreadable,
    /// The mailbox or the flash is not free yet.
    Busy,
    /// Bank 0 does not hold the recorded image of a build that installs.
    Fallback,
    /// Bank 0 installs only the manifest bank 1 gave way for.
    Pending,
    /// This manifest was activated and failed its trial.
    Failed,
    /// An install reset the feeder this power cycle.
    Attempt,
    /// Another update is in progress.
    Installing,
    NotInstalling,
}

crate::named_enum! {
    /// The `reason=` word of a refusal record (`PLOTA1 refused`). Several
    /// refusals share a word: the bench tells them apart by what it sent.
    pub enum RefusedReason {
        Downgrade = "downgrade",
        Same = "same",
        Key = "key",
        Signature = "signature",
        Statement = "statement",
        Manifest = "manifest",
        Store = "store",
        Busy = "busy",
        Fallback = "fallback",
        Pending = "pending",
        Failed = "failed",
        Attempt = "attempt",
        Installing = "installing",
        NotInstalling = "not-installing",
    }
}

impl Refused {
    pub const fn reason(self) -> RefusedReason {
        match self {
            Self::Offer(OfferError::Downgrade) => RefusedReason::Downgrade,
            Self::Offer(OfferError::Same) => RefusedReason::Same,
            Self::Offer(OfferError::Key(_)) | Self::Statement(_) => RefusedReason::Key,
            Self::Offer(OfferError::Signature) => RefusedReason::Signature,
            Self::Offer(OfferError::Statement(_)) => RefusedReason::Statement,
            Self::Offer(_) => RefusedReason::Manifest,
            Self::Store(_) | Self::Unreadable => RefusedReason::Store,
            Self::Busy => RefusedReason::Busy,
            Self::Fallback => RefusedReason::Fallback,
            Self::Pending => RefusedReason::Pending,
            Self::Failed => RefusedReason::Failed,
            Self::Attempt => RefusedReason::Attempt,
            Self::Installing => RefusedReason::Installing,
            Self::NotInstalling => RefusedReason::NotInstalling,
        }
    }
}

/// What an update request came to, as the update stage answers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Progress(Progress),
    /// A key statement took effect.
    Key(KeyStatement),
}

/// What one chunk did.
#[derive(Debug, PartialEq, Eq)]
pub enum Taken<'a> {
    /// Staged; more of the sector is wanted.
    Staged,
    /// Already held: answered, changed nothing.
    Held,
    /// The sector is complete: program it, then call [`Install::programmed`].
    Program(Program<'a>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Receiving,
    /// Every sector is written and the hash matched: the marker is next.
    Ready,
    Activated,
    Stopped(Stop),
}

pub struct Install {
    admitted: Admitted,
    writer: Writer,
    phase: State,
    /// When the last chunk came (or the install started).
    heard_ms: u32,
}

impl Install {
    /// Start installing `admitted` at `now_ms`. `None` only if the writer
    /// cannot take [`MAX_CHUNK`] chunks, which a constant rules out.
    pub fn new(admitted: Admitted, now_ms: u32) -> Option<Self> {
        Some(Self {
            writer: Writer::new(admitted.signed.manifest, MAX_CHUNK as usize)?,
            admitted,
            phase: State::Receiving,
            heard_ms: now_ms,
        })
    }

    pub fn admitted(&self) -> &Admitted {
        &self.admitted
    }

    /// Image bytes written and read back.
    pub fn written(&self) -> u32 {
        self.writer.written()
    }

    /// Why the install stopped: the port's reason, or the writer's.
    pub fn stopped(&self) -> Option<Stop> {
        match self.phase {
            State::Stopped(stop) => Some(stop),
            _ => self.writer.failed().map(Stop::of),
        }
    }

    /// Every byte is written and verified: record the activation, then
    /// program the marker.
    pub fn ready(&self) -> bool {
        self.phase == State::Ready
    }

    pub fn activated(&self) -> bool {
        self.phase == State::Activated
    }

    /// Still taking chunks.
    pub fn receiving(&self) -> bool {
        self.phase == State::Receiving && self.writer.failed().is_none()
    }

    /// The state, as published.
    pub fn progress(&self, bank: u8, min_version: u32) -> Progress {
        let phase = if self.activated() {
            Phase::Activated
        } else if self.stopped().is_some() {
            Phase::Idle
        } else {
            Phase::Receiving
        };
        Progress {
            written: self.written(),
            wanted: self.wanted(),
            stopped: self.stopped(),
            ..Progress::idle(bank, min_version).of(phase, &self.admitted)
        }
    }

    /// The chunk offsets wanted now, in order.
    pub fn wanted(&self) -> [Option<u32>; WANTED] {
        let mut out = [None; WANTED];
        if self.receiving() {
            for (slot, (offset, _)) in out.iter_mut().zip(self.writer.missing()) {
                *slot = Some(offset);
            }
        }
        out
    }

    fn stop(&mut self, stop: Stop) -> Stop {
        if let Some(earlier) = self.stopped() {
            return earlier;
        }
        if self.phase != State::Activated {
            self.phase = State::Stopped(stop);
        }
        stop
    }

    /// Stop now: cancelled by the owner, or the store failed. Bank 1's
    /// marker is never written after this.
    pub fn abort(&mut self, stop: Stop) {
        self.stop(stop);
    }

    /// End a quiet install once [`IDLE_MS`] passed without a chunk.
    pub fn tick(&mut self, now_ms: u32) -> Option<Stop> {
        let quiet = now_ms.wrapping_sub(self.heard_ms) >= IDLE_MS;
        (self.receiving() && quiet).then(|| self.stop(Stop::Timeout))
    }

    /// One chunk at `offset`.
    pub fn chunk(&mut self, offset: u32, data: &[u8], now_ms: u32) -> Result<Taken<'_>, Stop> {
        if let Some(stop) = self.stopped() {
            return Err(stop);
        }
        self.heard_ms = now_ms;
        if self.phase != State::Receiving || self.writer.has(offset) {
            return Ok(Taken::Held);
        }

        // The writer keeps its own failure, which `stopped` reports.
        match self.writer.accept(offset, data).map_err(Stop::of)? {
            None => Ok(Taken::Staged),
            Some(program) => Ok(Taken::Program(program)),
        }
    }

    /// The staged sector's readback. Once the last one passes the whole
    /// image's hash, the install is [`Self::ready`].
    pub fn programmed(&mut self, readback_ok: bool) -> Result<(), Stop> {
        if let Err(error) = self.writer.programmed(readback_ok) {
            return Err(self.stop(Stop::of(error)));
        }
        if self.writer.next_request().is_none() {
            self.phase = State::Ready;
        }
        Ok(())
    }

    /// The marker to program last, as a bank-1 offset, once the install is
    /// ready and its activation recorded.
    pub fn commit(&mut self) -> Result<(u32, [u8; 8]), Stop> {
        if self.phase != State::Ready {
            return Err(self.stop(Stop::Order));
        }
        self.writer
            .commit()
            .map_err(|error| self.stop(Stop::of(error)))
    }

    /// The marker read back: bank 1 is valid.
    pub fn marked(&mut self, readback_ok: bool) -> Result<(), Stop> {
        if self.phase != State::Ready {
            return Err(self.stop(Stop::Order));
        }
        if !readback_ok {
            return Err(self.stop(Stop::Activate));
        }
        self.phase = State::Activated;
        Ok(())
    }
}
