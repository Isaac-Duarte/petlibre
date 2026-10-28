//! The collector: each feeder's feed traces and statistics, read on the
//! long lane and stored beside its history.
//!
//! - **Traces.** After each `Fed` event (and at each connect, and with each
//!   statistics read) it looks for the feeds of the session's epoch with no
//!   trace stored, among the [`TRACE_WINDOW`] highest-numbered: the feeder
//!   keeps only its last 8-12 traces, so an older feed is never asked for.
//!   Each is read a page at a time and stored whole. A feeder that has no
//!   trace for a feed says so, and that answer is stored instead, so the
//!   feed is not asked for again; anything else (no answer, a trace that
//!   changed under the read, the session ending) leaves the feed to the
//!   next look. Reads change nothing on the feeder, so asking again is
//!   always safe.
//! - **Statistics.** Every section the session's capabilities allow, at
//!   each connect and then every [`Diagnostics::Read`]'s `stats_every`, and
//!   the food section alone after each `Fed` event, ahead of its trace: the
//!   food beam reaches the server only through it, so an emptied or
//!   refilled hopper shows one feed late at most, not one period. A
//!   section the feeder refuses (memory is an owner's, and busy during a
//!   feed) keeps its last read.
//!
//! Each runs on the long lane, so a step goes out only with no short
//! command in flight or waiting: a feed or a status read waits at most one
//! step. The collector holds the long-operation turn for one trace, or one
//! round of statistics, at a time.
//!
//! [`Diagnostics::Read`]: crate::Diagnostics::Read

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, watch};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use wellfed_api::{Connection as Link, FeederId, FeederRefusal};
use wellfed_client::trace::{Pages, TraceError};
use wellfed_model::{Answer, EventBody, Op, Refusal, StatsSection, Trace};
use wellfed_store::{RawTrace, TraceFound};

use crate::task::{LongLane, Opened};
use crate::{AskError, MirrorEvent, Operation, Shared};

/// How many of an epoch's highest-numbered feeds may still be missing a
/// trace: more than the feeder's ring keeps, so none it holds is skipped.
pub(crate) const TRACE_WINDOW: u32 = 16;

/// How many times a trace that changed while it was read is read again
/// from its start before the feed is left to the next look.
const REREADS: usize = 2;

/// What is due next.
#[derive(Clone, Copy, Debug, Default)]
struct Due {
    traces: bool,
    stats: bool,
    /// The food section alone.
    food: bool,
}

/// Which statistics sections a round reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Round {
    Every,
    Food,
}

pub(crate) struct Collector {
    pub(crate) shared: Arc<Shared>,
    pub(crate) id: FeederId,
    pub(crate) long: LongLane,
    pub(crate) opened: watch::Receiver<Option<Opened>>,
    pub(crate) updates: broadcast::Receiver<MirrorEvent>,
    pub(crate) stats_every: Duration,
    pub(crate) cancel: CancellationToken,
}

