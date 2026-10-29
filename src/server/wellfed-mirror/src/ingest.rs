//! Ingest: what the feeder pushes, stored as it sent it and broadcast. An
//! event is stored (with its projection and the cursor, in one
//! transaction) before it is broadcast, and a store failure ends the
//! session, so the reconnect replays whatever was not stored.
//!
//! Samples are broadcast at once and written in small batches: a batch is
//! written when it holds [`SAMPLE_BATCH`] samples, when its oldest has
//! waited [`Timing::sample_wait`], and when the session ends. The feeder never
//! replays a sample, so the batch is all that a failed write loses; it
//! ends the session like any store failure.
//!
//! Event bodies and sample values are kept in the feeder's own encoding
//! (postcard of the wire types), and [`stored_event`] and [`stored_sample`]
//! are the way back. The store projects the decoded event it is handed and
//! numbers the decoded sample, and rebuilds both from the stored bytes
//! through [`Mirror::refresh_projections`] and [`Mirror::refresh_samples`].
//!
//! [`Timing::sample_wait`]: crate::Timing::sample_wait

use std::sync::Arc;

use tokio::time::Instant;
use tracing::{info, warn};
use wellfed_api::{FeederEvent, FeederSample, Gap, SampleValue};
use wellfed_model::{Event, EventBody, Pushed};
use wellfed_proto::api as proto;
use wellfed_store::{EventRow, NewEvent, NewSample, Refresh, SampleRefresh};

use crate::session::{End, Failure};
use crate::task::Task;
use crate::{Mirror, MirrorEvent};

/// Room for an encoded event body (16 bytes on the feeder) or sample.
const RAW: usize = 32;

/// The most samples a batch holds before it is written.
pub(crate) const SAMPLE_BATCH: usize = 32;

/// The samples received and not yet written, and when they must be.
#[derive(Debug, Default)]
pub(crate) struct Batch {
    samples: Vec<NewSample>,
    due: Option<Instant>,
}

impl Batch {
    /// When the batch must be written; `None` while it is empty.
    pub(crate) const fn due(&self) -> Option<Instant> {
        self.due
    }
}

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

/// A stored sample's bytes as the value they encode, or `None` when they
/// do not decode.
pub fn stored_sample(raw: &[u8]) -> Option<SampleValue> {
    let metric: proto::Metric = wellfed_proto::decode(raw).ok()?;
    Some(SampleValue::from(metric))
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

    /// Number the stored samples again when what a number is changed
    /// since they were numbered, then bring the rollups up to date. Call
    /// it before starting any feeder, like [`Self::refresh_projections`].
    pub async fn refresh_samples(&self) -> wellfed_store::Result<SampleRefresh> {
        let store = &self.shared.store;
        let refresh = store.refresh_samples(stored_sample).await?;
        if let SampleRefresh::Decoded { samples, undecoded } = refresh
            && samples > 0
        {
            info!(samples, "stored samples numbered again");
            if undecoded > 0 {
                warn!(
                    undecoded,
                    "stored samples that do not decode have no number"
                );
            }
            store.roll_up_samples().await?;
        }
        Ok(refresh)
    }
}

impl Task {
    /// Store what the feeder pushed, or batch it if a sample, then
    /// broadcast it.
    pub(crate) async fn apply(&mut self, pushed: Pushed) -> Result<(), End> {
        let shared = Arc::clone(&self.shared);
        let store = &shared.store;
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
                // The session loop reads them again after this step: an
                // event may arrive inside another request's wait, where a
                // request of the task's own cannot go out.
                if self.changes_plans(&event.body) {
                    self.plans_stale = true;
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
                let batch = &mut self.batch;
                batch.samples.push(NewSample {
                    received,
                    uptime_ms: sample.uptime_ms,
                    value: sample.value,
                    raw: raw[..len].to_vec(),
                });
                let wait = shared.config.timing.sample_wait;
                batch.due.get_or_insert_with(|| Instant::now() + wait);
                if batch.samples.len() >= SAMPLE_BATCH {
                    self.write_samples().await?;
                }
                MirrorEvent::Sample(FeederSample::new(sample, received))
            }
            // This session never asks for the log: a tail has a session
            // of its own (`log.rs`).
            Pushed::Log(_) => return Ok(()),
        };
        // No subscriber is fine: the store holds it.
        let _ = self.updates.send(update);
        Ok(())
    }

    /// Write the batch of samples, emptying it whether or not the write
    /// succeeds: the feeder will not send them again.
    pub(crate) async fn write_samples(&mut self) -> Result<(), End> {
        let batch = std::mem::take(&mut self.batch);
        if batch.samples.is_empty() {
            return Ok(());
        }
        self.shared
            .store
            .record_samples(self.id, &batch.samples)
            .await?;
        Ok(())
    }
}
