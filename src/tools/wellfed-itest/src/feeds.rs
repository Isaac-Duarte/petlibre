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

use wellfed_core::feed::controller::FeedReason;
use wellfed_core::feed::owner::OwnerState;
use wellfed_core::feed::storage::OwnerOutcome;
use wellfed_core::record::{Confirm, Event, Owner};
use wellfed_model::{Refusal, Settings, SettingsPatch};
use wellfed_rig::device::{Answer, Ask};
use wellfed_rig::{Error, Result, require};

use super::log::events;
use super::scenarios::{RECORD_SECONDS, changes_to, read_settings, set_settings};
use super::{Ctx, compact};

const PORTIONS: u8 = 2;
/// From the reservation to the motor driving: the optics and the
/// supervisor come up first.
const PREPARE_SECONDS: f64 = 30.0;
/// About 9 s a portion on the bench, well inside the owner's own cap.
const FEED_SECONDS: f64 = 90.0;
/// Into the drive before the cancel: before the first portion's mark,
/// about 7.8 s in on the bench.
const CANCEL_AFTER_SECONDS: f64 = 2.0;
/// From a cancel, or a result, to the owner's next record.
const STOP_SECONDS: f64 = 10.0;

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
        let reason = format!(
            "feed {} may still be running ({wait}); nothing was restored",
            running.feed
        );
        ctx.halted = Some(reason.clone());
        if matches!(wait, Error::Interrupted) {
            return Err(Error::Interrupted);
        }
        let failure = result.err().map_or_else(String::new, |e| format!("{e}; "));
        return Err(Error::Bench(format!("{failure}{reason}")));
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
    let answer = ctx.ask_held(&Ask::Status)?;
    require(
        matches!(answer, Answer::Status(_)),
        format!("The held session's status answered {}", compact(&answer)),
    )?;

    let change = writes_during_feed(ctx, before, locked, issued)?;
    // The change the feed refused goes through once it is over.
    set_settings(ctx, change)?;
    cancel_during_feed(ctx, issued)
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
        FEED_SECONDS + STOP_SECONDS,
        |e| fed_of(e, running.feed).is_some(),
    )?;
    ctx.expect("the owner at rest", fed, STOP_SECONDS, |e| {
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
    ctx.expect("the motor driving", start, PREPARE_SECONDS, |e| {
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
    require(
        writes.is_empty(),
        format!("Store writes during feed {feed}: {}", writes.join(" | ")),
    )?;
    let result = result_at(ctx, fed, feed)?;
    require(
        result.outcome == Some(OwnerOutcome::Done)
            && result.requested == u16::from(PORTIONS)
            && result.completed == u16::from(PORTIONS),
        format!("Feed {feed} did not end Done {PORTIONS}/{PORTIONS}: {result:?}"),
    )?;
    require(
        settings == Answer::Refused(Refusal::Busy),
        format!(
            "A settings change during feed {feed} answered {}, not busy",
            compact(&settings)
        ),
    )?;
    let done = Answer::Done { generation: None };
    if let Some(answer) = &lock {
        require(
            *answer == done,
            format!(
                "A lock change during feed {feed} answered {}",
                compact(answer)
            ),
        )?;
    }
    require(
        clock == done,
        format!(
            "A clock sync during feed {feed} answered {}",
            compact(&clock)
        ),
    )?;
    deferred_writes(ctx, start)?;

    let after = read_settings(ctx)?;
    require(
        changes_to(&after) == changes_to(before),
        format!(
            "The settings read {} after the change the feed refused",
            compact(&after)
        ),
    )?;
    if let Some(locked) = locked {
        let now = ctx.device_status()?.locked;
        require(
            now == Some(!locked),
            format!(
                "The lock reads {now:?} after a change to {} during the feed",
                !locked
            ),
        )?;
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
    require(
        after > before.unwrap_or(0),
        format!("The interlock deferred no write during the feed (writes_deferred={after})"),
    )
}

/// Feed again, and cancel it from the held session once the motor drives.
fn cancel_during_feed(ctx: &mut Ctx, issued: &mut Option<Issued>) -> Result<()> {
    // Also a keepalive: the feeder drops a session quiet for 90 s.
    let answer = ctx.ask_held(&Ask::Status)?;
    require(
        matches!(answer, Answer::Status(_)),
        format!("The held session's status answered {}", compact(&answer)),
    )?;

    let (feed, start) = start_feed(ctx, issued)?;
    ctx.expect("the motor driving", start, PREPARE_SECONDS, |e| {
        owner_state(e, OwnerState::Feeding)
    })?;
    ctx.idle(CANCEL_AFTER_SECONDS)?;
    let since = ctx.mark()?;
    let cancel = ctx.ask_held(&Ask::CancelFeed { feed })?;
    ctx.note(&format!(
        "  Held session, cancel of feed {feed}: {}",
        compact(&cancel)
    ));

    let fed = settle(ctx, Issued { feed, since: start })?;
    *issued = None;

    require(
        cancel == Answer::Done { generation: None },
        format!("The cancel of feed {feed} answered {}", compact(&cancel)),
    )?;
    let result = result_at(ctx, fed, feed)?;
    require(
        result.outcome == Some(OwnerOutcome::Cancelled)
            && result.reason == Some(FeedReason::Cancelled),
        format!("Feed {feed} was not cancelled: {result:?}"),
    )?;
    // The owner logs its outputs stopped before it reports the result.
    let stopped = ctx.log.between(since, fed, |e| {
        matches!(e, Event::Owner(Owner::State { state, stopped: true }) if *state != OwnerState::Feeding)
    });
    require(
        !stopped.is_empty(),
        format!("No stopped outputs logged between the cancel of feed {feed} and its result"),
    )?;
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
        Err(Error::Interrupted) => return Err(Error::Interrupted),
        Err(error) => {
            *issued = taken(ctx, since)?.map(|feed| Issued { feed, since });
            if issued.is_none() {
                // It may still be on its way to the owner, or taken
                // unlogged: nothing tells when the outputs are free.
                ctx.halted = Some(format!(
                    "a feed request failed ({error}) and no feed record followed within \
                     {RECORD_SECONDS:.0} s; nothing was restored"
                ));
            }
            return Err(Error::Bench(format!(
                "The feed request failed, so the feed is uncertain and is not sent again: {error}"
            )));
        }
    };
    let Answer::Feeding { feed } = answer else {
        return Err(Error::Bench(format!(
            "The feed answered {}",
            compact(&answer)
        )));
    };
    *issued = Some(Issued { feed, since });
    ctx.note(&format!("  Feed {feed}: {PORTIONS} portions"));

    let start = ctx.expect(
        &format!("PLOWN1 feed id={feed}"),
        since,
        RECORD_SECONDS,
        |e| submitted(e) == Some(u64::from(feed)),
    )?;
    Ok((feed, start))
}

/// The feed the owner took since `since` without the suite hearing its
/// answer, if its record shows one.
fn taken(ctx: &mut Ctx, since: usize) -> Result<Option<u32>> {
    let found = ctx.expect("PLOWN1 feed id=", since, RECORD_SECONDS, |e| {
        submitted(e).is_some()
    });
    let mark = match found {
        Ok(mark) => mark,
        Err(Error::Interrupted) => return Err(Error::Interrupted),
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
struct Fed {
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
    events(line)
        .find_map(|e| fed_of(&e, feed))
        .ok_or_else(|| Error::Bench(format!("Feed {feed}'s result does not parse: {line}")))
}

/// No restart since `since`, on the UART or in the boot count.
fn no_reset(ctx: &mut Ctx, since: usize, boots: u32) -> Result<()> {
    let restarts = ctx.log.between(since, usize::MAX, |e| {
        matches!(e, Event::Start { .. } | Event::Panic { .. })
    });
    require(
        restarts.is_empty(),
        format!("The feeder restarted: {}", restarts.join(" | ")),
    )?;
    let status = ctx.device_status()?;
    require(
        status.boot_count == boots,
        format!("The boot count went from {boots} to {}", status.boot_count),
    )
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
    let answer = ctx.ask(&Ask::SetLock { locked })?;
    require(
        answer == Answer::Done { generation: None },
        format!("Restoring the lock answered {}", compact(&answer)),
    )
}
