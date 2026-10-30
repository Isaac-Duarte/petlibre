//! Where a run is, for the report of a stop that ends it: the step, and the
//! write it sent whose outcome had not settled (the request until the
//! replacement answers, the update key's enrollment until the run moves on,
//! the promotion until bank 1 confirms it, the plans' put until the end).
//! That write is uncertain. None is sent again, but for the plans' put: a later
//! run sends it again only if the plans' generation still equals the base kept
//! on disk before it left, and otherwise stops with nothing sent.

use std::fmt;

use wellfed_client::install::Phase;

/// A write the run sent whose outcome is not settled yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsettled {
    /// Stock's install request, under this id: settled once the
    /// replacement answers its status.
    Request(uuid::Uuid),
    /// The update key's enrollment, which a press may have admitted after
    /// its answer was lost: settled once the run moves past it.
    UpdateKey,
    /// The signed update, pushed from bank 0 into bank 1: settled once
    /// bank 1 confirms it.
    Promotion,
    /// Stock's converted plans, put on the replacement: settled once the
    /// run ends, a put that went unanswered being judged by a read back. A
    /// later run sends it again only from an unchanged generation.
    Plans,
}

impl Unsettled {
    /// Whether the run reaching `phase` settles this write.
    fn settled_at(self, phase: Phase) -> bool {
        match self {
            Self::Request(_) => phase > Phase::ReadStatus,
            Self::UpdateKey => phase > Phase::UpdateKey,
            Self::Promotion => phase > Phase::AwaitConfirm,
            Self::Plans => phase == Phase::Done,
        }
    }
}

impl fmt::Display for Unsettled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(id) => write!(f, "stock's install request {id}"),
            Self::UpdateKey => f.write_str("the update key's enrollment"),
            Self::Promotion => f.write_str("the promotion's push"),
            Self::Plans => f.write_str("the put of stock's plans"),
        }
    }
}

/// The steps' phase, as the report of a stopped run names it.
const fn phase_text(phase: Phase) -> &'static str {
    match phase {
        Phase::Provision => "pointing stock at this computer, before anything was sent to it",
        Phase::AwaitStock => "waiting for stock to report, before anything was sent to it",
        Phase::ConfirmFeeder => "the feeder check, before anything was sent to stock",
        Phase::Request => "the install request",
        Phase::AwaitInstall => "stock's install of the replacement",
        Phase::Pair => "pairing (Bluetooth setup or the recovery window)",
        Phase::Join => "waiting for the replacement to join the network after Bluetooth setup",
        Phase::ReadStatus => "reading the replacement's status",
        Phase::CloseTrial => "closing the stock server's record",
        Phase::UpdateKey => "enrolling the update key",
        Phase::Backup => "reading the stock backup",
        Phase::StockPlans => "reading stock's plans",
        Phase::Sign => "reading the feeder's floor and signing the promotion",
        Phase::Promote => "the promotion",
        Phase::AwaitConfirm => "waiting for bank 1 to confirm the promotion",
        Phase::ApplyPlans => "applying stock's plans to the replacement",
        Phase::Done => "the end of the run",
    }
}

/// The phase the run is in and the write it has not seen settle.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Track {
    phase: Phase,
    unsettled: Option<Unsettled>,
}

impl Track {
    pub(crate) const fn new() -> Self {
        Self {
            phase: Phase::Provision,
            unsettled: None,
        }
    }

    /// The run moved on to `phase`; a write that phase settles is no
    /// longer out.
    pub(crate) fn enter(&mut self, phase: Phase) {
        self.phase = phase;
        if self.unsettled.is_some_and(|write| write.settled_at(phase)) {
            self.unsettled = None;
        }
    }

    /// `write` is about to go out. It stays unsettled from here on, even
    /// before its answer, since a stop may come at any later await.
    pub(crate) fn sent(&mut self, write: Unsettled) {
        self.unsettled = Some(write);
    }

    pub(crate) fn stopped(self, by: Stop) -> Stopped {
        Stopped {
            by,
            phase: self.phase,
            unsettled: self.unsettled,
        }
    }
}

/// What ends a run through its cleanup: SIGTERM (a bench job's cancel, or
/// the app's), SIGINT (a terminal's Ctrl-C).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    Terminate,
    Interrupt,
}

impl fmt::Display for Stop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Terminate => "SIGTERM",
            Self::Interrupt => "SIGINT",
        })
    }
}

/// A run a stop ended, after stock's server stopped: the step it was in,
/// and the write whose outcome had not settled, if any.
#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error(
    "stopped by {by} during {}{}",
    phase_text(*.phase),
    unsettled_note(*.phase, *.unsettled)
)]
pub struct Stopped {
    pub by: Stop,
    pub phase: Phase,
    pub unsettled: Option<Unsettled>,
}

