//! The server's per-feeder runtime ([API design]): every task that holds a
//! feeder session lives here, and nothing above it sees a socket.
//!
//! - **Each feeder's task** keeps a session open, reconnecting with
//!   jittered backoff and, when its address fails, finding the feeder over
//!   mDNS. On every connect it first subscribes after the newest event the
//!   store holds, so its own outages lose nothing, then sets the feeder's
//!   clock in the household's [`Zone`] and gives it the zone's POSIX rule,
//!   so it follows DST with the server down. It records every event and
//!   sample as the feeder sent it: one store transaction per event, with
//!   its projections and the cursor.
//! - **Commands** reach the task on two lanes. A short one is
//!   [`Mirror::ask`]; a long operation holds an [`Operation`] and sends its
//!   steps one at a time. The task pipelines the lanes over one session, so
//!   a short command is answered after at most one step. One that is
//!   not answered in time is [`AskError::Unknown`], never a failure: the
//!   feeder may have run it, and a retry under the same request id gets its
//!   first answer. One whose caller stopped waiting is dropped unsent.
//! - **What a feeder sends** goes out on its own broadcast of
//!   [`MirrorEvent`]s (events, gaps, samples and link changes), after it is
//!   stored. The service merges the broadcasts into its stream.
//!
//! The mirror knows feeders by id, identity, key and address. It knows
//! nothing of users, alerts or stream ids, and the request ids it sends for
//! a caller are the caller's. Time and randomness come in through [`Clock`]
//! and [`Random`], so tests can fix them.
//!
//! [API design]: ../../../../docs/api/README.md
#![forbid(unsafe_code)]

mod command;
mod connect;
mod error;
mod ingest;
mod lanes;
mod session;
mod task;
mod time;
mod zone;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use wellfed_api::{Connection as Link, FeederEvent, FeederId, FeederSample, Gap};
use wellfed_store::Store;
use tokio::sync::{broadcast, watch};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub use command::Operation;
pub use connect::Enrolled;
pub use error::{AskError, ConnectError, Unreachable};
pub use ingest::stored_event;
pub use wellfed_client::api::{Invite, InviteText};
pub use time::{Clock, Random, RandomFailed, SystemClock, SystemRandom};
pub use zone::{Zone, ZoneError};

/// A device request id: the feeder answers a repeated one from memory.
pub type RequestId = wellfed_proto::api::RequestId;

/// How many of a feeder's updates a slow subscriber may fall behind
/// before it is told it lagged.
const UPDATES: usize = 1024;

/// Whether a feeder whose address fails is looked up over mDNS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Discovery {
    Mdns,
    Off,
}

#[derive(Debug)]
pub struct Config {
    pub discovery: Discovery,
    pub timing: Timing,
}

#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// The first reconnect delay, doubled after each failure.
    pub retry_min: Duration,
    pub retry_max: Duration,
    /// How often a connected feeder's clock is set again.
    pub time_sync: Duration,
    /// How long a caller waits for a command's answer: the connection's
    /// own answer timeout, and some room to queue behind another command.
    pub answer: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            retry_min: Duration::from_secs(1),
            retry_max: Duration::from_secs(60),
            time_sync: Duration::from_secs(6 * 3600),
            answer: wellfed_connect::ANSWER + Duration::from_secs(5),
        }
    }
}

/// A feeder to mirror: who it is, the server's key on it and where it was
/// last reached.
pub struct Feeder {
    pub id: FeederId,
    pub identity: [u8; 32],
    /// The server's secret key on this feeder, unsealed.
    pub secret: [u8; 32],
    pub address: Option<String>,
}

impl std::fmt::Debug for Feeder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Feeder")
            .field("id", &self.id)
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

/// One update from a feeder, sent after it is stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MirrorEvent {
    /// An event the store did not hold before.
    Event(FeederEvent),
    /// Events the feeder dropped before the server read them.
    Gap(Gap),
    Sample(FeederSample),
    /// The session's state changed.
    Link(Link),
}

