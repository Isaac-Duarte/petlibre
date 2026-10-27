//! How many failed logins the server takes: 5 a minute per name and 20 a
//! minute per address, so a password cannot be guessed at the rate the
//! server hashes.
//!
//! Each name and each address has a token bucket, kept as the time it is
//! next full (a generic cell rate: one attempt spends a minute's share, and
//! a bucket refuses an attempt that would take it past a minute ahead).
//! An attempt spends from both buckets before the password is checked, so
//! a flood of concurrent attempts cannot all pass a check made before any
//! of them failed; a login that succeeds gives its share back. No account
//! is locked: a limited name or address is refused `Retry-After` the time
//! until its next share, and sessions and tokens already open carry on.
//!
//! The buckets live in one map of at most [`ENTRIES`], so a flood of
//! names or addresses costs bounded memory. A bucket is dropped only once
//! it is full again, when it holds nothing a later attempt could miss; the
//! map is kept in order of that time, so any such bucket is found at once.
//! While every bucket still holds state, an attempt that needs a new one
//! is refused until the first is full again: a flood of fresh names or
//! addresses can delay others' logins, never buy the name under attack
//! more guesses by pushing its bucket out. An IPv6 address counts by its
//! /64, the block a single host is usually given, so rotating within it
//! buys nothing.

use std::collections::{BTreeSet, HashMap};
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Duration;

use jiff::Timestamp;

use crate::lock;

/// The most buckets kept.
pub(crate) const ENTRIES: usize = 10_000;

/// Failed logins a name takes a minute.
const PER_NAME: i64 = 5;
/// Failed logins an address takes a minute.
const PER_ADDRESS: i64 = 20;
const MINUTE_MS: i64 = 60_000;

/// The longest name a bucket keeps: longer than any account's, so it
/// limits the names that exist exactly, and bounds what a key holds.
const NAME_CHARS: usize = 64;

/// Whose bucket.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Key {
    /// A name, trimmed and lowercased, so its spellings share one bucket.
    Name(String),
    /// An IPv4 address, or an IPv6 address's /64.
    Address(IpAddr),
}

impl Key {
    fn name(name: &str) -> Self {
        let name: String = name.trim().chars().take(NAME_CHARS).collect();
        Self::Name(name.to_lowercase())
    }

    fn address(address: IpAddr) -> Self {
        Self::Address(match address.to_canonical() {
            IpAddr::V4(v4) => IpAddr::V4(v4),
            IpAddr::V6(v6) => {
                let prefix = u128::from(v6) & !u128::from(u64::MAX);
                IpAddr::V6(prefix.into())
            }
        })
    }

    /// The time an attempt spends, in milliseconds.
    const fn interval(&self) -> i64 {
        match self {
            Self::Name(_) => MINUTE_MS / PER_NAME,
            Self::Address(_) => MINUTE_MS / PER_ADDRESS,
        }
    }
}

/// A login refused for its rate: the next attempt is taken after
/// `retry_after`, a whole number of seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Limited {
    pub(crate) retry_after: Duration,
}

/// An attempt that spent from its buckets; [`Limits::succeeded`] gives the
/// spend back.
#[must_use = "a login that succeeds gives its attempt back"]
pub(crate) struct Attempt {
    keys: [Key; 2],
}

#[derive(Debug, Default)]
pub(crate) struct Limits {
    buckets: Mutex<Buckets>,
}

#[derive(Debug, Default)]
struct Buckets {
    /// Each bucket's next-full time (Unix ms).
    entries: HashMap<Key, i64>,
    /// The same, in order of that time, the first full again first.
    by_full: BTreeSet<(i64, Key)>,
    /// When a full map last warned that it refused a new bucket (Unix ms):
    /// a flood warns once a minute, not once an attempt.
    warned: Option<i64>,
}

impl Limits {
    /// Spend one attempt from `name`'s and `from`'s buckets at `now`, or
    /// refuse with the wait until both take one, spending nothing.
    pub(crate) fn attempt(
        &self,
        name: &str,
        from: IpAddr,
        now: Timestamp,
    ) -> Result<Attempt, Limited> {
        let now = now.as_millisecond();
        let keys = [Key::name(name), Key::address(from)];
        let mut buckets = lock(&self.buckets);

        let wait = keys
            .iter()
            .map(|key| {
                let full = buckets.full(key, now);
                (full + key.interval() - now - MINUTE_MS).max(0)
            })
            .max()
            .unwrap_or(0);
        let new = keys
            .iter()
            .filter(|key| !buckets.entries.contains_key(key))
            .count();
        let room = buckets.make_room(&keys, new, now);
        if room > 0
            && buckets
                .warned
                .is_none_or(|warned| now - warned >= MINUTE_MS)
        {
            buckets.warned = Some(now);
            tracing::warn!(
                buckets = ENTRIES,
                "the login limits are full: logins from new names or addresses wait until a bucket refills"
            );
        }
        let wait = wait.max(room);
        if wait > 0 {
            // Whole seconds, rounded up, as `Retry-After` carries them.
            let retry_after = Duration::from_secs(wait.unsigned_abs().div_ceil(1000));
            return Err(Limited { retry_after });
        }

        for key in &keys {
            let full = buckets.full(key, now);
            buckets.set(key.clone(), full.max(now) + key.interval());
        }
        Ok(Attempt { keys })
    }

    /// Give back a login's attempt: it was no guess. A bucket full again
    /// holds nothing worth keeping, and is dropped.
    pub(crate) fn succeeded(&self, attempt: Attempt, now: Timestamp) {
        let now = now.as_millisecond();
        let mut buckets = lock(&self.buckets);
        for key in attempt.keys {
            let full = buckets.full(&key, now) - key.interval();
            if full <= now {
                buckets.remove(&key);
            } else {
                buckets.set(key, full);
            }
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        lock(&self.buckets).entries.len()
    }
}

impl Buckets {
    /// When `key`'s bucket is next full, no later than a minute from now:
    /// a clock stepped back leaves no bucket refusing for longer than one
    /// full minute would.
    fn full(&self, key: &Key, now: i64) -> i64 {
        let full = self.entries.get(key).copied().unwrap_or(now);
        full.min(now + MINUTE_MS)
    }

    /// Drop buckets full again until `new` more fit, or say how long until
    /// the first that would have to go is full: a bucket still holding
    /// state is never dropped. The attempt's own `keys` are never the ones
    /// dropped, since it is about to set them, so the map never passes
    /// [`ENTRIES`].
    fn make_room(&mut self, keys: &[Key; 2], new: usize, now: i64) -> i64 {
        let over = (self.entries.len() + new).saturating_sub(ENTRIES);
        let victims: Vec<(i64, Key)> = self
            .by_full
            .iter()
            .filter(|(_, key)| !keys.contains(key))
            .take(over)
            .cloned()
            .collect();
        let blocking = victims
            .iter()
            .map(|(full, _)| full.min(&(now + MINUTE_MS)) - now)
            .max()
            .unwrap_or(0);
        if blocking > 0 {
            return blocking;
        }

        for (_, key) in &victims {
            self.remove(key);
        }
        0
    }

    fn set(&mut self, key: Key, full: i64) {
        self.remove(&key);
        self.by_full.insert((full, key.clone()));
        self.entries.insert(key, full);
    }

    fn remove(&mut self, key: &Key) {
        if let Some(full) = self.entries.remove(key) {
            self.by_full.remove(&(full, key.clone()));
        }
    }
}

#[cfg(test)]
mod tests;
