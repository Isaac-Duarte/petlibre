//! The device verbs over the server: the feeders it holds, their status,
//! settings, lock, feeds and history, and the journal its writes go
//! through.
//!
//! Each write is sent once under a fresh key ([`super::write`]). A feed is
//! answered once the feeder has reserved it, and its result is the journal
//! entry the feeder's history settles; the command follows the stream for
//! it, opened before the feed is sent so the result cannot slip past.

use std::time::Duration;

use anyhow::{Result, bail};
use clap::{Args, ValueEnum};
use uuid::Uuid;
use wellfed_server_client::api::{
    Command, CommandOutcome, CommandQuery, CommandState, EventQuery, FeedOutcome, Feeder, FeederId,
    SettingsPatch, StreamEvent, StreamQuery, Window,
};
use wellfed_server_client::{Client, Lock, Updates};

use super::{Output, Settled, Written, text, write};
use crate::Toggle;

/// Longer than any feed the owner's limits allow, as the device API's
/// `feed` waits.
const FEED_WAIT: Duration = Duration::from_secs(120);

/// Which feeder: by its id or its name, or the only one.
#[derive(Args)]
pub(crate) struct Which {
    /// The feeder's id or name; needed when the server holds more than one.
    #[arg(long)]
    feeder: Option<String>,
}

impl Which {
    /// The feeder this names, from the server's list.
    pub(super) async fn id(&self, client: &Client) -> Result<FeederId> {
        let feeders = client.feeders().await?;
        let matching: Vec<&Feeder> = feeders
            .iter()
            .filter(|feeder| {
                self.feeder.as_deref().is_none_or(|which| {
                    feeder.name == which || feeder.id.to_string() == which.to_lowercase()
                })
            })
            .collect();

        let [feeder] = matching.as_slice() else {
            match &self.feeder {
                Some(which) => bail!("{} feeders are named or numbered {which:?}", matching.len()),
                None => bail!(
                    "the server holds {} feeders; pass --feeder with an id or a name",
                    matching.len()
                ),
            }
        };
        Ok(feeder.id)
    }
}

#[derive(Args)]
pub(crate) struct SettingsArgs {
    #[command(flatten)]
    which: Which,
    #[arg(long)]
    lights: Option<Toggle>,
    #[arg(long)]
    sound: Option<Toggle>,
    /// 0-100.
    #[arg(long)]
    volume: Option<u8>,
    /// When lit lights may show: `always`, or `HH:MM-HH:MM` on the
    /// feeder's local clock (inclusive; may wrap midnight).
    #[arg(long, value_name = "WINDOW")]
    lights_window: Option<Window>,
    /// When sounds may play: `always`, or `HH:MM-HH:MM` as for
    /// `--lights-window`.
    #[arg(long, value_name = "WINDOW")]
    sound_window: Option<Window>,
}

#[derive(Args)]
pub(crate) struct LockArgs {
    #[command(flatten)]
    which: Which,
    state: Toggle,
}

#[derive(Args)]
pub(crate) struct FeedArgs {
    #[command(flatten)]
    which: Which,
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..))]
    portions: u8,
}

#[derive(Args)]
pub(crate) struct CancelFeedArgs {
    #[command(flatten)]
    which: Which,
    /// The feed's number, as `wellfed server feed` printed it.
    feed: u32,
}

#[derive(Args)]
pub(crate) struct EventsArgs {
    #[command(flatten)]
    which: Which,
    /// The feeder's epoch (default: its current one).
    #[arg(long)]
    epoch: Option<u32>,
    /// Start after this sequence number (default: the oldest held).
    #[arg(long)]
    after: Option<u64>,
    /// At most this many (1-1000, default 100).
    #[arg(long)]
    limit: Option<u32>,
    /// Then keep following the stream and print new events as they come.
    #[arg(long)]
    follow: bool,
}