/// What every feeder task shares.
pub(crate) struct Shared {
    store: Store,
    config: Config,
    clock: Arc<dyn Clock>,
    random: Arc<dyn Random>,
    /// The household's zone; `None` before there is a household, when the
    /// clock syncs are in UTC.
    zone: watch::Sender<Option<Arc<Zone>>>,
}

impl Shared {
    fn random<const N: usize>(&self) -> Result<[u8; N], RandomFailed> {
        let mut out = [0; N];
        self.random.fill(&mut out)?;
        Ok(out)
    }
}

/// The running feeder tasks: cheap to clone, one per server.
#[derive(Clone)]
pub struct Mirror {
    shared: Arc<Shared>,
    feeders: Arc<Mutex<HashMap<FeederId, task::Handle>>>,
    cancel: CancellationToken,
    tasks: TaskTracker,
}

impl std::fmt::Debug for Mirror {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mirror").finish_non_exhaustive()
    }
}

impl Mirror {
    /// A mirror with no feeders yet. Cancelling `cancel` stops every task
    /// it starts.
    pub fn new(
        store: Store,
        config: Config,
        clock: Arc<dyn Clock>,
        random: Arc<dyn Random>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                store,
                config,
                clock,
                random,
                zone: watch::Sender::new(None),
            }),
            feeders: Arc::new(Mutex::new(HashMap::new())),
            cancel,
            tasks: TaskTracker::new(),
        }
    }

    /// Start mirroring `feeder`, and subscribe to its updates from the
    /// first. A feeder already running is stopped first, and its task has
    /// ended before the new one starts, so two sessions never overlap.
    pub async fn start(&self, feeder: Feeder) -> broadcast::Receiver<MirrorEvent> {
        let id = feeder.id;
        let running = self.handles().remove(&id);
        if let Some(old) = running {
            old.stop().await;
        }

        let (updates, first) = broadcast::channel(UPDATES);
        let handle = task::spawn(
            Arc::clone(&self.shared),
            feeder,
            updates,
            self.cancel.child_token(),
            &self.tasks,
        );
        // Two starts racing for one id: the loser's task is stopped too.
        let raced = self.handles().insert(id, handle);
        if let Some(other) = raced {
            other.stop().await;
        }
        first
    }

    /// Stop mirroring a feeder and wait until its task has ended; whether
    /// it was running. Its last update is `Link::Offline`, then its
    /// broadcast closes.
    pub async fn stop(&self, id: FeederId) -> bool {
        let handle = self.handles().remove(&id);
        let Some(handle) = handle else {
            return false;
        };
        handle.stop().await;
        true
    }

    /// Set every feeder's clock in `zone` from now on, and give each the
    /// zone's rule: an online feeder at once, the others when they connect.
    pub fn set_zone(&self, zone: Zone) {
        self.shared.zone.send_replace(Some(Arc::new(zone)));
    }

    /// The household's zone, as the feeders keep it; `None` before there
    /// is a household, or when its stored name is no longer known.
    pub fn zone(&self) -> Option<Arc<Zone>> {
        self.shared.zone.borrow().clone()
    }

    /// The feeder's session state, or `None` when it is not mirrored.
    pub fn link(&self, id: FeederId) -> Option<Link> {
        self.handles().get(&id).map(task::Handle::link)
    }

    /// Follow a feeder's updates from now on.
    pub fn subscribe(&self, id: FeederId) -> Option<broadcast::Receiver<MirrorEvent>> {
        self.handles().get(&id).map(task::Handle::subscribe)
    }

    /// Stop every task and wait for them.
    pub async fn shutdown(&self) {
        self.cancel.cancel();
        self.tasks.close();
        self.tasks.wait().await;
    }

    fn handles(&self) -> MutexGuard<'_, HashMap<FeederId, task::Handle>> {
        // Insertions and removals are whole, so a poisoned map is still
        // consistent.
        self.feeders.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
