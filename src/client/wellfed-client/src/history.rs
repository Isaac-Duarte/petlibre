//! The feeding history: each feed the feeder's event history records, as
//! one entry an owner reads (when, who asked, how many portions it asked
//! for and delivered, and how it ended).
//!
//! The feeder keeps no log of its own for feeds. Each feed's end is a `Fed`
//! event in the event ring, which survives reboots and keeps its oldest
//! events until the ring needs their room; a session reading from before
//! that is told by a gap, and events the feeder could not record by a
//! `Lost` event. So the history is a view of the events a session
//! receives, the same for the CLI and the app.
//!
//! An uncertain feed is never shown as a count: the feeder stopped without
//! recording a result (a reset or fault mid-feed), so anything from the
//! portions it confirmed up to what was asked may have dispensed.
//! [`Delivered::Uncertain`] carries both bounds, as JSON too, and no
//! caller can read it as an exact number.

use jiff::Timestamp;
use serde::Serialize;
use wellfed_model::{Event, EventBody, FeedOutcome, FeedReason, FoodLevel, Gap, Pushed, Source};

/// One feed, from its `Fed` event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Feed {
    pub epoch: u32,
    /// The event's number: where a later read resumes.
    pub seq: u64,
    /// The feeder's number for the feed.
    pub feed: u32,
    /// When the event was recorded, by the feeder's clock; `None` while
    /// that was invalid (after a cold boot, before any source set it). A
    /// feed a reset interrupted is recorded when the next boot resolves
    /// it, not when it ran.
    pub at: Option<Timestamp>,
    /// Who asked for it; `None` when that is unknown: a feed a reset
    /// interrupted is resolved at the next boot, and the journal does not
    /// keep who asked unless a plan had claimed it.
    pub by: Option<Source>,
    pub requested: u8,
    pub delivered: Delivered,
    pub outcome: FeedOutcome,
    pub reason: FeedReason,
    /// What the food beam read during the feed; `Unread` for a feed a
    /// reset interrupted, whose reading went with the reset.
    pub hopper: FoodLevel,
}

/// The portions a feed delivered: `{"counted": 3}`, or
/// `{"uncertain": {"confirmed": 0, "requested": 3}}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivered {
    /// The feeder recorded its result: the portions the feedback sensor
    /// confirmed.
    Counted(u8),
    /// No result was recorded: at least `confirmed`, at most `requested`.
    Uncertain { confirmed: u8, requested: u8 },
}

impl Delivered {
    /// What a feed that ended `outcome`, with `completed` of `requested`
    /// portions confirmed, delivered: every reader of a feed's result goes
    /// through here, so none shows an uncertain feed as a count.
    pub const fn of(outcome: FeedOutcome, completed: u8, requested: u8) -> Self {
        match outcome {
            FeedOutcome::Uncertain => Self::Uncertain {
                confirmed: completed,
                requested,
            },
            FeedOutcome::Done
            | FeedOutcome::Cancelled
            | FeedOutcome::Expired
            | FeedOutcome::Failed => Self::Counted(completed),
        }
    }

    /// The portions confirmed and the most that may have dropped: the same
    /// number unless the feed is uncertain, which is never one number.
    pub const fn bounds(self) -> (u8, u8) {
        match self {
            Self::Counted(count) => (count, count),
            Self::Uncertain {
                confirmed,
                requested,
            } => (confirmed, requested),
        }
    }

    /// The portions and the ending in words, the same in every line that
    /// shows a feed's result: `2 of 3 portions, failed`, or for an
    /// uncertain feed `0 to 3 of 3 portions, uncertain`.
    pub fn describe(self, requested: u8, outcome: FeedOutcome) -> String {
        match self {
            Self::Counted(count) => {
                format!("{count} of {requested} portions, {}", outcome.code())
            }
            Self::Uncertain {
                confirmed,
                requested: most,
            } => format!("{confirmed} to {most} of {requested} portions, uncertain"),
        }
    }
}

