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
}

/// What the caller does next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Next {
    /// Send this request under a fresh id.
    Send(Sent, Op),
    /// Wait for the answers in flight.
    Wait,
    /// The feeder restarts: connect again, then call [`Installer::opened`].
    Reconnect,
    /// The image is installed and bank 1 activated; the feeder restarts
    /// into it, and its trial decides the rest.
    Activated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    #[error("the feeder refused the update: {0:?}")]
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
    /// The bank the session answers from.
    bank: Option<u8>,
}

impl<'a> Installer<'a> {
    pub fn new(image: Image<'a>) -> Self {
        Self {
            image,
            stage: Stage::Offer,
            view: None,
            in_flight: Vec::new(),
            reading: false,
            bank: None,
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
        self.stage = match self.stage {
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
        if self.stage == Stage::Offered {
            self.stage = Stage::Offer;
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
            Stage::Offered => Next::Wait,
            Stage::GivingWay => Next::Reconnect,
            Stage::Activated => Next::Activated,
            Stage::Failed(_) => Next::Wait,
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
            return Next::Wait;
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
            Sent::Offer => {}
        }

        let view = match answer {
            Answer::Update(view) => view,
            // A full mailbox: the chunk goes again when it is still wanted.
            Answer::Refused(Refusal::Busy) if matches!(sent, Sent::Chunk(_)) => return Ok(()),
            Answer::Refused(Refusal::Update(UpdateRefusal::NotInstalling))
                if matches!(sent, Sent::Chunk(_) | Sent::State) =>
            {
                return Err(self.fail(InstallError::Lost));
            }
            Answer::Refused(refusal) => return Err(self.fail(InstallError::Refused(*refusal))),
            _ => return Err(self.fail(InstallError::Unexpected)),
        };
        let (phase, stopped) = (view.phase, view.stopped);
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
