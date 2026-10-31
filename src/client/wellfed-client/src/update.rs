//! An update's client side, sans-IO: what to send the feeder next and what
//! its answers mean, for the owner's CLI, the bench and the app alike.
//!
//! An update crosses two boots of the feeder:
//! 1. On bank 1 the offer is checked and recorded, and the feeder gives way
//!    ([`UpdatePhase::GivingWay`]): it restarts into bank 0.
//! 2. The client reconnects (the same identity answers from bank 0) and
//!    offers the same manifest again; bank 0 installs only that one and
//!    starts receiving. A feeder already on bank 0 (fresh from stock, or
//!    after a failed trial) starts here.
//! 3. The client pushes the chunks each answer says are wanted. A dropped
//!    connection loses nothing: the install waits a minute for the next
//!    chunk, and [`Op::UpdateState`] says where it stands.
//! 4. Once every byte is verified the feeder writes bank 1's marker
//!    ([`UpdatePhase::Activated`]) and restarts into the new build, which
//!    confirms itself on its own.
//!
//! Chunks are idempotent by offset, so resending one is always safe. An
//! offer is not: a refused offer ends the install here, and the caller
//! decides what to do; nothing is retried behind its back.

use wellfed_model::{Answer, Op, Refusal, UpdatePhase, UpdateRefusal, UpdateStop, UpdateView};
use wellfed_proto::api::UPDATE_CHUNK_MAX;

/// What is offered: the signed manifest, the downgrade statement it may
/// need, and the image the manifest names (the bank's contents, without
/// the OTA container's header).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Image<'a> {
    pub manifest: &'a [u8],
    pub downgrade: Option<&'a [u8]>,
    pub payload: &'a [u8],
}

/// Which request an answer belongs to; the caller keeps it beside the
/// request's id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sent {
    Offer,
    Chunk(u32),
    State,
    Cancel,
}

/// What the caller does next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Next {
    /// Send this request under a fresh id.
    Send(Sent, Op),
    /// Wait for the answers in flight.
    Wait,
    /// The feeder restarts: once this session has ended, connect again and
    /// call [`Installer::opened`]. A session that still opens on bank 1 has
    /// not restarted yet, and this comes again; the caller bounds the wait.
    Reconnect,
    /// The image is installed and bank 1 activated; the feeder restarts
    /// into it, and its trial decides the rest.
    Activated,
    /// The install ended early, for good: nothing more is sent.
    Failed(InstallError),
    /// [`Installer::cancel`] went through: no candidate is left recorded and
    /// bank 1 is left as the install found it or invalid.
    Cancelled,
}

/// A refusal as an owner reads it on the app's stop screen and at the CLI.
/// The rarer ones keep their names, which a bug report can quote.
fn refused_words(refusal: &Refusal) -> String {
    let words = match refusal {
        Refusal::Update(UpdateRefusal::Key) => {
            "it does not trust the key that signed this firmware"
        }
        Refusal::Update(UpdateRefusal::Revoked) => "the key that signed this firmware was retired",
        Refusal::Update(UpdateRefusal::Signature) => "this firmware's signature does not check out",
        Refusal::Update(UpdateRefusal::Downgrade) => {
            "this version is not newer than what it has run"
        }
        Refusal::Update(UpdateRefusal::Failed) => "this firmware already failed on it",
        Refusal::Update(UpdateRefusal::Installing) => "another update is in progress",
        Refusal::Update(UpdateRefusal::Unconfirmed) => "nobody pressed Dispense in time",
        other => return format!("{other:?}"),
    };
    words.to_owned()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    #[error("the feeder refused the update: {}", refused_words(.0))]
    Refused(Refusal),
    #[error("the install stopped before activation: {0:?}")]
    Stopped(UpdateStop),
    #[error("bank 0 is not installing this update")]
    Lost,
    #[error("the feeder answered with something other than the update's state")]
    Unexpected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// Connected: offer next.
    Offer,
    /// The offer is out.
    Offered,
    /// Bank 1 gives way; reconnect.
    GivingWay,
    /// After a reconnect mid-install: read the state first.
    Resume,
    /// Pushing chunks.
    Receiving,
    /// Bank 0 is installing something already: read what, and resume it if
    /// it is this offer.
    Adopt,
    /// A cancel is to go, then is out.
    Cancel,
    Cancelling,
    Cancelled,
    Activated,
    Failed(InstallError),
}

/// Chunks the client keeps in flight at once: a sector's worth, which
/// the feeder's mailbox and the TCP window absorb.
pub const IN_FLIGHT: usize = 8;

