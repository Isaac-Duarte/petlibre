//! Time and randomness a test can repeat.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

use jiff::Timestamp;
use wellfed_service::{Clock, Random, RandomFailed};
use tokio::sync::Notify;

/// The instant a [`FixedClock`] shows, in Unix milliseconds: inside the
/// feeder clock's accepted range, so a clock sync from it is applied.
pub const NOW_MS: i64 = 1_790_000_000_000;

/// A clock that always shows [`NOW_MS`].
#[derive(Clone, Copy, Debug, Default)]
pub struct FixedClock;

impl FixedClock {
    pub fn now(self) -> Timestamp {
        Timestamp::from_millisecond(NOW_MS).expect("NOW_MS is a valid instant")
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        FixedClock::now(*self)
    }
}

/// A clock that starts at [`NOW_MS`] and moves only when a test moves it,
/// waking whatever sleeps until a time it passes. Clones share the time.
#[derive(Clone, Debug)]
pub struct ManualClock(Arc<Manual>);

#[derive(Debug)]
struct Manual {
    ms: AtomicI64,
    moved: Notify,
}

impl ManualClock {
    pub fn new() -> Self {
        Self(Arc::new(Manual {
            ms: AtomicI64::new(NOW_MS),
            moved: Notify::new(),
        }))
    }

    /// Move the clock on by `by`.
    pub fn advance(&self, by: Duration) {
        let by =
            i64::try_from(by.as_millis()).expect("a test moves the clock by less than 2^63 ms");
        self.0.ms.fetch_add(by, Ordering::SeqCst);
        self.0.moved.notify_waiters();
    }

    /// Move the clock on to `at`, or leave it where it is when it is past.
    pub fn advance_to(&self, at: Timestamp) {
        self.0.ms.fetch_max(at.as_millisecond(), Ordering::SeqCst);
        self.0.moved.notify_waiters();
    }

    /// Move the clock back by `by`, as an NTP correction does. Nothing
    /// sleeping wakes: its time only moved further off.
    pub fn step_back(&self, by: Duration) {
        let by =
            i64::try_from(by.as_millis()).expect("a test moves the clock by less than 2^63 ms");
        self.0.ms.fetch_sub(by, Ordering::SeqCst);
    }
}

impl Default for ManualClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_millisecond(self.0.ms.load(Ordering::SeqCst))
            .expect("a test keeps the clock in jiff's range")
    }

    fn sleep_until(&self, at: Timestamp) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            loop {
                // Registered before the time is read, so a move in between
                // still wakes it.
                let mut moved = std::pin::pin!(self.0.moved.notified());
                moved.as_mut().enable();
                if self.now() >= at {
                    return;
                }
                moved.await;
            }
        })
    }
}

/// A seeded generator (SplitMix64): the same seed gives the same keys,
/// tokens and request ids. Not for secrets outside a test.
#[derive(Debug)]
pub struct SeededRandom(AtomicU64);

impl SeededRandom {
    pub fn new(seed: u64) -> Self {
        Self(AtomicU64::new(seed))
    }

    /// A generator with the next seed of this process. Each service a test
    /// starts takes its own, so a service reopened on the same database
    /// does not draw the request ids its first run sent the feeder (which
    /// would answer them from memory).
    pub fn fresh() -> Self {
        static SEED: AtomicU64 = AtomicU64::new(0x5eed);
        Self::new(SEED.fetch_add(1, Ordering::Relaxed))
    }

    fn word(&self) -> u64 {
        const GAMMA: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut z = self
            .0
            .fetch_add(GAMMA, Ordering::Relaxed)
            .wrapping_add(GAMMA);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

impl Random for SeededRandom {
    fn fill(&self, out: &mut [u8]) -> Result<(), RandomFailed> {
        for chunk in out.chunks_mut(8) {
            let word = self.word().to_le_bytes();
            chunk.copy_from_slice(&word[..chunk.len()]);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(random: &SeededRandom) -> [u8; 20] {
        let mut out = [0; 20];
        random.fill(&mut out).expect("a seeded fill never fails");
        out
    }

    #[test]
    fn a_seed_repeats_and_draws_differ() {
        let (one, two) = (SeededRandom::new(7), SeededRandom::new(7));
        let first = draw(&one);
        assert_eq!(first, draw(&two));
        assert_ne!(first, draw(&one));
        assert_ne!(draw(&SeededRandom::fresh()), draw(&SeededRandom::fresh()));
    }
}