impl Collector {
    pub(crate) async fn run(mut self) {
        let mut due = Due::default();
        let mut next_stats = Instant::now() + self.stats_every;
        loop {
            let online = *self.long.link.borrow() == Link::Online;
            if online && (due.traces || due.stats || due.food) {
                let work = std::mem::take(&mut due);
                let cancel = self.cancel.clone();
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => return,
                    () = self.collect(work) => {}
                }
            }

            tokio::select! {
                biased;
                () = self.cancel.cancelled() => return,
                update = self.updates.recv() => match update {
                    Ok(MirrorEvent::Event(event)) => {
                        if matches!(event.body, EventBody::Fed { .. }) {
                            due.traces = true;
                            due.food = true;
                        }
                    }
                    Ok(MirrorEvent::Link(Link::Online)) => {
                        due = Due { traces: true, stats: true, food: false };
                    }
                    Ok(_) => {}
                    // A `Fed` event may be among those missed: the store
                    // knows.
                    Err(RecvError::Lagged(_)) => {
                        due.traces = true;
                        due.food = true;
                    }
                    Err(RecvError::Closed) => return,
                },
                () = tokio::time::sleep_until(next_stats) => {
                    next_stats = Instant::now() + self.stats_every;
                    due = Due { traces: true, stats: true, food: false };
                }
            }
        }
    }

    async fn collect(&self, due: Due) {
        let Some(opened) = *self.opened.borrow() else {
            return;
        };
        let capabilities = opened.capabilities;
        if due.food && !due.stats && capabilities.stats {
            self.stats(opened, Round::Food).await;
        }
        // A feeder serves traces only beside the statistics, and one from
        // before either would end the session on an op it does not know.
        if due.traces && capabilities.stats && capabilities.traces {
            self.traces(opened.epoch).await;
        }
        if due.stats && capabilities.stats {
            self.stats(opened, Round::Every).await;
        }
    }

    /// Fetch and store the trace of every recent feed of `epoch` that has
    /// none stored, newest first; stop at the first step with no answer.
    async fn traces(&self, epoch: u32) {
        let feeds = match self
            .shared
            .store
            .untraced_feeds(self.id, epoch, TRACE_WINDOW)
            .await
        {
            Ok(feeds) => feeds,
            Err(error) => {
                warn!("find the feeds with no trace: {error}");
                return;
            }
        };

        for feed in feeds {
            let Ok(mut operation) = self
                .long
                .clone()
                .operation(self.answer(), Arc::clone(&self.shared.random))
                .await
            else {
                return;
            };
            let found = match self.trace(&mut operation, feed).await {
                Ok(Some(found)) => found,
                Ok(None) => continue,
                Err(error) => {
                    debug!(feed, "trace read stopped: {error}");
                    return;
                }
            };
            drop(operation);

            let now = self.shared.clock.now();
            if let Err(error) = self
                .shared
                .store
                .record_trace(self.id, epoch, feed, now, &found)
                .await
            {
                warn!(feed, "store the trace: {error}");
                return;
            }
            debug!(feed, "trace stored");
        }
    }

    /// Read one feed's trace: what to store, or `None` to leave it for the
    /// next look.
    async fn trace(
        &self,
        operation: &mut Operation,
        feed: u32,
    ) -> Result<Option<TraceFound>, AskError> {
        for _ in 0..=REREADS {
            let mut pages = Pages::new(feed);
            let read = loop {
                let answer = operation.step(self.request()?, pages.next_op()).await?;
                match pages.take_raw(answer) {
                    Ok(None) => {}
                    Ok(Some(raw)) => break Ok(raw),
                    Err(error) => break Err(error),
                }
            };
            match read {
                Ok(raw) => {
                    if Trace::decode(raw.summary, raw.stored, &raw.body).is_err() {
                        warn!(feed, "the feeder's trace does not decode; not stored");
                        return Ok(None);
                    }
                    return Ok(Some(TraceFound::Trace(RawTrace {
                        summary: raw.summary,
                        stored: raw.stored,
                        body: raw.body,
                    })));
                }
                Err(TraceError::Changed) => {}
                Err(TraceError::Refused(Refusal::Busy)) => return Ok(None),
                Err(TraceError::Refused(refusal)) => {
                    info!(feed, "the feeder has no trace of the feed: {refusal:?}");
                    return Ok(Some(TraceFound::Missing(FeederRefusal::from(refusal))));
                }
                Err(error) => {
                    warn!(feed, "trace not read: {error}");
                    return Ok(None);
                }
            }
        }
        debug!(feed, "the trace kept changing while it was read");
        Ok(None)
    }

    /// Read and store the round's sections the session serves, on one
    /// turn.
    async fn stats(&self, opened: Opened, round: Round) {
        let capabilities = opened.capabilities;
        let sections = StatsSection::ALL
            .into_iter()
            .filter(|section| round == Round::Every || *section == StatsSection::Food)
            .filter(|section| match section {
                StatsSection::Feeds => capabilities.traces,
                StatsSection::Portions => capabilities.calibration,
                StatsSection::Counters
                | StatsSection::Memory
                | StatsSection::Flash
                | StatsSection::Power
                | StatsSection::Food => true,
            });
        let Ok(mut operation) = self
            .long
            .clone()
            .operation(self.answer(), Arc::clone(&self.shared.random))
            .await
        else {
            return;
        };

        for section in sections {
            let Ok(request) = self.request() else {
                return;
            };
            let stats = match operation.step(request, Op::Stats { section }).await {
                Ok(Answer::Stats(stats)) => stats,
                Ok(Answer::Refused(refusal)) => {
                    debug!(?section, "statistics refused: {refusal:?}");
                    continue;
                }
                Ok(other) => {
                    warn!(?section, "unexpected answer to the statistics: {other:?}");
                    continue;
                }
                Err(error) => {
                    debug!(?section, "statistics read stopped: {error}");
                    return;
                }
            };

            let now = self.shared.clock.now();
            if let Err(error) = self
                .shared
                .store
                .record_stats(self.id, now, opened.boot_count, &stats)
                .await
            {
                warn!(?section, "store the statistics: {error}");
                return;
            }
        }
    }

    fn answer(&self) -> Duration {
        self.shared.config.timing.answer
    }

    /// A fresh request id. Without randomness the read cannot go out, and
    /// nothing is lost by leaving it for the next look.
    fn request(&self) -> Result<crate::RequestId, AskError> {
        self.shared.random().map_err(|_| AskError::Unsendable)
    }
}
