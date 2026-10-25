//! Feeding never overlaps a Wi-Fi join or a storage write.
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
//! to the join. A join is bounded by the shim's 10 s scan wait, then one
//! association and one DHCP run, whose own bounds are the vendor SDK's
//! internal timeouts in `wifi_connect` and `LwIP_DHCP` (prebuilt, not in this
//! repository); attempts back off at least 1 s, so feeds get their turn
//! between them. Since nothing here can bound those two calls, the loop
//! counts a join that holds its turn past [`LONG_JOIN_MS`]
//! ([`Interlock::watch_join`]), so a stuck vendor call shows in the log. A
//! join waits at most the bound the claimant published with its claim, which
//! the firmware derives from its feed limits; past it the join goes ahead and is counted as forced, so a
//! claim that is never released cannot keep the feeder offline.
//!
//! **Storage writes.** Every flash lease checks that the outputs are
//! stopped and fails closed when they are not: the lease provider's hook
//! halts and the watchdog resets the board, and a lease the owner sees while
//! it drives faults the feed. So a writer outside the feed journal (the
//! settings writer's stores, the confirmation, the offer check's give-way)
//! takes the write turn first ([`WriteTurn::begin`]) and holds it for
//! one store write, within one step of its task:
//!
//! - While the outputs are claimed the write waits: the writer leaves its
//!   work queued and asks again on its next poll. It counts among the
//!   waiters, so the next claim lets it go first. Only a claim that outlives
//!   its bound, timed per claim, lets a write go ahead with the outputs
//!   claimed, counted as forced, and only while they read stopped: a write
//!   never starts into moving outputs. Each writer times its own wait
//!   ([`WriteTurn`]).
//! - A claim waits in the loop ([`Interlock::claim_feed_after_writes`]), in
//!   [`WRITE_POLL_MS`] steps for at most [`WRITE_WAIT_MS`], for a write
//!   running, a writer waiting, or a client's change queued for a writer
//!   that has not asked yet. A write still running, or a change still
//!   queued, then refuses the feed; waiting writers are passed: they have
//!   not started, and wait for the release.
//!
//! The feed journal writes under the loop's own claim, from the owner's rest
//! boundaries (the outputs already stopped), so it takes no write turn.
//! Writes and joins do not exclude each other.
//!
//! [scheduling gaps]: ../../../../docs/firmware/scheduling-gaps.md#result-october-25-2026-physical

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use crate::supervision::config::FLASH_LIMITS;

/// How often a waiting join looks again; the shim polls its scan as often.
pub const JOIN_POLL_MS: u32 = 50;

/// A join holding its turn this long is counted: the shim's scan wait is
/// 10 s, and associations and DHCP have finished within seconds on the bench.
pub const LONG_JOIN_MS: u32 = 60_000;

/// How often a claim waiting for a storage write looks again.
pub const WRITE_POLL_MS: u32 = 10;

/// The slowest a waiting writer may poll for its turn. Every writer runs
/// in the runtime's service task (the confirmation, the settings writer,
/// the offer check), whose stages each ask again after their own poll; each
/// asserts its poll against this.
pub const WRITER_POLL_MS: u32 = 500;

/// The longest a claim waits for storage writes: one store record (an
/// erase and two programs, the stores' commit protocol), each lease at its
/// whole budget, after a waiting writer's poll. Leases take tens of
/// milliseconds in practice; a write still running past this refuses the
/// feed, which moved nothing.
pub const WRITE_WAIT_MS: u32 = 3_000;

const _: () = assert!(WRITE_WAIT_MS >= 3 * FLASH_LIMITS.total_ms + WRITER_POLL_MS);

/// The turn byte's flags above the [`Turn`] code.
const TURN_BITS: u8 = 0x0f;
/// A storage write holds the flash.
const WRITING: u8 = 0x10;
/// How many writers wait for their turn, in the top three bits: while any
/// does, a new claim lets them go first.
const WAITERS_SHIFT: u8 = 5;
const WAITERS_MAX: u8 = 7;

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
    /// A storage write holds the flash.
    Writing = 3,
    /// A storage write waits to go first.
    WriteWaiting = 4,
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

/// What a storage write finds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteStep {
    /// The write's turn: it may take its flash leases, then
    /// [`Interlock::end_write`].
    Go(WriteStart),
    /// The outputs are claimed (or another write runs): keep the work queued
    /// and ask again.
    Wait,
}

/// How a write's turn began.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteStart {
    /// At once.
    Free,
    /// After waiting `ms` for the outputs' release.
    Waited { ms: u32 },
    /// Past the claim's bound, with the outputs still claimed.
    Forced { ms: u32 },
}