/// What one pushed message adds to the history.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Entry {
    Feed(Feed),
    /// Events that left the feeder's ring before they were read: any feeds
    /// among them are gone from the feeder.
    Gap(Gap),
    /// The feeder's event queue overflowed and dropped `count` events,
    /// which may have included feeds.
    Lost {
        epoch: u32,
        seq: u64,
        at: Option<Timestamp>,
        count: u16,
    },
}

impl Entry {
    /// The history's entry for `pushed`, if it is a feed's end, a gap or
    /// lost events.
    pub fn from_pushed(pushed: &Pushed) -> Option<Self> {
        match pushed {
            Pushed::Event(Event {
                epoch,
                seq,
                at,
                body: EventBody::Lost { count },
            }) => Some(Self::Lost {
                epoch: *epoch,
                seq: *seq,
                at: *at,
                count: *count,
            }),
            Pushed::Event(event) => Feed::from_event(event).map(Self::Feed),
            Pushed::Gap(gap) => Some(Self::Gap(*gap)),
            Pushed::Sample(_) | Pushed::Log(_) => None,
        }
    }
}

/// Where the history a subscription sends first ends: the newest event the
/// feeder had when asked. Later events may follow at once; a reader of the
/// history stops here, or tells history from news, rather than waiting for a
/// quiet moment after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Backlog {
    /// `Status::last_event` as read before subscribing.
    pub newest: Option<u64>,
}

impl Backlog {
    /// Whether a reader at `after` has it all already.
    pub fn read(self, after: Option<u64>) -> bool {
        self.newest
            .is_none_or(|newest| after.is_some_and(|after| after >= newest))
    }

    /// Whether `pushed` is the backlog's newest event, or past it.
    pub fn ends_at(self, pushed: &Pushed) -> bool {
        let seq = match pushed {
            Pushed::Event(event) => event.seq,
            Pushed::Gap(gap) => gap.to,
            Pushed::Sample(_) | Pushed::Log(_) => return false,
        };
        self.read(Some(seq))
    }
}

impl Feed {
    /// The feed `event` records the end of, if it is a `Fed` event.
    pub fn from_event(event: &Event) -> Option<Self> {
        let EventBody::Fed {
            feed,
            requested,
            completed,
            outcome,
            reason,
            hopper,
            by,
        } = event.body
        else {
            return None;
        };

        let delivered = Delivered::of(outcome, completed, requested);
        // The feeder names itself for an interrupted feed only because the
        // journal lost who asked; a plan that claimed it is named instead.
        let by = match (reason, by) {
            (FeedReason::Interrupted, Source::Feeder) => None,
            (_, by) => Some(by),
        };
        Some(Self {
            epoch: event.epoch,
            seq: event.seq,
            feed,
            at: event.at,
            by,
            requested,
            delivered,
            outcome,
            reason,
            hopper,
        })
    }
}

#[cfg(test)]
mod tests {
    use wellfed_model::{Hex, LogLines};

    use super::*;

    fn fed(outcome: FeedOutcome, completed: u8) -> Event {
        fed_by(
            outcome,
            completed,
            FeedReason::Done,
            Source::Client {
                client: Hex([1, 2, 3, 4]),
            },
        )
    }

    fn fed_by(outcome: FeedOutcome, completed: u8, reason: FeedReason, by: Source) -> Event {
        Event {
            epoch: 3,
            seq: 41,
            at: Some(Timestamp::from_second(1_793_000_000).unwrap()),
            body: EventBody::Fed {
                feed: 9,
                requested: 3,
                completed,
                outcome,
                reason,
                hopper: FoodLevel::Present,
                by,
            },
        }
    }

