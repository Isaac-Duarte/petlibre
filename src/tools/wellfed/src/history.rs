//! Reading the feeder over the device API: its status, its events and
//! samples, its feeding history, its debug log, its statistics and one
//! feed's trace.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Args, ValueEnum};
use jiff::tz::TimeZone;
use wellfed_client::history::Entry;
use wellfed_client::trace::Pages;
use wellfed_model::{self as model, Answer, Gap, Op, Pushed, StatsSection};

use crate::text::{self, time};
use crate::{Target, connect, print_answer, print_json, trace};

#[derive(Args)]
pub(crate) struct StatusArgs {
    #[command(flatten)]
    target: Target,
    /// The status as JSON, as the wire spells it, instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub(crate) struct EventsArgs {
    #[command(flatten)]
    target: Target,
    /// Start after this sequence number, in the feeder's current epoch
    /// (default: the oldest kept).
    #[arg(long)]
    after: Option<u64>,
    /// Keep the session open and print new events (and samples) as they
    /// happen.
    #[arg(long)]
    follow: bool,
    /// With --follow, the live samples too (power, signal, memory, the
    /// loop, the gateway, Wi-Fi and the supply).
    #[arg(long)]
    samples: bool,
    /// Each message as JSON, one a line, as the wire spells it, instead of
    /// one line of text an event.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub(crate) struct FeedsArgs {
    #[command(flatten)]
    target: Target,
    /// Start after this event number (the first column), in the feeder's
    /// current epoch (default: the oldest kept). A storage wipe starts a
    /// new epoch counting from 1 again, so a number from before it would
    /// skip that many of the new epoch's events: start without it then.
    #[arg(long)]
    after: Option<u64>,
    /// Each entry as JSON, one a line, instead of one line a feed. An
    /// uncertain feed's `delivered` is a range, never a count.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub(crate) struct LogsArgs {
    #[command(flatten)]
    target: Target,
    /// Start after this line number of the feeder's current boot (default:
    /// the oldest line its RAM tail keeps, a few KB). Lines count from 1
    /// each boot, and the feeder cannot tell an earlier boot's number below
    /// this boot's newest from its own: after a restart, start without it.
    #[arg(long)]
    after: Option<u32>,
    /// Keep the session open and print new lines as the feeder logs them.
    #[arg(long)]
    follow: bool,
}

#[derive(Args)]
pub(crate) struct StatsArgs {
    #[command(flatten)]
    target: Target,
    /// One section; repeat for several (default: every section).
    #[arg(long, value_enum)]
    section: Vec<Section>,
}

#[derive(Clone, Copy, ValueEnum)]
enum Section {
    Counters,
    Memory,
    Flash,
    Power,
    Food,
    Feeds,
    Portions,
}

impl Section {
    const fn model(self) -> StatsSection {
        match self {
            Self::Counters => StatsSection::Counters,
            Self::Memory => StatsSection::Memory,
            Self::Flash => StatsSection::Flash,
            Self::Power => StatsSection::Power,
            Self::Food => StatsSection::Food,
            Self::Feeds => StatsSection::Feeds,
            Self::Portions => StatsSection::Portions,
        }
    }
}

#[derive(Args)]
pub(crate) struct TraceArgs {
    #[command(flatten)]
    target: Target,
    /// The feed, as `wellfed feed` and the `Fed` event number it.
    feed: u32,
    /// The whole trace as JSON, instead of a summary and one line an entry.
    #[arg(long)]
    json: bool,
}

/// Each section asked for, one answer each, as JSON.
pub(crate) async fn stats_command(args: StatsArgs) -> Result<()> {
    let (_, _, mut connection) = connect(&args.target).await?;
    // An op the feeder predates would end the session rather than be
    // refused, so ask only a feeder that claims the statistics.
    if !connection.capabilities().stats {
        bail!("this feeder's build serves no statistics; update it first");
    }
    let sections: Vec<StatsSection> = if args.section.is_empty() {
        // A feeder that predates a section is not asked for it.
        let capabilities = connection.capabilities();
        StatsSection::ALL
            .into_iter()
            .filter(|&section| section != StatsSection::Feeds || capabilities.traces)
            .filter(|&section| section != StatsSection::Portions || capabilities.calibration)
            .collect()
    } else {
        args.section.iter().map(|section| section.model()).collect()
    };
    if sections.contains(&StatsSection::Feeds) && !connection.capabilities().traces {
        bail!("this feeder's build keeps no feed traces; update it first");
    }
    for section in sections {
        let answer = connection.ask(&Op::Stats { section }).await?;
        print_answer(&answer)?;
    }
    Ok(())
}

