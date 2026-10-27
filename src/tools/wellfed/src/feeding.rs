//! A feed over the device API, under a fresh request id and never retried:
//! a feed whose answer is lost may have run, and is settled from the
//! feeder's history instead.

use std::time::Duration;

use anyhow::{Result, bail};
use clap::Args;
use wellfed_model::{Answer, EventBody, Op, Pushed};

use crate::{Target, ask_and_print, connect, print_json};

#[derive(Args)]
pub(crate) struct FeedArgs {
    #[command(flatten)]
    target: Target,
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..))]
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
/// may have run. Prints the answer, then the feed's `Fed` event.
pub(crate) async fn feed_command(args: FeedArgs) -> Result<()> {
    let (_, _, mut connection) = connect(&args.target).await?;
    let Answer::Status(status) = connection.ask(&Op::Status).await? else {
        bail!("the feeder did not answer with its status");
    };
    // Only events after this one can be the feed's.
    connection.subscribe(status.last_event, false).await?;

    let answer = connection
        .ask(&Op::Feed {
            portions: args.portions,
        })
        .await?;
    print_json(&answer)?;
    let Answer::Feeding { feed } = answer else {
        bail!("the feeder did not start the feed: {answer:?}");
    };

    loop {
        let Some(message) = connection.next(FEED_WAIT).await? else {
            bail!(
                "no result for feed {feed} after {} s; `wellfed events` shows it once recorded",
                FEED_WAIT.as_secs()
            );
        };
        if let Pushed::Event(event) = message
            && matches!(event.body, EventBody::Fed { feed: ended, .. } if ended == feed)
        {
            return print_json(&event);
        }
    }
}

pub(crate) async fn cancel_feed_command(args: CancelFeedArgs) -> Result<()> {
    ask_and_print(&args.target, &Op::CancelFeed { feed: args.feed }).await
}
