//! `feed-writes`: what a client's writes do while a feed holds the outputs.
//!
//! Every flash lease fails closed while the outputs may move, so the feeder
//! keeps store writes out of a feed: a client's settings change is refused
//! `Busy` while the outputs are claimed, and the events a lock change or a
//! clock sync record wait in RAM until the owner is at rest. The feeder also
//! admits no handshake during a feed, so the writes go over a second owner
//! session held open from before it; the feeds go over the suite's own.
//!
//! A second feed is cancelled from the held session a moment into its
//! drive, which must stop the motor and end the feed cancelled.
//!
//! The feeds move the motor unattended (`--feeds`), so the hopper may be
//! empty: the rotor's marks count the portions whether or not food falls.

use std::time::Duration;

use wellfed_core::feed::controller::FeedReason;
use wellfed_core::feed::owner::OwnerState;
use wellfed_core::feed::storage::OwnerOutcome;
use wellfed_core::record::{Confirm, Event, Owner};
use wellfed_formats::require;
use wellfed_model::{Refusal, Settings, SettingsPatch};
use wellfed_rig::device::{Answer, Ask};

use super::log::events;
use super::scenarios::{RECORD_WAIT, changes_to, read_settings, set_settings};
use super::{Ctx, Error, Result, compact};

/// Why `feed-writes` failed. A feed that may still be running is its own
/// variant, since the run stops after it.
#[derive(Debug, thiserror::Error)]
pub enum FeedsError {
    /// Waiting out a feed failed (`wait`), after the scenario's own
    /// `failure` if it had one: nothing was restored.
    #[error(
        "{}{}",
        failed_first(failure.as_deref()),
        unsettled(*feed, wait)
    )]
    Unsettled {
        feed: u32,
        wait: Box<Error>,
        failure: Option<Box<Error>>,
    },
    #[error("The held session's status answered {}", compact(answer))]
    HeldStatus { answer: Answer },
    /// `writes` are the UART lines of the store writes.
    #[error("Store writes during feed {feed}: {}", writes.join(" | "))]
    StoreWrites { feed: u32, writes: Vec<String> },
    #[error("Feed {feed} did not end Done {}/{}: {result:?}", PORTIONS, PORTIONS)]
    NotDone { feed: u32, result: Fed },
    #[error(
        "A settings change during feed {feed} answered {}, not busy",
        compact(answer)
    )]
    SettingsNotBusy { feed: u32, answer: Answer },
    #[error("A lock change during feed {feed} answered {}", compact(answer))]
    LockAnswer { feed: u32, answer: Answer },
    #[error("A clock sync during feed {feed} answered {}", compact(answer))]
    ClockAnswer { feed: u32, answer: Answer },
    #[error(
        "The settings read {} after the change the feed refused",
        compact(after)
    )]
    SettingsAfter { after: Settings },
    #[error("The lock reads {now:?} after a change to {wanted} during the feed")]
    LockAfter { now: Option<bool>, wanted: bool },
    #[error("The interlock deferred no write during the feed (writes_deferred={after})")]
    NothingDeferred { after: u32 },
    #[error("The cancel of feed {feed} answered {}", compact(answer))]
    CancelAnswer { feed: u32, answer: Answer },
    #[error("Feed {feed} was not cancelled: {result:?}")]
    NotCancelled { feed: u32, result: Fed },
    #[error("No stopped outputs logged between the cancel of feed {feed} and its result")]
    NoStoppedOutputs { feed: u32 },
    /// The request failed, so the feeder may have taken it: never resent.
    #[error("The feed request failed, so the feed is uncertain and is not sent again: {error}")]
    Uncertain { error: Box<Error> },
    #[error("The feed answered {}", compact(answer))]
    FeedAnswer { answer: Answer },
    #[error("Feed {feed}'s result does not parse: {line}")]
    Unparsed { feed: u32, line: String },
    /// `restarts` are the UART lines of the restarts.
    #[error("The feeder restarted: {}", restarts.join(" | "))]
    Restarted { restarts: Vec<String> },
    #[error("The boot count went from {before} to {after}")]
    BootCount { before: u32, after: u32 },
    #[error("Restoring the lock answered {}", compact(answer))]
    LockRestore { answer: Answer },
}

/// Why the run stops after a feed left running: also what it records.
fn unsettled(feed: u32, wait: &Error) -> String {
    format!("feed {feed} may still be running ({wait}); nothing was restored")
}

/// The scenario's own failure ahead of a feed left running, if it had one.
fn failed_first(failure: Option<&Error>) -> String {
    failure.map_or_else(String::new, |error| format!("{error}; "))
}