/// One feed's trace, read a page at a time.
pub(crate) async fn trace_command(args: TraceArgs) -> Result<()> {
    let (_, _, mut connection) = connect(&args.target).await?;
    // As for the statistics: a feeder that predates the op would end the
    // session on it.
    if !connection.capabilities().traces {
        bail!("this feeder's build keeps no feed traces; update it first");
    }
    let mut pages = Pages::new(args.feed);
    let trace = loop {
        let answer = connection.ask(&pages.next_op()).await?;
        if let Some(trace) = pages.take(answer).context("read the trace")? {
            break trace;
        }
    };
    if args.json {
        return print_json(&trace);
    }
    print!("{}", trace::Text(&trace));
    Ok(())
}

pub(crate) async fn events_command(args: EventsArgs) -> Result<()> {
    let (_, _, mut connection) = connect(&args.target).await?;
    connection
        .subscribe(args.after, args.samples && args.follow)
        .await?;
    let zone = TimeZone::system();
    // Without --follow, stop once the backlog has been quiet for a moment.
    let wait = if args.follow {
        Duration::from_secs(3600)
    } else {
        Duration::from_secs(2)
    };
    loop {
        // The session keeps itself alive, so a quiet hour is no failure.
        let pushed = match connection.next(wait).await {
            Ok(Some(pushed)) => pushed,
            Ok(None) if args.follow => continue,
            Ok(None) => return Ok(()),
            Err(error) => return Err(error),
        };
        if args.json {
            println!("{}", serde_json::to_string(&pushed)?);
        } else {
            println!("{}", text::pushed(&pushed, &zone));
        }
    }
}

/// The status, as text in this computer's time zone or as JSON.
pub(crate) async fn status_command(args: StatusArgs) -> Result<()> {
    let (_, _, mut connection) = connect(&args.target).await?;
    let answer = connection.ask(&Op::Status).await?;
    match answer {
        Answer::Status(status) if !args.json => {
            println!("{}", text::status(&status, &TimeZone::system()));
            Ok(())
        }
        answer => print_answer(&answer),
    }
}

/// Each feed the feeder's history keeps, oldest first, one line a feed in
/// this computer's time zone.
pub(crate) async fn feeds_command(args: FeedsArgs) -> Result<()> {
    let (_, _, mut connection) = connect(&args.target).await?;
    connection.subscribe(args.after, false).await?;

    let zone = TimeZone::system();
    // The backlog is sent at once: stop once it has been quiet a moment.
    while let Some(pushed) = connection.next(Duration::from_secs(2)).await? {
        let Some(entry) = Entry::from_pushed(&pushed) else {
            continue;
        };
        if args.json {
            println!("{}", serde_json::to_string(&entry)?);
        } else {
            println!("{}", line(&entry, &zone));
        }
    }
    Ok(())
}

/// One history entry as a line: a feed's event number, time, number,
/// source, portions and ending, or what a gap or an overflow lost.
fn line(entry: &Entry, zone: &TimeZone) -> String {
    let feed = match *entry {
        Entry::Feed(feed) => feed,
        Entry::Gap(Gap { from, to, .. }) => {
            return format!(
                "events {from}-{to} left the feeder's history before they were read; feeds among them are gone"
            );
        }
        Entry::Lost { seq, at, count, .. } => {
            return format!(
                "{seq:>6}  {}  the feeder could not record {count} events; feeds among them are gone",
                time(at, zone)
            );
        }
    };

    format!(
        "{:>6}  {}  {}",
        feed.seq,
        time(feed.at, zone),
        text::fed(&feed)
    )
}

/// The lines as the feeder logged them, one per line on standard output;
/// lines the feeder overwrote before sending are counted on standard error.
pub(crate) async fn logs_command(args: LogsArgs) -> Result<()> {
    let (_, _, mut connection) = connect(&args.target).await?;
    match connection.ask(&Op::TailLog { after: args.after }).await? {
        Answer::Done { .. } => {}
        Answer::Refused(model::Refusal::Unavailable) => {
            bail!("this feeder's build keeps no log (it lacks the log capability)")
        }
        Answer::Refused(refusal) => bail!("refused: {refusal:?}"),
        other => bail!("unexpected answer {other:?}"),
    }
    // Without --follow, stop once the tail has been sent and is quiet, or
    // after a bound: a feeder logging steadily is never quiet.
    let wait = if args.follow {
        Duration::from_secs(3600)
    } else {
        Duration::from_secs(2)
    };
    let until = std::time::Instant::now() + LOGS_BOUND;
    loop {
        if !args.follow && std::time::Instant::now() >= until {
            return Ok(());
        }
        let pushed = match connection.next(wait).await? {
            Some(pushed) => pushed,
            None if args.follow => continue,
            None => return Ok(()),
        };
        let Pushed::Log(batch) = pushed else {
            continue;
        };
        if batch.dropped > 0 {
            eprintln!("({} lines not in the tail)", batch.dropped);
        }
        for line in batch.lines {
            println!("{line}");
        }
    }
}

