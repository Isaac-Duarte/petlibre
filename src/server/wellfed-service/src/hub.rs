//! Live updates for the server's event stream: every feeder task publishes
//! here, and each stream subscriber reads a broadcast.
//!
//! Each update gets a [`StreamId`], the stream's SSE `id`. The hub keeps
//! the most recent updates, so a client that reconnects with its last id
//! gets what it missed; one that is too far behind, or comes from before a
//! restart (another epoch), is told to resync instead.

use std::collections::VecDeque;
use std::fmt;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use wellfed_api::StreamEvent;
use tokio::sync::broadcast;

/// How many recent updates a reconnecting client can catch up on, and how
/// far a slow subscriber may fall behind before it must resync.
const RECENT: usize = 1024;

/// A position in the stream: this run's epoch and a sequence number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamId {
    pub epoch: u64,
    pub seq: u64,
}

impl fmt::Display for StreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}-{}", self.epoch, self.seq)
    }
}

impl FromStr for StreamId {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, ()> {
        let (epoch, seq) = text.trim().split_once('-').ok_or(())?;
        if epoch.len() != 16 {
            return Err(());
        }
        Ok(Self {
            epoch: u64::from_str_radix(epoch, 16).map_err(|_| ())?,
            seq: seq.parse().map_err(|_| ())?,
        })
    }
}

#[derive(Debug)]
pub struct Stamped {
    pub id: StreamId,
    pub event: StreamEvent,
}

/// What a new subscriber gets before the live updates.
#[derive(Debug)]
pub enum Resume {
    /// The updates after its last id (none for a new client).
    Replay(Vec<Arc<Stamped>>),
    /// Its last id is no longer held: it must refetch.
    Resync,
}

#[derive(Debug)]
pub struct Subscription {
    pub resume: Resume,
    pub live: broadcast::Receiver<Arc<Stamped>>,
}

#[derive(Debug)]
pub struct Hub {
    epoch: u64,
    sender: broadcast::Sender<Arc<Stamped>>,
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    next: u64,
    recent: VecDeque<Arc<Stamped>>,
}

impl Hub {
    pub(crate) fn new() -> Self {
        // A restart must not reuse ids, so the epoch is the start time.
        Self::with_epoch(jiff::Timestamp::now().as_microsecond().unsigned_abs())
    }

    fn with_epoch(epoch: u64) -> Self {
        Self {
            epoch,
            sender: broadcast::channel(RECENT).0,
            state: Mutex::new(State {
                next: 0,
                recent: VecDeque::with_capacity(RECENT),
            }),
        }
    }

    pub(crate) fn publish(&self, event: StreamEvent) {
        let mut state = self.lock();
        let stamped = Arc::new(Stamped {
            id: StreamId {
                epoch: self.epoch,
                seq: state.next,
            },
            event,
        });
        state.next += 1;
        if state.recent.len() == RECENT {
            state.recent.pop_front();
        }
        state.recent.push_back(Arc::clone(&stamped));
        // Sent under the lock, so a subscriber's replay and its live
        // updates neither overlap nor leave a hole. No subscriber is fine.
        let _ = self.sender.send(stamped);
    }

    /// Subscribe, resuming after `last` when given.
    pub fn subscribe(&self, last: Option<StreamId>) -> Subscription {
        let state = self.lock();
        let live = self.sender.subscribe();
        let resume = match last {
            None => Resume::Replay(Vec::new()),
            Some(last) if last.epoch != self.epoch || last.seq >= state.next => Resume::Resync,
            Some(last) => {
                let oldest = state.recent.front().map_or(state.next, |s| s.id.seq);
                if last.seq + 1 < oldest {
                    Resume::Resync
                } else {
                    let missed = state.recent.iter().filter(|s| s.id.seq > last.seq);
                    Resume::Replay(missed.cloned().collect())
                }
            }
        };
        Subscription { resume, live }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        // A panic while holding the lock leaves only counters behind,
        // which stay consistent, so a poisoned lock is still usable.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seqs(resume: &Resume) -> Option<Vec<u64>> {
        match resume {
            Resume::Replay(missed) => Some(missed.iter().map(|s| s.id.seq).collect()),
            Resume::Resync => None,
        }
    }

    #[test]
    fn a_reconnect_replays_what_it_missed_or_resyncs() {
        let hub = Hub::with_epoch(7);
        for _ in 0..3 {
            hub.publish(StreamEvent::Resync);
        }
        let at = |seq| Some(StreamId { epoch: 7, seq });

        assert_eq!(seqs(&hub.subscribe(None).resume), Some(vec![]));
        assert_eq!(seqs(&hub.subscribe(at(0)).resume), Some(vec![1, 2]));
        assert_eq!(seqs(&hub.subscribe(at(2)).resume), Some(vec![]));
        assert_eq!(seqs(&hub.subscribe(at(3)).resume), None, "from the future");
        let other = Some(StreamId { epoch: 8, seq: 0 });
        assert_eq!(seqs(&hub.subscribe(other).resume), None, "another run");

        for _ in 0..RECENT {
            hub.publish(StreamEvent::Resync);
        }
        assert_eq!(seqs(&hub.subscribe(at(1)).resume), None, "too far back");
        let held = seqs(&hub.subscribe(at(2)).resume).unwrap();
        assert_eq!(held.len(), RECENT);
    }

    #[test]
    fn a_subscriber_receives_what_is_published_after_it() {
        let hub = Hub::with_epoch(1);
        let mut sub = hub.subscribe(None);
        hub.publish(StreamEvent::Resync);
        let got = sub.live.try_recv().unwrap();
        assert_eq!(got.id, StreamId { epoch: 1, seq: 0 });
    }

    #[test]
    fn ids_round_trip_through_text() {
        let id = StreamId {
            epoch: 0xdead_beef,
            seq: 42,
        };
        assert_eq!(id.to_string(), "00000000deadbeef-42");
        assert_eq!(id.to_string().parse(), Ok(id));
        assert_eq!("deadbeef-42".parse::<StreamId>(), Err(()));
        assert_eq!("00000000deadbeef".parse::<StreamId>(), Err(()));
    }
}
