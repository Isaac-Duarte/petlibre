//! Reading one feed's trace a page at a time: which `FeedTrace` op asks for
//! the next page, and the whole trace once every page is in.
//!
//! A trace's body travels in pages of at most half a kilobyte, and each
//! page repeats the trace's summary. A trace that changed between two
//! pages (the feeder's ring reused its slot, or the RAM trace was a newer
//! feed's) reads as [`TraceError::Changed`], never as a body spliced from
//! two traces; asking again from the start gets the trace the feeder holds
//! now. Reads change nothing on the feeder, so any page may be asked
//! again.

use wellfed_model::{Answer, Malformed, Op, Refusal, Trace, TracePage, TraceSummary};

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TraceError {
    #[error("the feeder refused the trace: {0:?}")]
    Refused(Refusal),
    #[error("the feeder answered something other than a trace page")]
    Unexpected(Box<Answer>),
    #[error("the trace changed while it was read; read it again")]
    Changed,
    #[error("the feeder's page ended before the trace did")]
    Short,
    #[error(transparent)]
    Malformed(#[from] Malformed),
}

/// A whole trace as the feeder sent it, its body in the feeder's own
/// encoding, for a host that keeps it as sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawTrace {
    pub summary: TraceSummary,
    pub stored: bool,
    pub body: Vec<u8>,
}

/// One trace being read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pages {
    feed: u32,
    /// The first page's summary and body length, which every later page
    /// must repeat.
    first: Option<(TraceSummary, u16)>,
    body: Vec<u8>,
}

impl Pages {
    pub const fn new(feed: u32) -> Self {
        Self {
            feed,
            first: None,
            body: Vec::new(),
        }
    }

    /// The op that asks for the next page.
    pub fn next_op(&self) -> Op {
        Op::FeedTrace {
            feed: self.feed,
            offset: u16::try_from(self.body.len()).unwrap_or(u16::MAX),
        }
    }

    /// Take the answer to [`Self::next_op`]: the whole trace once its last
    /// page is in, `None` while more pages remain.
    pub fn take(&mut self, answer: Answer) -> Result<Option<Trace>, TraceError> {
        let Some(raw) = self.take_raw(answer)? else {
            return Ok(None);
        };
        Ok(Some(Trace::decode(raw.summary, raw.stored, &raw.body)?))
    }

    /// As [`Self::take`], with the body left in the feeder's encoding,
    /// unchecked: [`Trace::decode`] reads it.
    pub fn take_raw(&mut self, answer: Answer) -> Result<Option<RawTrace>, TraceError> {
        let page = match answer {
            Answer::FeedTrace(page) => page,
            Answer::Refused(refusal) => return Err(TraceError::Refused(refusal)),
            other => return Err(TraceError::Unexpected(Box::new(other))),
        };
        let TracePage {
            summary,
            stored,
            offset,
            total,
            bytes,
        } = page;

        // A trace the feeder stored meanwhile is the same trace, so only
        // the summary and the length must hold.
        let first = *self.first.get_or_insert((summary, total));
        if first != (summary, total)
            || summary.feed != self.feed
            || usize::from(offset) != self.body.len()
        {
            return Err(TraceError::Changed);
        }
        self.body.extend_from_slice(&bytes);
        let have = self.body.len();
        if have > usize::from(total) {
            return Err(TraceError::Changed);
        }
        if have < usize::from(total) {
            return if bytes.is_empty() {
                Err(TraceError::Short)
            } else {
                Ok(None)
            };
        }
        Ok(Some(RawTrace {
            summary,
            stored,
            body: std::mem::take(&mut self.body),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wellfed_model::{CurrentUnit, FeedOutcome, FeedReason};

    fn summary(drive_ms: u32) -> TraceSummary {
        TraceSummary {
            feed: 3,
            requested: 1,
            completed: 1,
            outcome: FeedOutcome::Done,
            reason: FeedReason::Done,
            drive_ms,
            samples: 2,
            unit: CurrentUnit::Scaled,
            current_peak: 70,
            current_mean: Some(65),
            rising: 0,
            falling: 0,
            first_edge_ms: None,
            beam_breaks: 0,
            reversals: 0,
            battery_rest_mv: None,
            battery_min_mv: None,
            stride: 1,
            truncated: false,
        }
    }

    /// Two current readings, 60 at 0 ms and 70 at 30 ms: four bytes.
    const BODY: [u8; 4] = [0x00, 0x78, 0x1e, 0x14];

    fn page(drive_ms: u32, offset: u16, bytes: &[u8]) -> Answer {
        Answer::FeedTrace(TracePage {
            summary: summary(drive_ms),
            stored: offset > 0,
            offset,
            total: 4,
            bytes: bytes.to_vec(),
        })
    }

    #[test]
    fn pages_join_into_the_trace() {
        let mut pages = Pages::new(3);
        assert_eq!(pages.next_op(), Op::FeedTrace { feed: 3, offset: 0 });
        assert_eq!(pages.take(page(9, 0, &BODY[..2])), Ok(None));
        assert_eq!(pages.next_op(), Op::FeedTrace { feed: 3, offset: 2 });
        let trace = pages.take(page(9, 2, &BODY[2..])).unwrap().unwrap();
        assert!(trace.stored, "stored by the last page");
        assert_eq!(
            trace.entries,
            [
                wellfed_model::TraceEntry::Current {
                    at_ms: 0,
                    value: 60
                },
                wellfed_model::TraceEntry::Current {
                    at_ms: 30,
                    value: 70
                },
            ]
        );
    }

    #[test]
    fn a_trace_that_changed_between_pages_is_not_spliced() {
        let mut pages = Pages::new(3);
        pages.take(page(9, 0, &BODY[..2])).unwrap();
        assert_eq!(
            pages.take(page(10, 2, &BODY[2..])),
            Err(TraceError::Changed)
        );

        let mut pages = Pages::new(3);
        pages.take(page(9, 0, &BODY[..2])).unwrap();
        assert_eq!(pages.take(page(9, 1, &BODY[1..])), Err(TraceError::Changed));

        let mut pages = Pages::new(4);
        assert_eq!(pages.take(page(9, 0, &BODY)), Err(TraceError::Changed));
    }

    #[test]
    fn an_empty_page_short_of_the_end_or_another_answer_ends_the_read() {
        let mut pages = Pages::new(3);
        assert_eq!(pages.take(page(9, 0, &[])), Err(TraceError::Short));
        let mut pages = Pages::new(3);
        let answer = Answer::Done { generation: None };
        assert_eq!(
            pages.take(answer.clone()),
            Err(TraceError::Unexpected(Box::new(answer)))
        );
    }
}
