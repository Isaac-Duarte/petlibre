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

use wellfed_bench::device::{Answer, Ask};
use wellfed_bench::{Error, Result, require};
use wellfed_core::feed::controller::FeedReason;
use wellfed_core::feed::storage::OwnerOutcome;
use wellfed_core::record::tag;
use wellfed_model::{Refusal, Settings, SettingsPatch};

use super::log::Record;
use super::scenarios::{RECORD_SECONDS, changes_to, read_settings, set_settings};
use super::{Ctx, compact};

const PORTIONS: u8 = 2;
/// The feeding owner's records: `feed id=`, `state=`, `fed id=`.
const OWNER: &str = "PLOWN1";
/// `state=` as `wellfed_hal::owner::OwnerState` numbers it.
const FEEDING: &str = "4";
const IDLE: &str = "1";
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

pub(super) fn feed_writes(ctx: &mut Ctx) -> Result<()> {
    let before = read_settings(ctx)?;
    let status = ctx.device_status()?;
    let since = ctx.mark()?;

    // Held before either feed: a handshake during one is held off.
    ctx.rig.hold()?;
    let result = held_across_feeds(ctx, &before, status.locked);
    ctx.rig.release();

    // Put back whatever went through, whatever failed.
    let restored = restore(ctx, &before, status.locked);
    let reset = no_reset(ctx, since, status.boot_count);
    result.and(reset).and(restored)
}

fn held_across_feeds(ctx: &mut Ctx, before: &Settings, locked: Option<bool>) -> Result<()> {
    let answer = ctx.ask_held(&Ask::Status)?;
    require(
        matches!(answer, Answer::Status(_)),
        format!("The held session's status answered {}", compact(&answer)),
    )?;

    let change = writes_during_feed(ctx, before, locked)?;
    // The change the feed refused goes through once it is over.
    set_settings(ctx, change)?;
    cancel_during_feed(ctx)
}

/// Feed on the suite's session and, while the motor drives, write on the
/// held one; the change the feed refused.
fn writes_during_feed(
    ctx: &mut Ctx,
    before: &Settings,
    locked: Option<bool>,
) -> Result<SettingsPatch> {
    let change = SettingsPatch {
        lights: Some(!before.lights),
        ..SettingsPatch::default()
    };
    let (feed, start) = start_feed(ctx)?;
    let moving = ctx.expect("the motor driving", start, PREPARE_SECONDS, |r| {
        owner_state(r, FEEDING)
    })?;

    let settings = ctx.ask_held(&Ask::PutSettings {
        base: before.generation.0,
        changes: change,
    })?;
    ctx.note(&format!(
        "  Held session, settings change during feed {feed}: {}",
        compact(&settings)
    ));
    match locked {
        Some(locked) => {
            let answer = ctx.ask_held(&Ask::SetLock { locked: !locked })?;
            ctx.note(&format!(
                "  Held session, lock {} during feed {feed}: {}",
                if locked { "off" } else { "on" },
                compact(&answer)
            ));
        }
        None => ctx.note("  The lock is not known yet; no lock change"),
    }
    let clock = ctx.ask_held(&Ask::SyncClock)?;
    ctx.note(&format!(
        "  Held session, clock sync during feed {feed}: {}",
        compact(&clock)
    ));

    // Every wait comes before any verdict, so the restore that follows a
    // failure finds the owner at rest.
    let fed = ctx.expect(
        &format!("PLOWN1 fed id={feed}"),
        moving,
        FEED_SECONDS,
        |r| fed_of(r, feed).is_some(),
    )?;
    ctx.expect("the owner at rest", fed, STOP_SECONDS, |r| {
        owner_state(r, IDLE)
    })?;

    let writes = ctx.log.between(start - 1, fed, store_write);
    require(
        writes.is_empty(),
        format!("Store writes during feed {feed}: {}", writes.join(" | ")),
    )?;
    let result = result_at(ctx, fed, feed)?;
    require(
        result.outcome == Some(OwnerOutcome::Done)
            && result.requested == PORTIONS
            && result.completed == PORTIONS,
        format!("Feed {feed} did not end Done {PORTIONS}/{PORTIONS}: {result:?}"),
    )?;
    require(
        settings == Answer::Refused(Refusal::Busy),
        format!(
            "A settings change during feed {feed} answered {}, not busy",
            compact(&settings)
        ),
    )?;
    let after = read_settings(ctx)?;
    require(
        changes_to(&after) == changes_to(before),
        format!(
            "The settings read {} after the change the feed refused",
            compact(&after)
        ),
    )?;
    Ok(change)
}

