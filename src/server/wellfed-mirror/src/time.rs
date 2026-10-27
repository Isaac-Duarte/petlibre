//! The server's wall clock and random source, injected so tests can fix
//! both. Every server crate reads time and draws secrets through these.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use jiff::Timestamp;

/// How long [`Clock::sleep_until`] sleeps at most before reading the wall
/// clock again, so a clock stepped while it sleeps (a correction, a
/// suspended host) is noticed within this.
const RECHECK: Duration = Duration::from_secs(60);

/// The wall clock.
pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> Timestamp;

    /// Wait until this clock shows `at` or later. The default sleeps on
    /// the runtime's timer and reads [`Self::now`] again at least every
    /// minute; a test clock that moves by hand wakes its sleepers instead.
    fn sleep_until(&self, at: Timestamp) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            loop {
                let left = self.now().duration_until(at);
                let Ok(left) = Duration::try_from(left) else {
                    return;
                };
                if left.is_zero() {
                    return;
                }
                tokio::time::sleep(left.min(RECHECK)).await;
            }
        })
    }
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
