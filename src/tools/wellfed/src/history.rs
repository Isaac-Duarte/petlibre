//! Reading what the feeder recorded over the device API: its events and
//! samples, its debug log, its statistics and one feed's trace.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Args, ValueEnum};
use wellfed_client::trace::Pages;
use wellfed_model::{self as model, Answer, Op, Pushed, StatsSection};

use crate::{Target, connect, print_answer, print_json, trace};

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
    #[arg(long)]
    samples: bool,
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
        println!("{}", serde_json::to_string(&pushed)?);
    }
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