pub struct Installer<'a> {
    image: Image<'a>,
    stage: Stage,
    view: Option<UpdateView>,
    /// Chunk offsets sent and not yet answered.
    in_flight: Vec<u32>,
    /// A state read is out: no chunk goes before its answer.
    reading: bool,
    /// The last state read wanted nothing (a sector being written): wait
    /// once before reading again.
    settling: bool,
    /// The bank the session answers from.
    bank: Option<u8>,
    /// The offered manifest's digest, as the feeder's state names it.
    digest: [u8; 32],
    /// The state read out is to tell whether bank 0 installs this offer.
    adopting: bool,
}

impl<'a> Installer<'a> {
    pub fn new(image: Image<'a>) -> Self {
        Self {
            image,
            stage: Stage::Offer,
            view: None,
            in_flight: Vec::new(),
            reading: false,
            settling: false,
            bank: None,
            digest: wellfed_core::ota::offer::manifest_digest(image.manifest),
            adopting: false,
        }
    }

    /// Stop the install and forget its candidate: the next poll sends the
    /// cancel, whatever is in flight. A recorded candidate that stays would
    /// make bank 0 refuse every other manifest.
    ///
    /// Once bank 1 is activated there is nothing left to cancel (the new
    /// build's trial decides), so this does nothing then.
    pub fn cancel(&mut self) {
        if !matches!(
            self.stage,
            Stage::Cancelling | Stage::Cancelled | Stage::Activated
        ) {
            self.stage = Stage::Cancel;
        }
    }

    /// The feeder's latest state, once one came.
    pub fn view(&self) -> Option<&UpdateView> {
        self.view.as_ref()
    }

    /// Why the install ended early, if it did.
    pub fn failed(&self) -> Option<InstallError> {
        match self.stage {
            Stage::Failed(error) => Some(error),
            _ => None,
        }
    }

    /// A session opened on `bank`. Bank 1 is offered to; bank 0 is offered
    /// to first, and read for its state once it is receiving.
    pub fn opened(&mut self, bank: u8) {
        self.bank = Some(bank);
        self.in_flight.clear();
        self.reading = false;
        self.adopting = false;
        self.stage = match self.stage {
            // A cancel goes to whichever bank answers.
            Stage::Cancel | Stage::Cancelling => Stage::Cancel,
            Stage::Cancelled => Stage::Cancelled,
            // Bank 1 has not restarted yet: wait for the session to end.
            Stage::GivingWay if bank == 1 => Stage::GivingWay,
            Stage::Receiving | Stage::Resume if bank == 0 => Stage::Resume,
            Stage::Failed(error) => Stage::Failed(error),
            Stage::Activated => Stage::Activated,
            _ => Stage::Offer,
        };
    }

    /// The session ended: whatever is in flight is unanswered, and the
    /// next session resumes.
    pub fn lost(&mut self) {
        self.in_flight.clear();
        self.reading = false;
        self.adopting = false;
        match self.stage {
            Stage::Offered | Stage::Adopt => self.stage = Stage::Offer,
            Stage::Cancelling => self.stage = Stage::Cancel,
            _ => {}
        }
    }

    /// The next thing to do.
    pub fn poll(&mut self) -> Next {
        match self.stage {
            Stage::Offer => {
                self.stage = Stage::Offered;
                Next::Send(
                    Sent::Offer,
                    Op::OfferUpdate {
                        manifest: self.image.manifest.to_vec(),
                        downgrade: self.image.downgrade.map(<[u8]>::to_vec),
                    },
                )
            }
            Stage::Resume => {
                self.stage = Stage::Receiving;
                self.reading = true;
                Next::Send(Sent::State, Op::UpdateState)
            }
            Stage::Adopt => {
                self.stage = Stage::Receiving;
                self.reading = true;
                self.adopting = true;
                Next::Send(Sent::State, Op::UpdateState)
            }
            Stage::Cancel => {
                self.stage = Stage::Cancelling;
                Next::Send(Sent::Cancel, Op::CancelUpdate)
            }
            Stage::Cancelling => Next::Wait,
            Stage::Cancelled => Next::Cancelled,
            Stage::Offered => Next::Wait,
            Stage::GivingWay => Next::Reconnect,
            Stage::Activated => Next::Activated,
            Stage::Failed(error) => Next::Failed(error),
            Stage::Receiving => self.next_chunk(),
        }
    }

