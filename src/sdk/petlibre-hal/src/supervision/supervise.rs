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
//!   stay stopped or sensing.
//! - **Trial deadline:** a trial image that has not confirmed itself within
//!   `TRIAL_DEADLINE_MS` stops feeding, so an image that runs but never gets
//!   healthy also falls back.

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
}

fn fresh(now: u32, beat: u32) -> bool {
    now.wrapping_sub(beat) < LIVENESS_MS
}

/// Whether to refresh the watchdog on this tick.
pub fn feed(i: &Inputs) -> bool {
    let trial = i.trial && !i.confirmed;
    if trial && i.now_ms >= TRIAL_DEADLINE_MS {
        return false;
    }
    match i.startup {
        Startup::Running => fresh(i.now_ms, i.startup_beat_ms),
        Startup::Failed => !trial,
        Startup::Started => i.tasks_alive && i.outputs_ok,
    }
}
