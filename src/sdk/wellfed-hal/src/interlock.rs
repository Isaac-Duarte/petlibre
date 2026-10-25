//! Feeding and Wi-Fi joins never overlap.
//!
//! A Wi-Fi association holds every task for 0.45–0.72 s, a priority-4 task
//! included (physical, [scheduling gaps]): the stall is in the vendor
//! driver, below the scheduler, so no task priority keeps a feed's 1 s beat
//! through it. The feeding owner therefore stays in the loop, and feeds and
//! joins take turns here instead:
//!
//! - **Feeds.** The loop claims the outputs before it submits a feed
//!   ([`Interlock::claim_feed`]) and releases them once the owner is at rest
//!   with its outputs stopped ([`Interlock::release_feed`]). A claim is
//!   refused while a join runs or waits; the firmware refuses that feed.
//! - **Joins.** Before each join attempt the C Wi-Fi task asks for
//!   credentials (`wellfed_wifi_select`), which first takes the join's turn
//!   ([`Interlock::begin_join`]), waiting while the outputs are claimed; the
//!   attempt's outcome (`wellfed_wifi_outcome`) ends it
//!   ([`Interlock::end_join`]). So the turn covers the whole attempt: scan,
//!   association and DHCP.
//!
//! **Neither starves.** A join that finds a feed running marks itself
//! waiting: no new feed is admitted, and the release hands the turn straight
//! to the join. A join is bounded by the shim (a 10 s scan wait, then one
//! association and one DHCP run) and backs off at least 1 s between attempts,
//! so feeds get their turn between attempts. A join waits at most the bound
//! the claimant published with its claim, which the firmware derives from its
//! feed limits; past it the join goes ahead and is counted as forced, so a
//! claim that is never released cannot keep the feeder offline.
//!
//! [scheduling gaps]: ../../../../docs/firmware/scheduling-gaps.md#result-october-25-2026-physical

use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};

/// How often a waiting join looks again; the shim polls its scan as often.
pub const JOIN_POLL_MS: u32 = 50;

/// Whose turn it is, as the interlock's byte holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Turn {
    /// Neither: a feed or a join may start.
    Free = 0,
    /// A join attempt runs, from its scan to its DHCP result.
    Joining = 1,
    /// The loop holds the outputs: a feed may be preparing or moving them.
    Feeding = 2,
    /// As `Feeding`, and a join waits for the release.
    FeedingJoinWaits = 3,
}

/// Why a feed may not start now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Held {
    /// A join attempt runs.
    Joining = 1,
    /// A join waits for the outputs, and goes first.
    JoinWaiting = 2,
}

impl Held {
    pub const fn code(self) -> u8 {
        self as u8
    }
}

/// What a join request finds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinStep {
    /// The join's turn: it may associate.
    Go,
    /// The outputs are claimed: wait for their release.
    Wait,
}

/// How a join's turn began.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinStart {
    /// At once.
    Free,
    /// After the outputs were released.
    Waited { ms: u32 },
    /// Past the claim's bound, with the outputs still claimed.
    Forced { ms: u32 },
}

impl Turn {
    pub const fn code(self) -> u8 {
        self as u8
    }

    /// Only `Interlock` stores the byte, from `code`; an unknown one reads as
    /// the outputs claimed, the side that never lets a join through early.
    pub const fn from_byte(byte: u8) -> Self {
        match byte {
            0 => Self::Free,
            1 => Self::Joining,
            3 => Self::FeedingJoinWaits,
            _ => Self::Feeding,
        }
    }

    /// The loop asks to start a feed. Held already by the loop (a feed the
    /// owner refused, not yet released) counts as its own.
    pub const fn claim_feed(self) -> Result<Self, Held> {
        match self {
            Self::Free | Self::Feeding => Ok(Self::Feeding),
            Self::Joining => Err(Held::Joining),
            Self::FeedingJoinWaits => Err(Held::JoinWaiting),
        }
    }

    /// The outputs are at rest: a waiting join takes the turn directly.
    pub const fn release_feed(self) -> Self {
        match self {
            Self::Feeding => Self::Free,
            Self::FeedingJoinWaits => Self::Joining,
            other => other,
        }
    }

    /// The Wi-Fi task asks to join. `Joining` is only ever the join's own
    /// turn (taken, or handed over by a release).
    pub const fn request_join(self) -> (Self, JoinStep) {
        match self {
            Self::Free | Self::Joining => (Self::Joining, JoinStep::Go),
            Self::Feeding | Self::FeedingJoinWaits => (Self::FeedingJoinWaits, JoinStep::Wait),
        }
    }

    /// The join attempt has its outcome.
    pub const fn end_join(self) -> Self {
        match self {
            Self::Joining => Self::Free,
            other => other,
        }
    }
}

