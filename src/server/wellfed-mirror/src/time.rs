//! The server's wall clock and random source, injected so tests can fix
//! both. Every server crate reads time and draws secrets through these.

use jiff::Timestamp;

/// The wall clock.
pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> Timestamp;
}

pub struct SystemClock;

impl Clock for SystemClock {
    #[expect(
        clippy::disallowed_methods,
        reason = "the one place a server crate reads the wall clock"
    )]
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

/// The system random generator failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the system random generator failed")]
pub struct RandomFailed;

/// Secret bytes: keys, session tokens, request ids, salts.
pub trait Random: Send + Sync + 'static {
    fn fill(&self, out: &mut [u8]) -> Result<(), RandomFailed>;
}

pub struct SystemRandom;

impl Random for SystemRandom {
    fn fill(&self, out: &mut [u8]) -> Result<(), RandomFailed> {
        getrandom::fill(out).map_err(|_| RandomFailed)
    }
}
