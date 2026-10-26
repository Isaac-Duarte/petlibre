//! When the KM4 watchdog feeder feeds.
//!
//! The feeder opens the watchdog on its first step, before the startup
//! diagnostic and probes run, so a fault or hang anywhere in startup becomes
//! a reset. For an unconfirmed bank-1 image KM0 counts that reset and gives
//! way to bank 0 after `trial::LIMIT`. This module is the feed decision only;
//! the glue gathers its inputs.
//!
//! - **Startup:** feed while the startup task keeps stepping.
//! - **Determined startup failure:** a trial image stops feeding, so the
//!   failure counts as a failed trial. A confirmed image (or bank 0) keeps
//!   feeding and holds STOP, rather than reset-looping.
//! - **Running:** feed while every running task keeps stepping and outputs
//!   stay stopped or sensing. While a feeding owner runs, its beat must also
//!   stay fresh, and while it supervises a feed its supervisor stands in for
//!   the stopped check (the timer interrupt stops the motor within a lease;
//!   the watchdog is the backstop for that interrupt dying).
//! - **Trial deadline:** a trial image that has not confirmed itself within
//!   `TRIAL_DEADLINE_MS` stops feeding, so an image that runs but never gets
//!   healthy also falls back.
//! - **Feeding that cannot open:** a trial image whose feeding owner failed
//!   to open (its journal, setup or recovery) stops feeding at once, as a
//!   determined startup failure does: it can never prove it feeds, so it
//!   falls back without waiting out the deadline three times.

use crate::supervision::beat::Beat;

/// What the firmware's feeding owner has shown this boot. A trial build
/// confirms only once its owner has proved it can feed, so a build that
/// breaks feeding falls back to bank 0 rather than confirming.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Feeding {
    /// The firmware runs no feeding owner: there is nothing to prove.
    #[default]
    Unclaimed = 0,
    /// The owner has not opened yet, or has not yet passed its checks.
    Pending = 1,
    /// Opened on the journal and at rest with its motor, sensor and current
    /// checks passing.
    Proven = 2,
    /// It cannot open this boot: a determined failure.
    Failed = 3,
}

impl Feeding {
    /// The state as the runtime's `AtomicU8` holds it.
    pub const fn code(self) -> u8 {
        self as u8
    }

    /// The inverse of `code`; anything else reads as a failure.
    pub const fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Unclaimed,
            1 => Self::Pending,
            2 => Self::Proven,
            _ => Self::Failed,
        }
    }

    /// Whether a trial build may confirm on it.
    pub const fn proven(self) -> bool {
        matches!(self, Self::Unclaimed | Self::Proven)
    }
}

/// A task step older than this is a hang.
pub const LIVENESS_MS: u32 = 10_000;
/// Uptime by which a trial image must have confirmed itself. Confirmation
/// needs 30 s of continuous health after the startup probes (about two
/// minutes), so this leaves room for a slow join.
pub const TRIAL_DEADLINE_MS: u32 = 10 * 60 * 1000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Startup {
    /// The startup diagnostic and indicator probe are still stepping.
    #[default]
    Running,
    /// The role's tasks have started.
    Started,
    /// Startup stopped on a determined failure.
    Failed,
}

impl Startup {
    /// The state as the app's `AtomicU8` holds it (and its `PLSUP1` lines
    /// print it).
    pub const fn code(self) -> u8 {
        match self {
            Self::Running => 0,
            Self::Started => 1,
            Self::Failed => 2,
        }
    }

    /// The inverse of `code`; anything else reads as a failure.
    pub const fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Running,
            1 => Self::Started,
            _ => Self::Failed,
        }
    }
}

/// One feeder tick's view of the system.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Inputs {
    pub now_ms: u32,
    pub startup: Startup,
    /// The startup task's last step, in uptime milliseconds.
    pub startup_beat_ms: u32,
    /// Running bank 1 without a confirmation record for this build.
    pub trial: bool,
    /// This boot wrote the confirmation record.
    pub confirmed: bool,
    /// Every running task stepped within `LIVENESS_MS` (or has ended).
    pub tasks_alive: bool,
    /// Outputs verified stopped, or motor STOP with only the supervised
    /// emitters lit.
    pub outputs_ok: bool,
    /// The feeding owner's beat, while one runs.
    pub owner: Option<Beat>,
    /// What the feeding owner has shown this boot.
    pub feeding: Feeding,
}

fn fresh(now: u32, beat: u32) -> bool {
    now.wrapping_sub(beat) < LIVENESS_MS
}

/// Whether to refresh the watchdog on this tick.
pub fn feed(i: &Inputs) -> bool {
    let trial = i.trial && !i.confirmed;
    if trial && (i.now_ms >= TRIAL_DEADLINE_MS || i.feeding == Feeding::Failed) {
        return false;
    }
    match i.startup {
        Startup::Running => fresh(i.now_ms, i.startup_beat_ms),
        Startup::Failed => !trial,
        Startup::Started => {
            let owner_alive = i.owner.is_none_or(|beat| beat.fresh(i.now_ms));
            let supervised = i
                .owner
                .is_some_and(|beat| beat.supervised && beat.fresh(i.now_ms));
            i.tasks_alive && owner_alive && (i.outputs_ok || supervised)
        }
    }
}
