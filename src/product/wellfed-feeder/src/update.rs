//! The update path's words on the wire: what each state and refusal of
//! `wellfed_core::ota` becomes in the device API, so the firmware and
//! every host fake answer an owner the same way.

use wellfed_core::ota::install::{Phase, Progress, Refused, Stop, WANTED};
use wellfed_core::ota::offer::{KeyError, OfferError, StatementError};
use wellfed_core::store::update::UpdateError;
use wellfed_proto::api::{Refusal, UpdatePhase, UpdateRefusal, UpdateStop, UpdateView, WANTED_MAX};

const _: () = assert!(WANTED == WANTED_MAX);

pub const fn stop(stop: Stop) -> UpdateStop {
    match stop {
        Stop::Timeout => UpdateStop::Timeout,
        Stop::Cancelled => UpdateStop::Cancelled,
        Stop::Order => UpdateStop::Order,
        Stop::Marker => UpdateStop::Marker,
        Stop::Write => UpdateStop::Write,
        Stop::Hash => UpdateStop::Hash,
        Stop::Activate => UpdateStop::Activate,
        Stop::Store => UpdateStop::Store,
    }
}

pub const fn view(progress: &Progress) -> UpdateView {
    UpdateView {
        phase: match progress.phase {
            Phase::Idle => UpdatePhase::Idle,
            Phase::Checking => UpdatePhase::Checking,
            Phase::GivingWay => UpdatePhase::GivingWay,
            Phase::Receiving => UpdatePhase::Receiving,
            Phase::Activated => UpdatePhase::Activated,
        },
        bank: progress.bank,
        min_version: progress.min_version,
        digest: progress.digest,
        version: progress.version,
        image_len: progress.image_len,
        written: progress.written,
        wanted: progress.wanted,
        stopped: match progress.stopped {
            Some(s) => Some(stop(s)),
            None => None,
        },
    }
}

const fn key(error: KeyError) -> UpdateRefusal {
    match error {
        KeyError::Unknown => UpdateRefusal::Key,
        KeyError::Revoked => UpdateRefusal::Revoked,
    }
}

const fn offer(error: OfferError) -> UpdateRefusal {
    match error {
        OfferError::Length | OfferError::Version | OfferError::Magic | OfferError::Kind => {
            UpdateRefusal::Manifest
        }
        OfferError::Key(error) => key(error),
        OfferError::Signature => UpdateRefusal::Signature,
        OfferError::Bank | OfferError::Size => UpdateRefusal::Target,
        OfferError::Same => UpdateRefusal::Same,
        OfferError::Downgrade => UpdateRefusal::Downgrade,
        OfferError::Statement(StatementError::Key(error)) => key(error),
        OfferError::Statement(_) => UpdateRefusal::Statement,
    }
}

const fn statement(error: StatementError) -> UpdateRefusal {
    match error {
        StatementError::Key(error) => key(error),
        StatementError::Signature => UpdateRefusal::Signature,
        StatementError::Length
        | StatementError::Magic
        | StatementError::Device
        | StatementError::Digest
        | StatementError::Floor => UpdateRefusal::Statement,
    }
}

/// Why an update request was refused, as the owner reads it.
pub const fn refusal(refused: Refused) -> Refusal {
    Refusal::Update(match refused {
        Refused::Busy => return Refusal::Busy,
        Refused::Store(UpdateError::Full | UpdateError::Occupied) => return Refusal::Full,
        Refused::Store(UpdateError::Revoked) => UpdateRefusal::Revoked,
        Refused::Store(_) | Refused::Unreadable => UpdateRefusal::Store,
        Refused::Offer(error) => offer(error),
        Refused::Statement(error) => statement(error),
        Refused::Fallback => UpdateRefusal::Fallback,
        Refused::Pending => UpdateRefusal::Pending,
        Refused::Failed => UpdateRefusal::Failed,
        Refused::Attempt => UpdateRefusal::Attempt,
        Refused::Installing => UpdateRefusal::Installing,
        Refused::NotInstalling => UpdateRefusal::NotInstalling,
    })
}