#[derive(Args)]
pub(crate) struct CommandArgs {
    /// The write's key, as the command that sent it printed.
    key: Uuid,
}

#[derive(Args)]
pub(crate) struct CommandsArgs {
    /// One feeder's writes only.
    #[arg(long)]
    feeder: Option<String>,
    /// Writes in this state only: `unknown` is the uncertain list.
    #[arg(long, value_enum)]
    state: Option<State>,
    /// The previous page's `next`, as it came.
    #[arg(long)]
    after: Option<String>,
    /// At most this many (1-1000, default 100).
    #[arg(long)]
    limit: Option<u32>,
}

#[derive(Clone, Copy, ValueEnum)]
enum State {
    Sent,
    Answered,
    Unknown,
    Resolved,
}

impl State {
    const fn api(self) -> CommandState {
        match self {
            Self::Sent => CommandState::Sent,
            Self::Answered => CommandState::Answered,
            Self::Unknown => CommandState::Unknown,
            Self::Resolved => CommandState::Resolved,
        }
    }
}

pub(super) async fn list(client: &Client, out: Output) -> Result<Settled> {
    let feeders = client.feeders().await?;
    out.show(&feeders, |feeders| text::lines(feeders, text::feeder))?;
    Ok(Settled::Known)
}

pub(super) async fn status(client: &Client, which: &Which, out: Output) -> Result<Settled> {
    let id = which.id(client).await?;
    let detail = client.feeder(id).await?;
    out.show(&detail, text::status)?;
    Ok(Settled::Known)
}

/// Without a change, the settings as the feeder holds them; with one, the
/// change against the generation just read, so a change made meanwhile is
/// refused (the current settings print) rather than overwritten.
pub(super) async fn settings(client: &Client, args: SettingsArgs, out: Output) -> Result<Settled> {
    let id = args.which.id(client).await?;
    let current = client.settings(id).await?;
    let patch = SettingsPatch {
        lights: args.lights.map(Toggle::on),
        sound: args.sound.map(Toggle::on),
        volume: args.volume,
        lights_window: args.lights_window,
        sound_window: args.sound_window,
    };
    if patch == SettingsPatch::default() {
        out.show(&current, text::settings)?;
        return Ok(Settled::Known);
    }

    let key = Uuid::new_v4();
    let sent = client.put_settings(id, current.generation, &patch, key);
    let Written::Answered(settings) = write(out, key, sent).await? else {
        return Ok(Settled::Unknown);
    };
    out.show(&settings, text::settings)?;
    Ok(Settled::Known)
}

pub(super) async fn lock(client: &Client, args: LockArgs, out: Output) -> Result<Settled> {
    let id = args.which.id(client).await?;
    let lock = if args.state.on() {
        Lock::Locked
    } else {
        Lock::Unlocked
    };

    let key = Uuid::new_v4();
    let Written::Answered(()) = write(out, key, client.set_lock(id, lock, key)).await? else {
        return Ok(Settled::Unknown);
    };
    out.text(match lock {
        Lock::Locked => "The panel is locked.",
        Lock::Unlocked => "The panel is unlocked.",
    });
    Ok(Settled::Known)
}

/// One feed under a fresh key, sent once: the entry the server answers
/// once the feeder has reserved it, then the entry its result settles.
pub(super) async fn feed(client: &Client, args: FeedArgs, out: Output) -> Result<Settled> {
    let id = args.which.id(client).await?;
    let mut updates = client.stream(&StreamQuery::default(), None).await?;

    let key = Uuid::new_v4();
    let sent = client.feed(id, args.portions, key);
    let Written::Answered(entry) = write(out, key, sent).await? else {
        return Ok(Settled::Unknown);
    };
    out.show(&entry, |entry| match entry.feed {
        Some(feed) => format!("Feed {feed} started (key {}).", entry.key),
        None => format!("Feed sent (key {}).", entry.key),
    })?;
    if entry.state == CommandState::Resolved {
        return settle(&entry, out);
    }

    let result = tokio::time::timeout(FEED_WAIT, resolved(&mut updates, key)).await;
    match result {
        Ok(Some(entry)) => settle(&entry, out),
        Ok(None) | Err(_) => {
            out.text(format!("No result yet after {} s.", FEED_WAIT.as_secs()));
            out.unknown(key, None)?;
            Ok(Settled::Unknown)
        }
    }
}

