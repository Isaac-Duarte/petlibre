//! A feed over the device API, under a fresh request id and never retried:
//! a feed whose answer is lost may have run, and is settled from the
//! feeder's history instead.

use std::time::Duration;

use anyhow::{Result, bail};
use clap::Args;
use wellfed_client::history::Feed;
use wellfed_model::{Answer, Event, Op, PORTIONS_MAX, Pushed, Refusal};

use crate::device::Connection;
use crate::{Target, ask_and_print, connect, print_json, text};

#[derive(Args)]
pub(crate) struct FeedArgs {
    #[command(flatten)]
    target: Target,
    /// Portions to dispense, 1-48.
    #[arg(long, default_value_t = 1, value_parser = crate::plans::portions())]
    portions: u8,
}

#[derive(Args)]
pub(crate) struct CancelFeedArgs {
    #[command(flatten)]
    target: Target,
    /// The feed's number, as `wellfed feed` printed it.
    feed: u32,
}

/// Longer than any feed the owner's limits allow.
const FEED_WAIT: Duration = Duration::from_secs(120);

/// One feed under a fresh id, never retried: a feed whose answer is lost
/// may have run. Prints the answer, then the feed's `Fed` event, with a
/// line on standard error saying what it asked for and what it delivered.
pub(crate) async fn feed_command(args: FeedArgs) -> Result<()> {
    let (_, _, mut connection) = connect(&args.target).await?;
    feed(&mut connection, args.portions).await
}

/// The feed on an open session: its answer, then its `Fed` event.
pub(crate) async fn feed(connection: &mut Connection, portions: u8) -> Result<()> {
    let Answer::Status(status) = connection.ask(&Op::Status).await? else {
        bail!("the feeder did not answer with its status");
    };
    // Only events after this one can be the feed's.
    connection.subscribe(status.last_event, false).await?;

    let answer = connection.ask(&Op::Feed { portions }).await?;
    print_json(&answer)?;
    let feed = started(&answer)?;

    loop {
        let Some(message) = connection.next(FEED_WAIT).await? else {
            bail!(
                "no result for feed {feed} after {} s; `wellfed events` shows it once recorded",
                FEED_WAIT.as_secs()
            );
        };
        if let Pushed::Event(event) = message
            && let Some(line) = fed(&event, feed)
        {
            print_json(&event)?;
            eprintln!("{line}");
            return Ok(());
        }
    }
}

/// The feed the answer started, or why it started none.
fn started(answer: &Answer) -> Result<u32> {
    match answer {
        Answer::Feeding { feed } => Ok(*feed),
        Answer::Refused(Refusal::Portions { asked }) => {
            bail!("the feeder refused {asked} portions: a feed asks for 1 to {PORTIONS_MAX}")
        }
        Answer::Refused(refusal) => bail!("the feeder did not start the feed: {refusal:?}"),
        _ => bail!("the feeder did not start the feed: {answer:?}"),
    }
}

/// `feed`'s result as one line, if `event` is its `Fed` event: the same
/// line `wellfed feeds` shows for it.
fn fed(event: &Event, feed: u32) -> Option<String> {
    Feed::from_event(event)
        .filter(|ended| ended.feed == feed)
        .map(|ended| text::fed(&ended))
}

pub(crate) async fn cancel_feed_command(args: CancelFeedArgs) -> Result<()> {
    ask_and_print(&args.target, &Op::CancelFeed { feed: args.feed }).await
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use wellfed_model::{EventBody, FeedOutcome, FeedReason, FoodLevel, Source};

    use super::*;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        feed: FeedArgs,
    }

    #[test]
    fn a_feed_asks_for_one_to_48_portions() {
        let parse = |portions: &str| Cli::try_parse_from(["feed", "--portions", portions]);

        assert_eq!(parse("1").unwrap().feed.portions, 1);
        assert_eq!(parse("48").unwrap().feed.portions, 48);
        for refused in ["0", "49", "255"] {
            assert!(parse(refused).is_err(), "{refused} portions parsed");
        }
        let default = Cli::try_parse_from(["feed"]).unwrap();
        assert_eq!(default.feed.portions, 1);
    }

    #[test]
    fn a_portions_refusal_names_the_value() {
        let refused = Answer::Refused(Refusal::Portions { asked: 49 });
        let error = started(&refused).unwrap_err().to_string();
        assert_eq!(
            error,
            "the feeder refused 49 portions: a feed asks for 1 to 48"
        );

        assert_eq!(started(&Answer::Feeding { feed: 7 }).unwrap(), 7);
        assert!(started(&Answer::Refused(Refusal::Busy)).is_err());
    }

    #[test]
    fn the_result_line_says_what_was_asked_for_and_delivered() {
        let fed_event = |outcome, completed, reason, hopper| Event {
            epoch: 1,
            seq: 40,
            at: None,
            body: EventBody::Fed {
                feed: 7,
                requested: 3,
                completed,
                outcome,
                reason,
                hopper,
                by: Source::Feeder,
            },
        };

        let failed = fed_event(
            FeedOutcome::Failed,
            2,
            FeedReason::NoFeedback,
            FoodLevel::Present,
        );
        assert_eq!(
            fed(&failed, 7).as_deref(),
            Some("feed 7  by feeder  2 of 3 portions, failed (no_feedback)")
        );
        assert_eq!(fed(&failed, 8), None);

        // A result the journal lost: the confirmed portions are a floor.
        let lost = fed_event(
            FeedOutcome::Uncertain,
            2,
            FeedReason::Output,
            FoodLevel::Present,
        );
        assert_eq!(
            fed(&lost, 7).as_deref(),
            Some("feed 7  by feeder  2 to 3 of 3 portions, uncertain (output)")
        );

        let empty = fed_event(FeedOutcome::Done, 3, FeedReason::Done, FoodLevel::Low);
        assert_eq!(
            fed(&empty, 7).as_deref(),
            Some("feed 7  by feeder  3 of 3 portions, done (done)  food read low")
        );
    }
}
