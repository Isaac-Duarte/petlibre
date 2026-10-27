//! Ingest: what the feeder pushes, stored as it sent it and then
//! broadcast. An event is stored (with its projection and the cursor, in
//! one transaction) before it is broadcast, and a store failure ends the
//! session, so the reconnect replays whatever was not stored.
//!
//! Event bodies and sample values are kept in the feeder's own encoding
//! (postcard of the wire types), and [`stored_event`] is the way back. The
//! store projects the decoded event it is handed, and rebuilds its
//! projections from the stored bodies through [`Mirror::refresh_projections`].

use wellfed_api::{FeederEvent, FeederSample, Gap};
use wellfed_model::{Event, EventBody, Pushed};
use wellfed_proto::api as proto;
use wellfed_store::{EventRow, NewEvent, NewSample, Refresh};
use tracing::{info, warn};

use crate::session::{End, Failure};
use crate::task::Task;
use crate::{Mirror, MirrorEvent};

/// Room for an encoded event body (16 bytes on the feeder) or sample.
const RAW: usize = 32;

/// A stored event of `epoch` as the feeder sent it, or `None` when its
/// body does not decode.
pub fn stored_event(epoch: u32, row: &EventRow) -> Option<FeederEvent> {
    let event = Event {
        epoch,
        seq: row.seq,
        at: row.at,
        body: stored_body(&row.body)?,
    };
    Some(FeederEvent::new(event, row.received))
}

/// A stored body as the event it encodes, or `None` when it does not
/// decode.
pub(crate) fn stored_body(raw: &[u8]) -> Option<EventBody> {
    let body: proto::EventBody = wellfed_proto::decode(raw).ok()?;
    Some(EventBody::from(body))
}

impl Mirror {
    /// Rebuild the store's projections from the stored events when the
    /// projection code changed since they were built. Call it before
    /// starting any feeder, so ingest never writes beside a stale build.
    pub async fn refresh_projections(&self) -> wellfed_store::Result<Refresh> {
        let refresh = self.shared.store.refresh_projections(stored_body).await?;
        if let Refresh::Rebuilt { events, undecoded } = refresh {
            info!(events, "projections rebuilt from the stored events");
            if undecoded > 0 {
                warn!(
                    undecoded,
                    "stored events that do not decode project nothing"
                );
            }
        }
        Ok(refresh)
    }
}

impl Task {
    /// Store what the feeder pushed, then broadcast it.
    pub(crate) async fn apply(&self, pushed: Pushed) -> Result<(), End> {
        let store = &self.shared.store;
        let received = self.shared.clock.now();
        let mut raw = [0u8; RAW];
        let update = match pushed {
            Pushed::Event(event) => {
                let len = wellfed_proto::encode(&proto::EventBody::from(event.body), &mut raw)
                    .map_err(|_| Failure::Encode)
                    .map_err(End::Failed)?;
                let new = store
                    .record_event(
                        self.id,
                        NewEvent {
                            epoch: event.epoch,
                            seq: event.seq,
                            at: event.at,
                            received,
                            body: event.body,
                            raw: &raw[..len],
                        },
                    )
                    .await?;
                if !new {
                    return Ok(());
                }
                MirrorEvent::Event(FeederEvent::new(event, received))
            }
            Pushed::Gap(gap) => {
                let (epoch, from, to) = (gap.epoch, gap.from, gap.to);
                warn!(from, to, "events lost before the server read them");
                store.record_gap(self.id, epoch, from, to, received).await?;
                MirrorEvent::Gap(Gap {
                    epoch,
                    from,
                    to,
                    received,
                })
            }
            Pushed::Sample(sample) => {
                let len = wellfed_proto::encode(&proto::Metric::from(sample.value), &mut raw)
                    .map_err(|_| Failure::Encode)
                    .map_err(End::Failed)?;
                store
                    .record_sample(
                        self.id,
                        NewSample {
                            received,
                            uptime_ms: sample.uptime_ms,
                            metric: sample.value.metric(),
                            value: &raw[..len],
                        },
                    )
                    .await?;
                MirrorEvent::Sample(FeederSample::new(sample, received))
            }
            // The server never asks for the log, so none comes.
            Pushed::Log(_) => return Ok(()),
        };
        // No subscriber is fine: the store holds it.
        let _ = self.updates.send(update);
        Ok(())
    }
}