/// The entry under `key` once the stream reports it resolved; `None` if
/// the stream ends first.
async fn resolved(updates: &mut Updates, key: Uuid) -> Option<Command> {
    while let Some(update) = updates.next().await {
        let Ok(update) = update else {
            return None;
        };
        if let StreamEvent::Command { command } = update.event
            && command.key == key
            && command.state == CommandState::Resolved
        {
            return Some(command);
        }
    }
    None
}

pub(super) async fn cancel_feed(
    client: &Client,
    args: CancelFeedArgs,
    out: Output,
) -> Result<Settled> {
    let id = args.which.id(client).await?;

    let key = Uuid::new_v4();
    let sent = client.cancel_feed(id, args.feed, key);
    let Written::Answered(()) = write(out, key, sent).await? else {
        return Ok(Settled::Unknown);
    };
    out.text(format!("Feed {} is stopping.", args.feed));
    Ok(Settled::Known)
}

/// A page of the feeder's history, then with `--follow` its new events
/// and gaps from the stream.
pub(super) async fn events(client: &Client, args: EventsArgs, out: Output) -> Result<Settled> {
    let id = args.which.id(client).await?;
    // Open first, so nothing recorded between the page and the stream is
    // missed.
    let mut updates = match args.follow {
        true => Some(client.stream(&StreamQuery::default(), None).await?),
        false => None,
    };

    let query = EventQuery {
        epoch: args.epoch,
        after: args.after,
        limit: args.limit,
        kind: None,
    };
    let page = client.events(id, &query).await?;
    out.show(&page, |page| {
        let mut lines: Vec<String> = page.events.iter().map(text::event).collect();
        lines.extend(page.gaps.iter().map(text::gap));
        lines.join("\n")
    })?;
    if let (Some(next), false) = (page.next, args.follow) {
        eprintln!("More after {next}: --after {next}");
    }

    let Some(updates) = updates.as_mut() else {
        return Ok(Settled::Known);
    };
    // The stream opened before the page was read, so it may repeat the
    // page's newest events.
    let printed = page.events.last().map(|event| (event.epoch, event.seq));
    while let Some(update) = updates.next().await {
        let update = update?;
        match &update.event {
            StreamEvent::Event { feeder, event }
                if *feeder == id && Some((event.epoch, event.seq)) > printed =>
            {
                out.show(&update.event, |_| text::event(event))?;
            }
            StreamEvent::Gap { feeder, gap } if *feeder == id => {
                out.show(&update.event, |_| text::gap(gap))?;
            }
            _ => {}
        }
    }
    bail!("the server ended the stream")
}

/// The journal entry under a key: how a write whose outcome was unknown
/// is settled. Exits as the write would have: 3 while its outcome is
/// still unknown.
pub(super) async fn command(client: &Client, args: &CommandArgs, out: Output) -> Result<Settled> {
    let entry = client.command(args.key).await?;
    out.show(&entry, text::command)?;
    settled(&entry)
}

pub(super) async fn commands(client: &Client, args: CommandsArgs, out: Output) -> Result<Settled> {
    let feeder = match args.feeder {
        Some(feeder) => Some(
            Which {
                feeder: Some(feeder),
            }
            .id(client)
            .await?,
        ),
        None => None,
    };
    let query = CommandQuery {
        feeder,
        state: args.state.map(State::api),
        after: args.after,
        limit: args.limit,
    };

    let page = client.commands(&query).await?;
    out.show(&page, |page| text::lines(&page.items, text::command))?;
    if let Some(next) = &page.next {
        eprintln!("More after {next}: --after {next}");
    }
    Ok(Settled::Known)
}

