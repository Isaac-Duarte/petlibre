//! Manual feeding in the loop: whether a feed can start now, how a feed is
//! queued (a Dispense press and a client's `Feed` alike).
//!
//! **Readiness** is one decision ([`readiness`]): the feeding owner's own
//! reason not to feed ([`Feeds::blocked`]), then BLE setup mode (it ends in
//! a restart, which would leave a feed uncertain) and an install (it holds
//! the flash), then the brownout hold ([`super::supply`]). The owner's own
//! reasons include the battery pack below its floor while the adapter is
//! out (`Blocked::Battery`). The status reports it, a feed is asked for only while it is
//! ready, and the owner's own checks stay behind it for a feed that lost a
//! race (a join that began in between). A change of a lasting reason
//! ([`Blocked::lasting`]) is recorded in the history as a `FeedingBlocked`
//! event; every change goes to the log.
//!
//! **One path.** A Dispense press asks for one portion, as stock's does,
//! through [`queue`], which a client's `Feed` also takes: the same
//! readiness, the same owner, the same `Fed` event (by the panel).
//!
//! **Silent.** A feed plays no tune; that is the owner's choice. A tune a
//! client asks for with `play` is stepped after the owner and the device
//! API in each pass, so a speaker that is busy, will not open or stalls
//! never delays a feed's start, its stop or its terminal result.

use core::fmt;

use wellfed_proto::api::{Blocked, EventBody, Readiness, Refusal, Source};

use super::Feeds;
use super::events;
use super::platform::Platform;

/// What a Dispense press asks for: one portion, as stock.
pub const DISPENSE_PORTIONS: u8 = 1;

/// Whether a feed can start now. The owner's lasting reasons come first,
/// since they outlast setup mode and an install; then those two; then the
/// owner's passing ones; then a brownout's `battery_hold`.
pub fn readiness<F: Feeds>(
    feeds: &F,
    setup: bool,
    installing: bool,
    battery_hold: bool,
) -> Readiness {
    let blocked = match feeds.blocked() {
        Some(blocked) if blocked.lasting() || blocked == Blocked::Unserved => Some(blocked),
        _ if setup => Some(Blocked::Setup),
        _ if installing => Some(Blocked::Updating),
        None if battery_hold => Some(Blocked::Battery),
        owner => owner,
    };
    blocked.map_or(Readiness::Ready, Readiness::Blocked)
}

/// `ready`, unless a plan's claimed feed must be the next one (`meal`):
/// then a feed is being prepared.
pub const fn held(ready: Readiness, meal: bool) -> Readiness {
    match ready {
        Readiness::Ready if meal => Readiness::Blocked(Blocked::Feeding),
        ready => ready,
    }
}

/// Queue one feed of `portions` for `by` while `ready` allows it. The
/// number the journal holds the feed under, or why it was refused.
pub fn queue<F: Feeds>(
    feeds: &mut F,
    ready: Readiness,
    portions: u8,
    by: Source,
) -> Result<u32, Refusal> {
    if let Readiness::Blocked(blocked) = ready {
        return Err(blocked.refusal());
    }
    feeds.submit(portions, by)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    /// Readiness changed.
    Readiness(Readiness),
    /// A Dispense press asked for a feed.
    Dispense { result: Result<u32, Refusal> },
}

/// One record's body (`PLFEED1`).
pub fn describe(record: Record, f: &mut impl fmt::Write) -> fmt::Result {
    match record {
        Record::Readiness(Readiness::Ready) => f.write_str("readiness state=ready"),
        Record::Readiness(Readiness::Blocked(blocked)) => {
            write!(
                f,
                "readiness state=blocked reason={}",
                blocked_name(blocked)
            )
        }
        Record::Dispense { result: Ok(feed) } => write!(f, "dispense feed={feed}"),
        Record::Dispense {
            result: Err(refusal),
        } => {
            write!(f, "dispense refused={}", refusal_name(refusal))
        }
    }
}

/// The last readiness logged and the last lasting reason recorded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Watch {
    logged: Option<Readiness>,
    lasting: Option<Blocked>,
}

impl Watch {
    pub const fn new() -> Self {
        Self {
            logged: None,
            lasting: None,
        }
    }

    /// Note `now`: a changed readiness is logged, and a changed lasting
    /// reason recorded as an event. Each boot starts with none recorded, so
    /// a reason that holds at boot is recorded again after the `Boot`.
    pub fn note<P: Platform>(
        &mut self,
        platform: &mut P,
        now_ms: u32,
        now: Readiness,
        mut record: impl FnMut(Record),
    ) {
        if self.logged != Some(now) {
            self.logged = Some(now);
            record(Record::Readiness(now));
        }
        let lasting = match now {
            Readiness::Blocked(blocked) if blocked.lasting() => Some(blocked),
            Readiness::Ready | Readiness::Blocked(_) => None,
        };
        if lasting != self.lasting {
            self.lasting = lasting;
            events::record(
                platform,
                now_ms,
                EventBody::FeedingBlocked { reason: lasting },
            );
        }
    }
}

const fn blocked_name(blocked: Blocked) -> &'static str {
    match blocked {
        Blocked::Starting => "starting",
        Blocked::Journal => "journal",
        Blocked::Fault => "fault",
        Blocked::Resolving => "resolving",
        Blocked::Feeding => "feeding",
        Blocked::Joining => "joining",
        Blocked::Updating => "updating",
        Blocked::Setup => "setup",
        Blocked::Unserved => "unserved",
        Blocked::Battery => "battery",
    }
}

pub(super) const fn refusal_name(refusal: Refusal) -> &'static str {
    match refusal {
        Refusal::Busy => "busy",
        Refusal::Fault => "fault",
        Refusal::Unavailable => "unavailable",
        Refusal::Invalid => "invalid",
        Refusal::NotWritable => "not-writable",
        Refusal::Unverified => "unverified",
        Refusal::Forbidden
        | Refusal::Stale { .. }
        | Refusal::Full
        | Refusal::NotFound
        | Refusal::Muted
        | Refusal::Time
        | Refusal::Update(_) => "other",
    }
}
