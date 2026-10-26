//! A subscriber's live samples waiting to go out: one of each kind, so a
//! burst of kinds taken in the same pass all reach it, and a kind not sent
//! before its next sample is replaced by it rather than queued behind it.

use wellfed_proto::api::{Metric, Sample};

/// One per [`Metric`] variant.
const KINDS: usize = 6;

/// Where each kind waits. No wildcard: a new metric needs its own slot.
const fn kind(metric: &Metric) -> usize {
    match metric {
        Metric::Power(_) => 0,
        Metric::Rssi(_) => 1,
        Metric::Heap { .. } => 2,
        Metric::Loop { .. } => 3,
        Metric::Gateway { .. } => 4,
        Metric::Wifi { .. } => 5,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Samples([Option<Sample>; KINDS]);

impl Samples {
    pub(super) const EMPTY: Self = Self([None; KINDS]);

    /// Keep `sample`, replacing an unsent one of its kind.
    pub(super) fn put(&mut self, sample: Sample) {
        if let Some(slot) = self.0.get_mut(kind(&sample.metric)) {
            *slot = Some(sample);
        }
    }

    /// The oldest waiting sample, so no kind starves another.
    pub(super) fn take(&mut self) -> Option<Sample> {
        let oldest = self
            .0
            .iter_mut()
            .filter(|slot| slot.is_some())
            .min_by_key(|slot| slot.map_or(0, |sample| sample.at_ms))?;
        oldest.take()
    }
}