/// What the report adds after the phase. Pairing is only an owner
/// enrollment, safe to repeat, but a write still unsettled beside it is not,
/// so the two are told apart.
fn unsettled_note(phase: Phase, unsettled: Option<Unsettled>) -> String {
    let pairing = phase == Phase::Pair;
    match unsettled {
        None if pairing => {
            " (an owner enrollment, safe to run again); no write was waiting on its outcome"
                .to_string()
        }
        None => "; no write was waiting on its outcome".to_string(),
        Some(write) => {
            let enrollment = if pairing {
                " The enrollment is safe to repeat; that write is not."
            } else {
                ""
            };
            let again = match write {
                Unsettled::Plans => {
                    "a later run sends it again only if the feeder's plans have not changed since"
                }
                _ => "it is not sent again",
            };
            format!(
                ", with {write} sent and its outcome not yet settled: it may have run, and \
                 {again}.{enrollment} Read what the feeder runs (wellfed status, or \
                 stock's own report) before running again"
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The update key's enrollment stays unsettled until the run moves
    /// past it, so a stop during the press names it.
    #[test]
    fn the_enrollment_stays_unsettled_until_the_run_moves_on() {
        let mut track = Track::new();
        track.enter(Phase::UpdateKey);
        track.sent(Unsettled::UpdateKey);
        track.enter(Phase::UpdateKey);
        assert_eq!(track.unsettled, Some(Unsettled::UpdateKey));
        track.enter(Phase::Backup);
        assert_eq!(track.unsettled, None);
    }

    /// Stopped while stock downloads: the request is out and unsettled,
    /// through pairing, until the replacement answers its status.
    #[test]
    fn a_request_stays_unsettled_until_the_replacement_answers() {
        let id = uuid::Uuid::from_u128(7);
        let mut track = Track::new();
        track.enter(Phase::Request);
        track.sent(Unsettled::Request(id));
        for phase in [Phase::AwaitInstall, Phase::Pair, Phase::ReadStatus] {
            track.enter(phase);
            assert_eq!(track.unsettled, Some(Unsettled::Request(id)), "{phase:?}");
        }
        track.enter(Phase::AwaitInstall);
        let stopped = track.stopped(Stop::Terminate).to_string();
        assert!(stopped.starts_with("stopped by SIGTERM during stock's install"));
        assert!(stopped.contains(&format!("stock's install request {id} sent")));
        assert!(stopped.contains("not sent again"));

        track.enter(Phase::CloseTrial);
        assert_eq!(track.unsettled, None);
        let stopped = track.stopped(Stop::Interrupt).to_string();
        assert!(
            stopped.ends_with("no write was waiting on its outcome"),
            "{stopped}"
        );
    }

    /// Stopped after the push came back activated but before bank 1
    /// confirmed: the promotion is still unsettled.
    #[test]
    fn a_promotion_stays_unsettled_until_bank_1_confirms() {
        let mut track = Track::new();
        track.enter(Phase::Promote);
        track.sent(Unsettled::Promotion);
        track.enter(Phase::AwaitConfirm);
        let stopped = track.stopped(Stop::Terminate);
        assert_eq!(stopped.unsettled, Some(Unsettled::Promotion));
        assert!(
            stopped
                .to_string()
                .contains("waiting for bank 1 to confirm the promotion, with the promotion's push")
        );

        track.enter(Phase::ApplyPlans);
        assert_eq!(track.unsettled, None);
        track.enter(Phase::ApplyPlans);
        track.sent(Unsettled::Plans);
        // A put that went unanswered is judged by reading the plans back.
        track.enter(Phase::ApplyPlans);
        assert_eq!(track.unsettled, Some(Unsettled::Plans));
        let stopped = track.stopped(Stop::Interrupt).to_string();
        assert!(stopped.contains("only if the feeder's plans have not changed since"));
        track.enter(Phase::Done);
        assert_eq!(track.unsettled, None);
    }

    /// Pairing sends no write that matters twice: the report says so.
    #[test]
    fn a_stop_while_pairing_says_it_is_safe_to_run_again() {
        let mut track = Track::new();
        track.enter(Phase::Pair);
        let stopped = track.stopped(Stop::Interrupt).to_string();
        assert!(stopped.contains("safe to run again"), "{stopped}");
        assert!(stopped.ends_with("no write was waiting on its outcome"));

        // Paired before the replacement's status settled the request: the
        // enrollment may be repeated, the request may not.
        track.sent(Unsettled::Request(uuid::Uuid::from_u128(7)));
        let stopped = track.stopped(Stop::Interrupt).to_string();
        assert!(!stopped.contains("safe to run again"), "{stopped}");
        assert!(
            stopped.contains("The enrollment is safe to repeat; that write is not."),
            "{stopped}"
        );
    }
}