const PORTIONS: u8 = 2;
/// From the reservation to the motor driving: the optics and the
/// supervisor come up first.
const PREPARE_WAIT: Duration = Duration::from_secs(30);
/// About 9 s a portion on the bench, well inside the owner's own cap.
const FEED_WAIT: Duration = Duration::from_secs(90);
/// Into the drive before the cancel: before the first portion's mark,
/// about 7.8 s in on the bench.
const CANCEL_AFTER: Duration = Duration::from_secs(2);
/// From a cancel, or a result, to the owner's next record.
const STOP_WAIT: Duration = Duration::from_secs(10);

/// A feed the owner may still be running: its id, and the UART mark from
/// before it was asked for, where its result is looked for.
#[derive(Debug, Clone, Copy)]
struct Issued {
    feed: u32,
    since: usize,
}

pub(super) fn feed_writes(ctx: &mut Ctx) -> Result<()> {
    let before = read_settings(ctx)?;
    let status = ctx.device_status()?;
    let since = ctx.mark()?;

    // Held before either feed: a handshake during one is held off.
    ctx.rig.hold()?;
    let mut issued = None;
    let result = held_across_feeds(ctx, &before, status.locked, &mut issued);
    ctx.rig.release();

    // A failure can leave a feed running, and nothing may be written or
    // fed until it ends; one that does not end stops the suite.
    if let Some(running) = issued
        && let Err(wait) = settle(ctx, running)
    {
        ctx.halted = Some(unsettled(running.feed, &wait));
        if wait.interrupted() {
            return Err(wait);
        }
        return Err(FeedsError::Unsettled {
            feed: running.feed,
            wait: Box::new(wait),
            failure: result.err().map(Box::new),
        }
        .into());
    }
    // An uncertain feed the UART never showed: nothing is safe to send.
    if ctx.halted.is_some() {
        return result;
    }

    // Put back whatever went through, whatever failed.
    let restored = restore(ctx, &before, status.locked);
    let reset = no_reset(ctx, since, status.boot_count);
    result.and(reset).and(restored)
}

fn held_across_feeds(
    ctx: &mut Ctx,
    before: &Settings,
    locked: Option<bool>,
    issued: &mut Option<Issued>,
) -> Result<()> {
    held_status(ctx)?;

    let change = writes_during_feed(ctx, before, locked, issued)?;
    // The change the feed refused goes through once it is over.
    set_settings(ctx, change)?;
    cancel_during_feed(ctx, issued)
}

/// The held session answers a status request with the status.
fn held_status(ctx: &mut Ctx) -> Result<()> {
    match ctx.ask_held(&Ask::Status)? {
        Answer::Status(_) => Ok(()),
        answer => Err(FeedsError::HeldStatus { answer }.into()),
    }
}

/// Wait for `running`'s result and the owner at rest after it.
fn settle(ctx: &mut Ctx, running: Issued) -> Result<usize> {
    ctx.note(&format!(
        "  Waiting for feed {} to end before anything else",
        running.feed
    ));
    let fed = ctx.expect(
        &format!("PLOWN1 fed id={}", running.feed),
        running.since,
        FEED_WAIT + STOP_WAIT,
        |e| fed_of(e, running.feed).is_some(),
    )?;
    ctx.expect("the owner at rest", fed, STOP_WAIT, |e| {
        owner_state(e, OwnerState::Idle)
    })?;
    Ok(fed)
}

/// Feed on the suite's session and, while the motor drives, write on the
/// held one; the change the feed refused.
fn writes_during_feed(
    ctx: &mut Ctx,
    before: &Settings,
    locked: Option<bool>,
    issued: &mut Option<Issued>,
) -> Result<SettingsPatch> {
    let change = SettingsPatch {
        lights: Some(!before.lights),
        ..SettingsPatch::default()
    };
    let (feed, start) = start_feed(ctx, issued)?;
    ctx.expect("the motor driving", start, PREPARE_WAIT, |e| {
        owner_state(e, OwnerState::Feeding)
    })?;

    let settings = ctx.ask_held(&Ask::PutSettings {
        base: before.generation.0,
        changes: change,
    })?;
    ctx.note(&format!(
        "  Held session, settings change during feed {feed}: {}",
        compact(&settings)
    ));
    let lock = match locked {
        Some(locked) => {
            let answer = ctx.ask_held(&Ask::SetLock { locked: !locked })?;
            ctx.note(&format!(
                "  Held session, lock {} during feed {feed}: {}",
                if locked { "off" } else { "on" },
                compact(&answer)
            ));
            Some(answer)
        }
        None => {
            ctx.note("  The lock is not known yet; no lock change");
            None
        }
    };
    let clock = ctx.ask_held(&Ask::SyncClock)?;
    ctx.note(&format!(
        "  Held session, clock sync during feed {feed}: {}",
        compact(&clock)
    ));

    let fed = settle(ctx, Issued { feed, since: start })?;
    *issued = None;

    let writes = ctx.log.between(start - 1, fed, store_write);
    require(writes.is_empty(), || FeedsError::StoreWrites {
        feed,
        writes,
    })?;
    let result = result_at(ctx, fed, feed)?;
    require(
        result.outcome == Some(OwnerOutcome::Done)
            && result.requested == u16::from(PORTIONS)
            && result.completed == u16::from(PORTIONS),
        || FeedsError::NotDone { feed, result },
    )?;
    require(settings == Answer::Refused(Refusal::Busy), || {
        FeedsError::SettingsNotBusy {
            feed,
            answer: settings,
        }
    })?;
    let done = Answer::Done { generation: None };
    if let Some(answer) = &lock {
        require(*answer == done, || FeedsError::LockAnswer {
            feed,
            answer: answer.clone(),
        })?;
    }
    require(clock == done, || FeedsError::ClockAnswer {
        feed,
        answer: clock,
    })?;
    deferred_writes(ctx, start)?;

    let after = read_settings(ctx)?;
    require(changes_to(&after) == changes_to(before), || {
        FeedsError::SettingsAfter { after }
    })?;
    if let Some(locked) = locked {
        let now = ctx.device_status()?.locked;
        require(now == Some(!locked), || FeedsError::LockAfter {
            now,
            wanted: !locked,
        })?;
    }
    Ok(change)
}