/// The turn byte: the feed and join [`Turn`], and the write flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct State {
    turn: Turn,
    writing: bool,
    /// Writers waiting for their turn; saturates rather than wraps.
    waiters: u8,
}

/// Whether a claim lets waiting writers go first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Waiters {
    GoFirst,
    /// The claim has waited its whole `WRITE_WAIT_MS` for them.
    Passed,
}

impl State {
    const fn from_byte(byte: u8) -> Self {
        Self {
            turn: Turn::from_byte(byte & TURN_BITS),
            writing: byte & WRITING != 0,
            waiters: byte >> WAITERS_SHIFT,
        }
    }

    const fn code(self) -> u8 {
        self.turn.code() | if self.writing { WRITING } else { 0 } | (self.waiters << WAITERS_SHIFT)
    }

    /// A claim: refused by a write as well as a join, except that the loop's
    /// own claim stays its own.
    const fn claim_feed(self, waiters: Waiters) -> Result<Self, Held> {
        let turn = match self.turn.claim_feed() {
            Ok(turn) => turn,
            Err(held) => return Err(held),
        };
        if matches!(self.turn, Turn::Free) {
            if self.writing {
                return Err(Held::Writing);
            }
            if self.waiters > 0 && matches!(waiters, Waiters::GoFirst) {
                return Err(Held::WriteWaiting);
            }
        }
        Ok(Self { turn, ..self })
    }

    const fn outputs_claimed(self) -> bool {
        matches!(self.turn, Turn::Feeding | Turn::FeedingJoinWaits)
    }

    /// A writer asks for the flash; `counted` when it already counts among
    /// the waiters. A writer that must wait counts itself; one that gets the
    /// turn leaves the count, so another's wait stays marked.
    const fn request_write(self, counted: bool) -> (Self, bool) {
        if self.outputs_claimed() || self.writing {
            let waiters = if counted || self.waiters == WAITERS_MAX {
                self.waiters
            } else {
                self.waiters + 1
            };
            return (Self { waiters, ..self }, false);
        }
        (self.take_write(counted), true)
    }

    /// The write turn, leaving the waiters' count if `counted`.
    const fn take_write(self, counted: bool) -> Self {
        let waiters = if counted {
            self.waiters.saturating_sub(1)
        } else {
            self.waiters
        };
        Self {
            writing: true,
            waiters,
            ..self
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
    /// Joins that held their turn past `LONG_JOIN_MS`.
    pub long_joins: u32,
    /// Storage writes that waited for the outputs.
    pub writes_deferred: u32,
    /// Storage writes that went ahead past the claim's bound.
    pub writes_forced: u32,
    /// The longest a storage write waited, in ms.
    pub write_waited_max_ms: u32,
    /// Feeds whose claim waited for a storage write.
    pub feeds_delayed: u32,
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
    /// When the current join's turn began; meaningful while `Joining`.
    join_began_ms: AtomicU32,
    /// The current join was already counted as long.
    join_counted: AtomicBool,
    long_joins: AtomicU32,
    writes_deferred: AtomicU32,
    writes_forced: AtomicU32,
    write_waited_max_ms: AtomicU32,
    feeds_delayed: AtomicU32,
    /// Claims taken from no claim, so a writer can tell one feed from the
    /// next.
    claims: AtomicU32,
    /// When the loop's last claim through `claim_feed_after_writes` landed.
    claimed_at_ms: AtomicU32,
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
            join_began_ms: AtomicU32::new(0),
            join_counted: AtomicBool::new(false),
            long_joins: AtomicU32::new(0),
            writes_deferred: AtomicU32::new(0),
            writes_forced: AtomicU32::new(0),
            write_waited_max_ms: AtomicU32::new(0),
            feeds_delayed: AtomicU32::new(0),
            claims: AtomicU32::new(0),
            claimed_at_ms: AtomicU32::new(0),
        }
    }

    fn state(&self) -> State {
        State::from_byte(self.turn.load(Ordering::Acquire))
    }

    pub fn turn(&self) -> Turn {
        self.state().turn
    }

    /// The loop holds the outputs: a feed may be preparing or moving them.
    pub fn outputs_claimed(&self) -> bool {
        self.state().outputs_claimed()
    }

    /// The outputs are claimed, for no longer than the claim's bound since
    /// it landed: what the loop holds handshakes off for. Past the bound a
    /// claim never released stops holding them, as it stops holding joins,
    /// so it cannot keep new sessions out until a reset.
    pub fn claim_in_bound(&self, now_ms: u32) -> bool {
        self.outputs_claimed()
            && now_ms.wrapping_sub(self.claimed_at_ms.load(Ordering::Acquire))
                < self.bound_ms.load(Ordering::Acquire)
    }