/// Feed again, and cancel it from the held session once the motor drives.
fn cancel_during_feed(ctx: &mut Ctx) -> Result<()> {
    // Also a keepalive: the feeder drops a session quiet for 90 s.
    let answer = ctx.ask_held(&Ask::Status)?;
    require(
        matches!(answer, Answer::Status(_)),
        format!("The held session's status answered {}", compact(&answer)),
    )?;

    let (feed, start) = start_feed(ctx)?;
    let moving = ctx.expect("the motor driving", start, PREPARE_SECONDS, |r| {
        owner_state(r, FEEDING)
    })?;
    ctx.idle(CANCEL_AFTER_SECONDS)?;
    let since = ctx.mark()?;
    let cancel = ctx.ask_held(&Ask::CancelFeed { feed })?;
    ctx.note(&format!(
        "  Held session, cancel of feed {feed}: {}",
        compact(&cancel)
    ));

    let fed = ctx.expect(
        &format!("PLOWN1 fed id={feed}"),
        moving,
        FEED_SECONDS,
        |r| fed_of(r, feed).is_some(),
    )?;
    ctx.expect("the owner at rest", fed, STOP_SECONDS, |r| {
        owner_state(r, IDLE)
    })?;

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
    let stopped = ctx.log.between(since, fed, |r| {
        r.tag == OWNER
            && r.field("state").is_some_and(|s| s != FEEDING)
            && r.field("stopped") == Some("1")
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

/// Ask for a feed on the suite's session; its id and the mark just past
/// the owner taking it.
fn start_feed(ctx: &mut Ctx) -> Result<(u32, usize)> {
    let since = ctx.mark()?;
    let answer = ctx.ask(&Ask::Feed { portions: PORTIONS })?;
    let Answer::Feeding { feed } = answer else {
        return Err(Error::Bench(format!(
            "The feed answered {}",
            compact(&answer)
        )));
    };
    ctx.note(&format!("  Feed {feed}: {PORTIONS} portions"));

    let id = feed.to_string();
    let start = ctx.expect(
        &format!("PLOWN1 feed id={feed}"),
        since,
        RECORD_SECONDS,
        |r| r.tag == OWNER && r.first == "feed" && r.field("id") == Some(id.as_str()),
    )?;
    Ok((feed, start))
}

fn owner_state(record: &Record, state: &str) -> bool {
    record.tag == OWNER && record.field("state") == Some(state)
}

/// A record the settings writer logs once a store write lands, or the
/// build confirmation's.
fn store_write(record: &Record) -> bool {
    match record.tag {
        tag::SETTINGS => {
            record.first == "saved"
                || record.body_starts("net saved")
                || record.body_starts("keys change")
        }
        tag::CONFIRM => true,
        _ => false,
    }
}

/// What `PLOWN1 fed` reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fed {
    requested: u8,
    completed: u8,
    outcome: Option<OwnerOutcome>,
    reason: Option<FeedReason>,
}

/// `record` as feed `feed`'s result, if it is one.
fn fed_of(record: &Record, feed: u32) -> Option<Fed> {
    if record.tag != OWNER || record.first != "fed" || record.number::<u32>("id") != Some(feed) {
        return None;
    }
    Some(Fed {
        requested: record.number("requested")?,
        completed: record.number("completed")?,
        outcome: record.number("outcome").and_then(OwnerOutcome::from_byte),
        reason: record.number("reason").and_then(FeedReason::from_byte),
    })
}

/// The result on the line just before `mark` (an [`Ctx::expect`] result).
fn result_at(ctx: &Ctx, mark: usize, feed: u32) -> Result<Fed> {
    let line = &ctx.log.line(mark - 1).text;
    Record::all(line)
        .find_map(|r| fed_of(&r, feed))
        .ok_or_else(|| Error::Bench(format!("Feed {feed}'s result does not parse: {line}")))
}

/// No restart since `since`, on the UART or in the boot count.
fn no_reset(ctx: &mut Ctx, since: usize, boots: u32) -> Result<()> {
    let restarts = ctx.log.between(since, usize::MAX, |r| {
        (r.tag == tag::START && r.first == "start") || r.tag == tag::PANIC
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