/// What the interlock has done this boot, for the loop's log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// Feeds refused because a join ran or waited.
    pub refused: u32,
    /// Joins that waited for the outputs.
    pub deferred: u32,
    /// Joins that went ahead past the claim's bound.
    pub forced: u32,
    /// The longest a join waited, in ms.
    pub waited_max_ms: u32,
}

/// The turn between the loop (feeds) and the C Wi-Fi task (joins), as
/// atomics: neither task blocks the other except a join waiting, in its own
/// task, for the release.
pub struct Interlock {
    turn: AtomicU8,
    /// The claimant's bound on one claim; what a join waits at most.
    bound_ms: AtomicU32,
    refused: AtomicU32,
    deferred: AtomicU32,
    forced: AtomicU32,
    waited_max_ms: AtomicU32,
}

impl Default for Interlock {
    fn default() -> Self {
        Self::new()
    }
}

impl Interlock {
    pub const fn new() -> Self {
        Self {
            turn: AtomicU8::new(Turn::Free.code()),
            bound_ms: AtomicU32::new(0),
            refused: AtomicU32::new(0),
            deferred: AtomicU32::new(0),
            forced: AtomicU32::new(0),
            waited_max_ms: AtomicU32::new(0),
        }
    }

    pub fn turn(&self) -> Turn {
        Turn::from_byte(self.turn.load(Ordering::Acquire))
    }

    pub fn status(&self) -> Status {
        Status {
            refused: self.refused.load(Ordering::Relaxed),
            deferred: self.deferred.load(Ordering::Relaxed),
            forced: self.forced.load(Ordering::Relaxed),
            waited_max_ms: self.waited_max_ms.load(Ordering::Relaxed),
        }
    }

    /// One atomic step of the turn.
    fn transition<T>(&self, step: impl Fn(Turn) -> (Turn, T)) -> T {
        let mut current = self.turn.load(Ordering::Acquire);
        loop {
            let (next, out) = step(Turn::from_byte(current));
            match self.turn.compare_exchange_weak(
                current,
                next.code(),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return out,
                Err(actual) => current = actual,
            }
        }
    }

    /// The loop, before it submits a feed: claim the outputs for at most
    /// `bound_ms`, the longest the feed can keep them from rest.
    pub fn claim_feed(&self, bound_ms: u32) -> Result<(), Held> {
        // Published before the claim, so a join that sees the claim sees
        // its bound; one firmware always passes the same bound.
        self.bound_ms.store(bound_ms, Ordering::Release);
        let result = self.transition(|turn| match turn.claim_feed() {
            Ok(next) => (next, Ok(())),
            Err(held) => (turn, Err(held)),
        });
        if result.is_err() {
            self.refused.fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    /// The loop, once the owner is at rest with its outputs stopped and no
    /// feed in flight. Harmless when nothing is claimed.
    pub fn release_feed(&self) {
        self.transition(|turn| (turn.release_feed(), ()));
    }

    /// The Wi-Fi task, before a join attempt: take the join's turn, waiting
    /// in `delay` steps while the outputs are claimed, for at most the
    /// claim's bound.
    pub fn begin_join(
        &self,
        mut now_ms: impl FnMut() -> u32,
        mut delay: impl FnMut(u32),
    ) -> JoinStart {
        let begun = now_ms();
        let mut waited = false;
        loop {
            let step = self.transition(Turn::request_join);
            let ms = now_ms().wrapping_sub(begun);
            if step == JoinStep::Go {
                if !waited {
                    return JoinStart::Free;
                }
                self.waited_max_ms.fetch_max(ms, Ordering::Relaxed);
                return JoinStart::Waited { ms };
            }

            if !waited {
                waited = true;
                self.deferred.fetch_add(1, Ordering::Relaxed);
            }
            if ms >= self.bound_ms.load(Ordering::Acquire) {
                // The claim outlived every feed's bound: join anyway, and
                // the loop's next claim waits for this join's end.
                self.transition(|_| (Turn::Joining, ()));
                self.forced.fetch_add(1, Ordering::Relaxed);
                self.waited_max_ms.fetch_max(ms, Ordering::Relaxed);
                return JoinStart::Forced { ms };
            }
            delay(JOIN_POLL_MS);
        }
    }

    /// The Wi-Fi task, after the attempt's outcome.
    pub fn end_join(&self) {
        self.transition(|turn| (turn.end_join(), ()));
    }

    /// Host tests only: back to the boot state.
    #[cfg(not(target_os = "none"))]
    pub fn reset(&self) {
        self.turn.store(Turn::Free.code(), Ordering::Release);
        for value in [
            &self.bound_ms,
            &self.refused,
            &self.deferred,
            &self.forced,
            &self.waited_max_ms,
        ] {
            value.store(0, Ordering::Release);
        }
    }
}

/// The firmware's one interlock: the loop's feeds and the C Wi-Fi task's joins.
pub static INTERLOCK: Interlock = Interlock::new();