/// A settled feed's result, printed: done is success, an uncertain or
/// unrecorded feed exits 3, and any other end is the command's failure.
fn settle(entry: &Command, out: Output) -> Result<Settled> {
    out.show(entry, |entry| {
        entry
            .outcome
            .as_ref()
            .map_or_else(String::new, text::outcome)
    })?;
    settled(entry)
}

/// What an entry says of its write, as an exit status: a refusal or a feed
/// that ended without its portions is an error; a write not yet answered,
/// a feed still running, and an uncertain or unrecorded feed are unknown.
fn settled(entry: &Command) -> Result<Settled> {
    let pending = matches!(entry.state, CommandState::Sent | CommandState::Unknown);
    match &entry.outcome {
        _ if pending => Ok(Settled::Unknown),
        None | Some(CommandOutcome::Feeding { .. } | CommandOutcome::Unrecorded) => {
            Ok(Settled::Unknown)
        }
        Some(
            CommandOutcome::Done | CommandOutcome::Settings { .. } | CommandOutcome::Invited { .. },
        ) => Ok(Settled::Known),
        Some(CommandOutcome::Refused { refusal }) => {
            bail!("the feeder refused it: {}", text::word(refusal))
        }
        Some(CommandOutcome::Fed { outcome, .. }) => match outcome {
            FeedOutcome::Done => Ok(Settled::Known),
            FeedOutcome::Uncertain => Ok(Settled::Unknown),
            FeedOutcome::Cancelled | FeedOutcome::Expired | FeedOutcome::Failed => {
                bail!("the feed ended {}", text::word(outcome))
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use wellfed_server_client::api::{CommandKind, FeedReason, UserId};

    use super::*;

    fn entry(state: CommandState, outcome: Option<CommandOutcome>) -> Command {
        Command {
            key: Uuid::nil(),
            user: UserId(Uuid::nil()),
            token: None,
            feeder: FeederId(Uuid::nil()),
            kind: CommandKind::Feed,
            state,
            outcome,
            feed: Some(1),
            boot_count: None,
            sent: jiff::Timestamp::UNIX_EPOCH,
            updated: jiff::Timestamp::UNIX_EPOCH,
        }
    }

    fn fed(outcome: FeedOutcome) -> Option<CommandOutcome> {
        Some(CommandOutcome::Fed {
            feed: 1,
            requested: 2,
            completed: 2,
            outcome,
            reason: FeedReason::Done,
        })
    }

    #[test]
    fn an_entry_exits_as_its_write_settled() {
        use CommandState::{Answered, Resolved, Sent, Unknown};

        let known = [
            entry(Answered, Some(CommandOutcome::Done)),
            entry(Resolved, fed(FeedOutcome::Done)),
        ];
        for entry in known {
            assert_eq!(settled(&entry).ok(), Some(Settled::Known), "{entry:?}");
        }

        let unknown = [
            entry(Sent, None),
            entry(Unknown, None),
            entry(Answered, Some(CommandOutcome::Feeding { feed: 1 })),
            entry(Resolved, fed(FeedOutcome::Uncertain)),
            entry(Resolved, Some(CommandOutcome::Unrecorded)),
        ];
        for entry in unknown {
            assert_eq!(settled(&entry).ok(), Some(Settled::Unknown), "{entry:?}");
        }

        let failed = [
            entry(Resolved, fed(FeedOutcome::Failed)),
            entry(Resolved, fed(FeedOutcome::Cancelled)),
            entry(
                Answered,
                Some(CommandOutcome::Refused {
                    refusal: wellfed_server_client::api::FeederRefusal::Busy,
                }),
            ),
        ];
        for entry in failed {
            assert!(settled(&entry).is_err(), "{entry:?}");
        }
    }
}