    /// A storage write holds the flash.
    pub fn writing(&self) -> bool {
        self.state().writing
    }

    pub fn status(&self) -> Status {
        Status {
            refused: self.refused.load(Ordering::Relaxed),
            deferred: self.deferred.load(Ordering::Relaxed),
            forced: self.forced.load(Ordering::Relaxed),
            waited_max_ms: self.waited_max_ms.load(Ordering::Relaxed),
            long_joins: self.long_joins.load(Ordering::Relaxed),
            writes_deferred: self.writes_deferred.load(Ordering::Relaxed),
            writes_forced: self.writes_forced.load(Ordering::Relaxed),
            write_waited_max_ms: self.write_waited_max_ms.load(Ordering::Relaxed),
            feeds_delayed: self.feeds_delayed.load(Ordering::Relaxed),
        }
    }

    /// One atomic step of the feed and join turn; the write flags stay.
    fn transition<T>(&self, step: impl Fn(Turn) -> (Turn, T)) -> T {
        self.update(|state| {
            let (turn, out) = step(state.turn);
            (State { turn, ..state }, out)
        })
    }

    /// One atomic step of the whole byte.
    fn update<T>(&self, step: impl Fn(State) -> (State, T)) -> T {
        let mut current = self.turn.load(Ordering::Acquire);
        loop {
            let (next, out) = step(State::from_byte(current));
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
    /// `bound_ms`, the longest the feed can keep them from rest. One attempt,
    /// refused by a running or waiting join or storage write.
    pub fn claim_feed(&self, bound_ms: u32) -> Result<(), Held> {
        let result = self.try_claim(bound_ms, Waiters::GoFirst);
        if result.is_err() {
            self.refused.fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    fn try_claim(&self, bound_ms: u32, waiters: Waiters) -> Result<(), Held> {
        // Published before the claim, so a join or write that sees the
        // claim sees its bound; one firmware always passes the same bound.
        self.bound_ms.store(bound_ms, Ordering::Release);
        // Counted before the claim can land, so a writer that sees the claim
        // never pairs it with the previous count. A claim that then fails
        // (or a race that makes it the loop's own) counts once too many,
        // which only restarts a waiting writer's forced bound later.
        if self.turn() == Turn::Free {
            self.claims.fetch_add(1, Ordering::AcqRel);
        }
        self.update(|state| match state.claim_feed(waiters) {
            Ok(next) => (next, Ok(())),
            Err(held) => (state, Err(held)),
        })
    }

    /// As [`Self::claim_feed`], but storage writes first get up to
    /// [`WRITE_WAIT_MS`], waiting in `delay` steps: a write running, a
    /// writer waiting for its turn, or work `queued` for a writer that has
    /// not asked for its turn yet (a client's change in a mailbox). Past the
    /// wait a write still running refuses the claim, and waiting writers are
    /// passed: they have not started, and wait for the release. Work still
    /// queued in a mailbox at the deadline (a stalled writer) refuses the
    /// claim as a running write does: the client's change would otherwise
    /// land after the feed. A join still refuses at once.
    pub fn claim_feed_after_writes(
        &self,
        bound_ms: u32,
        mut queued: impl FnMut() -> bool,
        mut now_ms: impl FnMut() -> u32,
        mut delay: impl FnMut(u32),
    ) -> Result<(), Held> {
        let begun = now_ms();
        let mut waited = false;
        loop {
            let passed = now_ms().wrapping_sub(begun) >= WRITE_WAIT_MS;
            let waiters = if passed {
                Waiters::Passed
            } else {
                Waiters::GoFirst
            };
            let result = match (queued(), passed) {
                (true, false) => Err(Held::WriteWaiting),
                (true, true) => Err(Held::Writing),
                (false, _) => self.try_claim(bound_ms, waiters),
            };
            match result {
                Ok(()) => {
                    self.claimed_at_ms.store(now_ms(), Ordering::Release);
                    return Ok(());
                }
                Err(Held::Writing | Held::WriteWaiting) if !passed => {}
                Err(held) => {
                    self.refused.fetch_add(1, Ordering::Relaxed);
                    return Err(held);
                }
            }

            if !waited {
                waited = true;
                self.feeds_delayed.fetch_add(1, Ordering::Relaxed);
            }
            delay(WRITE_POLL_MS);
        }
    }

    /// Ask for the write turn: true when it is taken. While it is not, the
    /// writer counts among the waiters, so the next claim lets it go first.
    fn request_write(&self, counted: bool) -> bool {
        self.update(|state| state.request_write(counted))
    }

    /// Take the write turn with the outputs still claimed, leaving the
    /// waiters' count; false while another write holds it.
    fn force_write(&self) -> bool {
        self.update(|state| {
            if state.writing {
                return (state, false);
            }
            (state.take_write(true), true)
        })
    }

    /// The storage write has taken its last lease (or failed).
    pub fn end_write(&self) {
        self.update(|state| {
            let next = State {
                writing: false,
                ..state
            };
            (next, ())
        });
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
            // Stamped before the turn is taken, so the loop's watch never
            // reads a join's turn with the previous join's start.
            let now = now_ms();
            self.join_began_ms.store(now, Ordering::Release);
            self.join_counted.store(false, Ordering::Release);
            let step = self.transition(Turn::request_join);
            let ms = now.wrapping_sub(begun);
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

    /// The loop, each pass: count a join that has held its turn past
    /// `LONG_JOIN_MS`, once per join. True when this call counted it.
    pub fn watch_join(&self, now_ms: u32) -> bool {
        if self.turn() != Turn::Joining
            || now_ms.wrapping_sub(self.join_began_ms.load(Ordering::Acquire)) < LONG_JOIN_MS
            || self.join_counted.swap(true, Ordering::AcqRel)
        {
            return false;
        }
        self.long_joins.fetch_add(1, Ordering::Relaxed);
        true
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
            &self.join_began_ms,
            &self.long_joins,
            &self.writes_deferred,
            &self.writes_forced,
            &self.write_waited_max_ms,
            &self.feeds_delayed,
            &self.claims,
            &self.claimed_at_ms,
        ] {
            value.store(0, Ordering::Release);
        }
        self.join_counted.store(false, Ordering::Release);
    }
}

/// One writer's wait for the write turn, kept by the writer between its
/// polls, so each writer times its own wait.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WriteTurn {
    /// When this writer first found the turn taken, in uptime ms; while it
    /// is set, the writer counts among the waiters.
    since: Option<u32>,
    /// The claim (`Interlock::claims`) its forced bound is timed against.
    claim: u32,
    /// When this writer first saw that claim.
    claim_seen: u32,
}

impl WriteTurn {
    pub const fn new() -> Self {
        Self {
            since: None,
            claim: 0,
            claim_seen: 0,
        }
    }

    /// Before one store write, from task context: the write's turn, or
    /// [`WriteStep::Wait`] while the outputs are claimed (or another write
    /// runs). A [`WriteStep::Go`] must be followed by
    /// [`Interlock::end_write`].
    ///
    /// Only a claim that outlives its bound, timed from when this writer
    /// first saw that same claim (a writer that misses the gap between two
    /// feeds starts again on the second), lets the write go ahead with the
    /// outputs claimed, and only while `stopped` reads them stopped: a write
    /// never starts into moving outputs, whose lease would reset the board.
    pub fn begin(
        &mut self,
        interlock: &Interlock,
        now_ms: u32,
        stopped: impl FnOnce() -> bool,
    ) -> WriteStep {
        let counted = self.since.is_some();
        if interlock.request_write(counted) {
            let start = match self.since.take() {
                None => WriteStart::Free,
                Some(since) => {
                    let ms = now_ms.wrapping_sub(since);
                    interlock.writes_deferred.fetch_add(1, Ordering::Relaxed);
                    interlock
                        .write_waited_max_ms
                        .fetch_max(ms, Ordering::Relaxed);
                    WriteStart::Waited { ms }
                }
            };
            return WriteStep::Go(start);
        }

        let since = *self.since.get_or_insert(now_ms);
        if !interlock.outputs_claimed() {
            return WriteStep::Wait; // Another write runs.
        }
        let claim = interlock.claims.load(Ordering::Acquire);
        if !counted || claim != self.claim {
            self.claim = claim;
            self.claim_seen = now_ms;
        }
        let ms = now_ms.wrapping_sub(self.claim_seen);
        if ms < interlock.bound_ms.load(Ordering::Acquire) || !stopped() || !interlock.force_write()
        {
            return WriteStep::Wait;
        }
        // The claim outlived every feed's bound with the outputs stopped:
        // write anyway, and the lease checks the outputs again.
        self.since = None;
        let waited = now_ms.wrapping_sub(since);
        interlock.writes_deferred.fetch_add(1, Ordering::Relaxed);
        interlock.writes_forced.fetch_add(1, Ordering::Relaxed);
        interlock
            .write_waited_max_ms
            .fetch_max(waited, Ordering::Relaxed);
        WriteStep::Go(WriteStart::Forced { ms })
    }
}

/// The firmware's one interlock: the loop's feeds, the C Wi-Fi task's joins
/// and the runtime's storage writes.
pub static INTERLOCK: Interlock = Interlock::new();