/// The writes the interlock held back, from the `PLOWN1 interlock` records
/// before and after `start` when the feeder logged one since: the events
/// the lock change and the clock sync recorded wait out the feed.
fn deferred_writes(ctx: &mut Ctx, start: usize) -> Result<()> {
    let deferred = |lines: Vec<String>| -> Option<u32> {
        lines.iter().rev().find_map(|line| {
            events(line).find_map(|e| match e {
                Event::Owner(Owner::Interlock { writes_deferred }) => writes_deferred,
                _ => None,
            })
        })
    };
    let interlock = |e: &Event<'_>| matches!(e, Event::Owner(Owner::Interlock { .. }));
    let before = deferred(ctx.log.between(0, start, interlock));
    let Some(after) = deferred(ctx.log.between(start, usize::MAX, interlock)) else {
        ctx.note("  No interlock record since the feed began");
        return Ok(());
    };

    ctx.note(&format!(
        "  Interlock: writes_deferred {} then {after}",
        before.map_or_else(|| "unlogged".to_string(), |n| n.to_string())
    ));
    require(after > before.unwrap_or(0), || {
        FeedsError::NothingDeferred { after }.into()
    })
}

/// Feed again, and cancel it from the held session once the motor drives.
fn cancel_during_feed(ctx: &mut Ctx, issued: &mut Option<Issued>) -> Result<()> {
    // Also a keepalive: the feeder drops a session quiet for 90 s.
    held_status(ctx)?;

    let (feed, start) = start_feed(ctx, issued)?;
    ctx.expect("the motor driving", start, PREPARE_WAIT, |e| {
        owner_state(e, OwnerState::Feeding)
    })?;
    ctx.idle(CANCEL_AFTER)?;
    let since = ctx.mark()?;
    let cancel = ctx.ask_held(&Ask::CancelFeed { feed })?;
    ctx.note(&format!(
        "  Held session, cancel of feed {feed}: {}",
        compact(&cancel)
    ));

    let fed = settle(ctx, Issued { feed, since: start })?;
    *issued = None;

    require(cancel == Answer::Done { generation: None }, || {
        FeedsError::CancelAnswer {
            feed,
            answer: cancel,
        }
    })?;
    let result = result_at(ctx, fed, feed)?;
    require(
        result.outcome == Some(OwnerOutcome::Cancelled)
            && result.reason == Some(FeedReason::Cancelled),
        || FeedsError::NotCancelled { feed, result },
    )?;
    // The owner logs its outputs stopped before it reports the result.
    let stopped = ctx.log.between(since, fed, |e| {
        matches!(e, Event::Owner(Owner::State { state, stopped: true }) if *state != OwnerState::Feeding)
    });
    require(!stopped.is_empty(), || FeedsError::NoStoppedOutputs {
        feed,
    })?;
    ctx.note(&format!(
        "  Feed {feed} cancelled with {}/{PORTIONS} portions",
        result.completed
    ));
    Ok(())
}

