//! Time and randomness a test can repeat.

use std::sync::atomic::{AtomicU64, Ordering};

use jiff::Timestamp;
use wellfed_service::{Clock, Random, RandomFailed};

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
