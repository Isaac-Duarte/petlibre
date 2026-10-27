//! Each mirrored feeder's updates, forwarded into the hub.
//!
//! The mirror broadcasts what a feeder sends once it is stored; the hub
//! stamps it with a stream id beside the service's own updates. A feeder's
//! link is recorded here before the hub hears of it, so the feeder list
//! never shows a state the stream has not sent.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use wellfed_api::{Connection as Link, FeederId, StreamEvent};
use wellfed_mirror::{Feeder, Mirror, MirrorEvent};
use wellfed_store::FeederRow;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::{Result, Service, Shared, keys, lock};

impl Service {
    /// Open the feeder's key, start mirroring it, and forward its updates.
    pub(crate) fn spawn(&self, row: &FeederRow) -> Result<()> {
        let secret = keys::open(&self.shared.config.key, row.id, &row.identity, &row.key)?;
        self.links().insert(row.id, Link::Offline);
        let updates = self.mirror.start(Feeder {
            id: row.id,
            identity: row.identity,
            secret,
            address: row.address.clone(),
        });
        let forward = Forward {
            shared: Arc::clone(&self.shared),
            mirror: self.mirror.clone(),
            links: Arc::clone(&self.links),
            id: row.id,
        };
        self.tasks
            .spawn(forward.run(updates, self.cancel.child_token()));
        Ok(())
    }
}

struct Forward {
    shared: Arc<Shared>,
    mirror: Mirror,
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
                // Whatever was missed is in the store; a client refetches,
                // and the link is read again.
                Err(RecvError::Lagged(missed)) => {
                    warn!(feeder = %self.id, missed, "the stream fell behind the feeder");
                    self.shared.hub.publish(StreamEvent::Resync);
                    if let Some(link) = self.mirror.link(self.id) {
                        self.publish(MirrorEvent::Link(link));
                    }
                }
                Err(RecvError::Closed) => return,
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
        self.shared.hub.publish(event);
    }
}