/// How long `logs` without `--follow` reads: the whole tail is a few
/// frames, sent within a second.
const LOGS_BOUND: Duration = Duration::from_secs(10);

#[cfg(test)]
mod tests {
    use jiff::Timestamp;
    use wellfed_client::history::Feed;
    use wellfed_model::{Event, EventBody, FeedOutcome, FeedReason, FoodLevel, Hex, Source};

    use super::*;

    fn feed(at: Option<Timestamp>, outcome: FeedOutcome, reason: FeedReason, by: Source) -> Entry {
        let event = Event {
            epoch: 1,
            seq: 1175,
            at,
            body: EventBody::Fed {
                feed: 128,
                requested: 3,
                completed: 1,
                outcome,
                reason,
                hopper: FoodLevel::Present,
                by,
            },
        };
        Entry::Feed(Feed::from_event(&event).unwrap())
    }

    #[test]
    fn a_feed_reads_as_one_line_with_its_time_source_portions_and_ending() {
        let at = Some(Timestamp::from_second(1_793_100_000).unwrap());
        let plan = Source::Schedule { plan: 0 };
        let failed = feed(at, FeedOutcome::Failed, FeedReason::Output, plan);

        assert_eq!(
            line(&failed, &TimeZone::UTC),
            "  1175  2026-10-27 11:20:00 +00:00  feed 128  by plan 0  1 of 3 portions, failed (output)"
        );
        let denver = TimeZone::fixed(jiff::tz::offset(-6));
        assert!(line(&failed, &denver).contains("2026-10-27 05:20:00 -06:00  feed 128"));
        let client = Source::Client {
            client: Hex([0xab, 0xcd, 0x01, 0x02]),
        };
        let unset = feed(None, FeedOutcome::Done, FeedReason::Done, client);
        assert!(
            line(&unset, &TimeZone::UTC).contains("(clock not set)  feed 128  by client abcd0102")
        );
    }

    #[test]
    fn an_uncertain_feed_reads_as_a_range() {
        let uncertain = feed(
            None,
            FeedOutcome::Uncertain,
            FeedReason::Output,
            Source::Panel,
        );

        assert!(
            line(&uncertain, &TimeZone::UTC)
                .ends_with("by panel  1 to 3 of 3 portions, uncertain (output)")
        );
    }

    #[test]
    fn a_feed_that_read_the_hopper_empty_says_so() {
        let Entry::Feed(mut empty) = feed(None, FeedOutcome::Done, FeedReason::Done, Source::Panel)
        else {
            unreachable!("feed builds a feed")
        };
        empty.hopper = FoodLevel::Empty;

        assert!(
            line(&Entry::Feed(empty), &TimeZone::UTC)
                .ends_with("1 of 3 portions, done (done)  hopper read empty")
        );
    }

    #[test]
    fn an_interrupted_feed_reads_by_unknown() {
        let interrupted = feed(
            None,
            FeedOutcome::Uncertain,
            FeedReason::Interrupted,
            Source::Feeder,
        );

        assert!(
            line(&interrupted, &TimeZone::UTC)
                .ends_with("by unknown  1 to 3 of 3 portions, uncertain (interrupted)")
        );
    }

    #[test]
    fn a_gap_or_an_overflow_says_its_feeds_are_gone() {
        let gap = Entry::Gap(Gap {
            epoch: 1,
            from: 1,
            to: 40,
        });
        let lost = Entry::Lost {
            epoch: 1,
            seq: 77,
            at: None,
            count: 3,
        };

        assert_eq!(
            line(&gap, &TimeZone::UTC),
            "events 1-40 left the feeder's history before they were read; feeds among them are gone"
        );
        assert_eq!(
            line(&lost, &TimeZone::UTC),
            "    77  (clock not set)  the feeder could not record 3 events; feeds among them are gone"
        );
    }
}
