//! Each mirrored feeder's updates, forwarded into the hub.
//!
//! The mirror broadcasts what a feeder sends once it is stored; the hub
//! stamps it with a stream id beside the service's own updates. A feeder's
//! link is recorded here before the hub hears of it, so the feeder list
//! never shows a state the stream has not sent, and its entry goes when
//! the mirror lets the feeder go.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use wellfed_api::{Connection as Link, FeederId, StreamEvent};
use wellfed_mirror::{Feeder, MirrorEvent};
use wellfed_store::FeederRow;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, info_span, warn};

use crate::{Hub, Result, Service, keys, lock};

impl Service {
    /// Open the feeder's key, start mirroring it, and forward its updates.
    pub(crate) async fn spawn(&self, row: &FeederRow) -> Result<()> {
        let secret = keys::open(&self.shared.config.key, row.id, &row.identity, &row.key)?;
        self.links().insert(row.id, Link::Offline);
        let feeder = Feeder {
            id: row.id,
            identity: row.identity,
            secret,
            address: row.address.clone(),
        };
        lock(&self.fingerprints).insert(row.id, feeder.fingerprint());
        let updates = self.mirror.start(feeder).await;
        if let Some(feeds) = self.mirror.subscribe(row.id) {
            self.watch_feeds(row.id, feeds);
        }
        if let Some(links) = self.mirror.subscribe(row.id) {
            self.watch_revocations(row.id, links);
        }

        let mirror = self.mirror.clone();
        let id = row.id;
        let forward = Forward {
            hub: Arc::clone(&self.shared.hub),
            current: Box::new(move || mirror.link(id)),
            links: Arc::clone(&self.links),
            id,
        };
        // Under the same span as the mirror's task (no parent, as there), so
        // its lines name the feeder and not the request that adopted it.
        let span = info_span!(parent: None, "feeder", id = %id);
        self.tasks.spawn(
            forward
                .run(updates, self.cancel.child_token())
                .instrument(span),
        );
        Ok(())
    }
}

/// The mirror's link for the feeder now, or `None` once it is let go.
type Current = Box<dyn Fn() -> Option<Link> + Send + Sync>;

struct Forward {
    hub: Arc<Hub>,
    current: Current,
    links: Arc<Mutex<HashMap<FeederId, Link>>>,
    id: FeederId,
}

impl Forward {
    async fn run(self, mut updates: Receiver<MirrorEvent>, cancel: CancellationToken) {
        loop {
            let update = tokio::select! {
                biased;
                () = cancel.cancelled() => return,
                update = updates.recv() => update,
            };
            match update {
                Ok(update) => self.publish(update),
                // A client refetches after `resync`, so the backlog is
                // dropped rather than replayed behind it. The link is read
                // after resubscribing: a change after that read still
                // arrives, so the last link published is the current one.
                Err(RecvError::Lagged(missed)) => {
                    warn!(missed, "the stream fell behind the feeder");
                    updates = updates.resubscribe();
                    self.hub.publish(StreamEvent::Resync);
                    if let Some(link) = (self.current)() {
                        self.publish(MirrorEvent::Link(link));
                    }
                }
                Err(RecvError::Closed) => {
                    if (self.current)().is_none() {
                        lock(&self.links).remove(&self.id);
                    }
                    return;
                }
            }
        }
    }

    fn publish(&self, update: MirrorEvent) {
        let feeder = self.id;
        let event = match update {
            MirrorEvent::Event(event) => StreamEvent::Event { feeder, event },
            MirrorEvent::Gap(gap) => StreamEvent::Gap { feeder, gap },
            MirrorEvent::Sample(sample) => StreamEvent::Sample { feeder, sample },
            MirrorEvent::Link(connection) => {
                let previous = lock(&self.links).insert(feeder, connection);
                if previous == Some(connection) {
                    return;
                }
                StreamEvent::Connection { feeder, connection }
            }
        };
        self.hub.publish(event);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::sync::broadcast;
    use uuid::Uuid;

    use super::*;

    const FEEDER: FeederId = FeederId(Uuid::from_u128(1));

    fn connection(event: &StreamEvent) -> Option<Link> {
        match event {
            StreamEvent::Connection { connection, .. } => Some(*connection),
            _ => None,
        }
    }

    #[tokio::test]
    async fn a_lagging_forward_resyncs_then_shows_the_current_link_and_lets_go() {
        let hub = Arc::new(Hub::new(jiff::Timestamp::UNIX_EPOCH));
        let mut stream = hub.subscribe(None);
        let links = Arc::new(Mutex::new(HashMap::new()));
        let current = Arc::new(Mutex::new(Some(Link::Online)));
        let reading = Arc::clone(&current);
        let forward = Forward {
            hub: Arc::clone(&hub),
            current: Box::new(move || *lock(&reading)),
            links: Arc::clone(&links),
            id: FEEDER,
        };

        // One slot, three updates: the forward has lagged before it starts.
        let (sender, updates) = broadcast::channel(1);
        for link in [Link::Connecting, Link::Offline, Link::Connecting] {
            sender.send(MirrorEvent::Link(link)).unwrap();
        }
        let running = tokio::spawn(forward.run(updates, CancellationToken::new()));
        let mut next = async || {
            let stamped = tokio::time::timeout(Duration::from_secs(5), stream.live.recv());
            stamped.await.unwrap().unwrap().event.clone()
        };

        assert_eq!(next().await, StreamEvent::Resync);
        assert_eq!(
            connection(&next().await),
            Some(Link::Online),
            "the current link"
        );
        sender.send(MirrorEvent::Link(Link::Offline)).unwrap();
        assert_eq!(
            connection(&next().await),
            Some(Link::Offline),
            "then live again"
        );
        assert_eq!(lock(&links).get(&FEEDER), Some(&Link::Offline));

        // The mirror lets the feeder go: its broadcast closes.
        *lock(&current) = None;
        drop(sender);
        running.await.unwrap();
        assert_eq!(lock(&links).get(&FEEDER), None);
    }
}