    fn next_chunk(&mut self) -> Next {
        if self.reading || self.in_flight.len() >= IN_FLIGHT {
            return Next::Wait;
        }
        let wanted = self.view.as_ref().map_or(&[][..], |view| &view.wanted[..]);
        let Some(&offset) = wanted
            .iter()
            .find(|offset| !self.in_flight.contains(offset))
        else {
            // Every chunk the last answer named is in: the stage is writing
            // the sector, so ask again what it wants once nothing is out.
            if !self.in_flight.is_empty() || core::mem::take(&mut self.settling) {
                return Next::Wait;
            }
            self.reading = true;
            return Next::Send(Sent::State, Op::UpdateState);
        };
        let Some(data) = self.chunk(offset) else {
            self.stage = Stage::Failed(InstallError::Lost);
            return Next::Wait;
        };
        self.in_flight.push(offset);
        Next::Send(
            Sent::Chunk(offset),
            Op::UpdateChunk {
                offset,
                data: data.to_vec(),
            },
        )
    }

    /// The image bytes at `offset`, as the feeder asks for them.
    fn chunk(&self, offset: u32) -> Option<&'a [u8]> {
        let start = usize::try_from(offset).ok()?;
        let end = start
            .saturating_add(UPDATE_CHUNK_MAX)
            .min(self.image.payload.len());
        self.image.payload.get(start..end).filter(|d| !d.is_empty())
    }

    /// The feeder's answer to `sent`.
    pub fn answer(&mut self, sent: Sent, answer: &Answer) -> Result<(), InstallError> {
        if let Stage::Failed(error) = self.stage {
            return Err(error);
        }
        match sent {
            Sent::Chunk(offset) => self.in_flight.retain(|&o| o != offset),
            Sent::State => self.reading = false,
            Sent::Offer | Sent::Cancel => {}
        }
        if sent == Sent::Cancel {
            return match answer {
                Answer::Update(view) => {
                    self.view = Some(view.clone());
                    self.stage = Stage::Cancelled;
                    Ok(())
                }
                Answer::Refused(refusal) => Err(self.fail(InstallError::Refused(*refusal))),
                _ => Err(self.fail(InstallError::Unexpected)),
            };
        }
        // Whatever comes for the install after a cancel is moot.
        if matches!(
            self.stage,
            Stage::Cancel | Stage::Cancelling | Stage::Cancelled
        ) {
            return Ok(());
        }

        let view = match answer {
            Answer::Update(view) => view,
            // A full mailbox: the chunk goes again when it is still wanted.
            Answer::Refused(Refusal::Busy) if matches!(sent, Sent::Chunk(_)) => return Ok(()),
            // The install ended under the chunk: the state says why.
            Answer::Refused(Refusal::Update(UpdateRefusal::NotInstalling))
                if matches!(sent, Sent::Chunk(_)) =>
            {
                if !self.reading {
                    self.stage = Stage::Resume;
                }
                return Ok(());
            }
            Answer::Refused(Refusal::Update(UpdateRefusal::NotInstalling))
                if sent == Sent::State =>
            {
                return Err(self.fail(InstallError::Lost));
            }
            // A re-offer after a lost answer finds its own install running.
            Answer::Refused(Refusal::Update(UpdateRefusal::Installing))
                if sent == Sent::Offer && self.bank == Some(0) =>
            {
                self.stage = Stage::Adopt;
                return Ok(());
            }
            Answer::Refused(refusal) => return Err(self.fail(InstallError::Refused(*refusal))),
            _ => return Err(self.fail(InstallError::Unexpected)),
        };
        if sent == Sent::State && core::mem::take(&mut self.adopting) {
            let ours = view.digest.map(|digest| digest.0) == Some(self.digest);
            if !(ours && view.phase == UpdatePhase::Receiving) {
                let installing = Refusal::Update(UpdateRefusal::Installing);
                return Err(self.fail(InstallError::Refused(installing)));
            }
        }
        let (phase, stopped) = (view.phase, view.stopped);
        self.settling = sent == Sent::State && view.wanted.is_empty();
        self.view = Some(view.clone());

        match phase {
            UpdatePhase::Activated => self.stage = Stage::Activated,
            UpdatePhase::GivingWay => self.stage = Stage::GivingWay,
            UpdatePhase::Receiving => self.stage = Stage::Receiving,
            // Still being checked: ask again.
            UpdatePhase::Checking => self.stage = Stage::Resume,
            // Bank 0 restarted mid-install (a power cut) and lost it: the
            // offer goes again and the install starts over. A stop it
            // reports ended the install for good.
            UpdatePhase::Idle
                if stopped.is_none() && sent == Sent::State && self.bank == Some(0) =>
            {
                self.stage = Stage::Offer;
            }
            UpdatePhase::Idle => {
                let error = stopped.map_or(InstallError::Lost, InstallError::Stopped);
                return Err(self.fail(error));
            }
        }
        Ok(())
    }

    fn fail(&mut self, error: InstallError) -> InstallError {
        self.stage = Stage::Failed(error);
        error
    }
}