    #[test]
    fn a_recorded_feed_reads_as_its_count() {
        let feed = Feed::from_event(&fed(FeedOutcome::Done, 3)).unwrap();

        assert_eq!(feed.delivered, Delivered::Counted(3));
        assert_eq!((feed.epoch, feed.seq, feed.feed), (3, 41, 9));
        assert_eq!(feed.requested, 3);
        let client = Source::Client {
            client: Hex([1, 2, 3, 4]),
        };
        assert_eq!(feed.by, Some(client));
        assert!(feed.at.is_some());

        let failed = Feed::from_event(&fed(FeedOutcome::Failed, 1)).unwrap();
        assert_eq!(failed.delivered, Delivered::Counted(1));
    }

    #[test]
    fn an_uncertain_feed_reads_as_a_range_never_a_count() {
        let feed = Feed::from_event(&fed(FeedOutcome::Uncertain, 1)).unwrap();

        assert_eq!(
            feed.delivered,
            Delivered::Uncertain {
                confirmed: 1,
                requested: 3
            }
        );
        assert_eq!(feed.outcome, FeedOutcome::Uncertain);
    }

    #[test]
    fn a_result_reads_as_a_count_unless_uncertain() {
        let failed = Delivered::of(FeedOutcome::Failed, 2, 3);
        let uncertain = Delivered::of(FeedOutcome::Uncertain, 2, 3);

        assert_eq!(failed, Delivered::Counted(2));
        assert_eq!(
            failed.describe(3, FeedOutcome::Failed),
            "2 of 3 portions, failed"
        );
        assert_eq!(
            uncertain.describe(3, FeedOutcome::Uncertain),
            "2 to 3 of 3 portions, uncertain"
        );
    }

    #[test]
    fn an_interrupted_feed_has_no_known_requester_unless_a_plan_claimed_it() {
        let interrupted = |by| fed_by(FeedOutcome::Uncertain, 0, FeedReason::Interrupted, by);

        let unknown = Feed::from_event(&interrupted(Source::Feeder)).unwrap();
        assert_eq!(unknown.by, None);
        let planned = Feed::from_event(&interrupted(Source::Schedule { plan: 2 })).unwrap();
        assert_eq!(planned.by, Some(Source::Schedule { plan: 2 }));
    }

    #[test]
    fn delivered_portions_say_which_kind_in_json() {
        let counted = serde_json::to_value(Delivered::Counted(3)).unwrap();
        let uncertain = serde_json::to_value(Delivered::Uncertain {
            confirmed: 0,
            requested: 3,
        })
        .unwrap();

        assert_eq!(counted, serde_json::json!({"counted": 3}));
        assert_eq!(
            uncertain,
            serde_json::json!({"uncertain": {"confirmed": 0, "requested": 3}})
        );
        let feed = Feed::from_event(&fed(FeedOutcome::Uncertain, 0)).unwrap();
        let json = serde_json::to_value(Entry::Feed(feed)).unwrap();
        assert_eq!(json["kind"], "feed");
        assert_eq!(json["delivered"], uncertain);
    }

    #[test]
    fn only_feeds_gaps_and_lost_events_are_entries() {
        let gap = Gap {
            epoch: 3,
            from: 1,
            to: 40,
        };
        let boot = Event {
            epoch: 3,
            seq: 42,
            at: None,
            body: EventBody::Boot {
                bank: 1,
                reset_flags: 0,
            },
        };
        let lost = Event {
            body: EventBody::Lost { count: 2 },
            ..boot
        };

        assert_eq!(Entry::from_pushed(&Pushed::Gap(gap)), Some(Entry::Gap(gap)));
        assert_eq!(Entry::from_pushed(&Pushed::Event(boot)), None);
        assert_eq!(
            Entry::from_pushed(&Pushed::Event(lost)),
            Some(Entry::Lost {
                epoch: 3,
                seq: 42,
                at: None,
                count: 2
            })
        );
        assert!(matches!(
            Entry::from_pushed(&Pushed::Event(fed(FeedOutcome::Done, 3))),
            Some(Entry::Feed(_))
        ));
        let log = Pushed::Log(LogLines {
            seq: 1,
            dropped: 0,
            lines: Vec::new(),
        });
        assert_eq!(Entry::from_pushed(&log), None);
    }
}