/// Ask once for a feed on the suite's session; its id and the mark just
/// past the owner taking it. From the request on, `issued` holds any feed
/// the owner may be running. A request that fails is uncertain and never
/// sent again: the feeder may have taken it.
fn start_feed(ctx: &mut Ctx, issued: &mut Option<Issued>) -> Result<(u32, usize)> {
    let since = ctx.mark()?;
    let answer = match ctx.ask(&Ask::Feed { portions: PORTIONS }) {
        Ok(answer) => answer,
        Err(error) if error.interrupted() => return Err(error),
        Err(error) => {
            *issued = taken(ctx, since)?.map(|feed| Issued { feed, since });
            if issued.is_none() {
                // It may still be on its way to the owner, or taken
                // unlogged: nothing tells when the outputs are free.
                ctx.halted = Some(format!(
                    "a feed request failed ({error}) and no feed record followed within \
                     {:.0} s; nothing was restored",
                    RECORD_WAIT.as_secs_f64()
                ));
            }
            return Err(FeedsError::Uncertain {
                error: Box::new(error),
            }
            .into());
        }
    };
    let Answer::Feeding { feed } = answer else {
        return Err(FeedsError::FeedAnswer { answer }.into());
    };
    *issued = Some(Issued { feed, since });
    ctx.note(&format!("  Feed {feed}: {PORTIONS} portions"));

    let start = ctx.expect(&format!("PLOWN1 feed id={feed}"), since, RECORD_WAIT, |e| {
        submitted(e) == Some(u64::from(feed))
    })?;
    Ok((feed, start))
}

/// The feed the owner took since `since` without the suite hearing its
/// answer, if its record shows one.
fn taken(ctx: &mut Ctx, since: usize) -> Result<Option<u32>> {
    let found = ctx.expect("PLOWN1 feed id=", since, RECORD_WAIT, |e| {
        submitted(e).is_some()
    });
    let mark = match found {
        Ok(mark) => mark,
        Err(error) if error.interrupted() => return Err(error),
        Err(_) => return Ok(None),
    };
    Ok(events(&ctx.log.line(mark - 1).text)
        .find_map(|e| submitted(&e))
        .and_then(|id| u32::try_from(id).ok()))
}

/// The feed a `PLOWN1 feed id=` record hands to the owner.
fn submitted(event: &Event<'_>) -> Option<u64> {
    match *event {
        Event::Owner(Owner::Submitted { id }) => Some(id),
        _ => None,
    }
}

fn owner_state(event: &Event<'_>, state: OwnerState) -> bool {
    matches!(event, Event::Owner(Owner::State { state: s, .. }) if *s == state)
}

/// A record the settings writer logs once a store write lands, or the
/// build confirmation's: its write landing, or failing. `already=1` names
/// a build confirmed before, so nothing was written.
fn store_write(event: &Event<'_>) -> bool {
    matches!(
        event,
        Event::Settings(_)
            | Event::Confirm(Confirm::Confirmed {
                healthy_ms: Some(_)
            })
            | Event::Confirm(Confirm::Error { .. })
    )
}

/// What `PLOWN1 fed` reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fed {
    requested: u16,
    completed: u16,
    outcome: Option<OwnerOutcome>,
    reason: Option<FeedReason>,
}

/// `event` as feed `feed`'s result, if it is one.
fn fed_of(event: &Event<'_>, feed: u32) -> Option<Fed> {
    match *event {
        Event::Owner(Owner::Fed {
            id,
            requested,
            completed,
            outcome,
            reason,
        }) if id == u64::from(feed) => Some(Fed {
            requested,
            completed,
            outcome,
            reason,
        }),
        _ => None,
    }
}

/// The result on the line just before `mark` (an [`Ctx::expect`] result).
fn result_at(ctx: &Ctx, mark: usize, feed: u32) -> Result<Fed> {
    let line = &ctx.log.line(mark - 1).text;
    events(line).find_map(|e| fed_of(&e, feed)).ok_or_else(|| {
        FeedsError::Unparsed {
            feed,
            line: line.clone(),
        }
        .into()
    })
}

/// No restart since `since`, on the UART or in the boot count.
fn no_reset(ctx: &mut Ctx, since: usize, boots: u32) -> Result<()> {
    let restarts = ctx.log.between(since, usize::MAX, |e| {
        matches!(e, Event::Start { .. } | Event::Panic { .. })
    });
    require(restarts.is_empty(), || FeedsError::Restarted { restarts })?;
    let status = ctx.device_status()?;
    require(status.boot_count == boots, || {
        FeedsError::BootCount {
            before: boots,
            after: status.boot_count,
        }
        .into()
    })
}

/// Put back the settings and the lock as `before` and `locked` had them.
fn restore(ctx: &mut Ctx, before: &Settings, locked: Option<bool>) -> Result<()> {
    if changes_to(&read_settings(ctx)?) != changes_to(before) {
        set_settings(ctx, changes_to(before))?;
    }
    let Some(locked) = locked else {
        return Ok(());
    };
    if ctx.device_status()?.locked == Some(locked) {
        return Ok(());
    }
    match ctx.ask(&Ask::SetLock { locked })? {
        Answer::Done { generation: None } => Ok(()),
        answer => Err(FeedsError::LockRestore { answer }.into()),
    }
}
