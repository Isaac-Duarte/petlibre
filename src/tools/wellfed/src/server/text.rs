//! The API's bodies as text for a person: one line an item, in the API's
//! own words (`online`, `feeder_refused`), so the text and `--json` name
//! things alike.

use std::fmt::Write as _;

use jiff::tz::TimeZone;
use wellfed_client::history::Delivered;
use wellfed_model::Event;
use wellfed_server_client::api::{
    Command, CommandOutcome, Feeder, FeederDetail, FeederEvent, Gap, Install, InstallTarget,
    Release, StockBackup, StockState, Token, User,
};

pub(super) use crate::text::{settings, word};

/// An optional value, or a dash.
fn or_dash(value: Option<impl ToString>) -> String {
    value.map_or_else(|| "-".to_owned(), |value| value.to_string())
}

pub(super) fn feeder(feeder: &Feeder) -> String {
    format!(
        "{}  {}  {}  {}  {}",
        feeder.id,
        feeder.name,
        word(&feeder.connection),
        word(&feeder.role),
        or_dash(feeder.address.as_deref()),
    )
}

pub(super) fn status(detail: &FeederDetail) -> String {
    let mut text = feeder(&detail.feeder);
    let Some(snapshot) = &detail.status else {
        text.push_str("\nNo status read yet.");
        return text;
    };

    let read = if snapshot.live { "live" } else { "last read" };
    // Writing to a String cannot fail.
    let _ = write!(
        text,
        "\nStatus ({read} {}):\n{}",
        snapshot.at,
        crate::text::status(&snapshot.status, &TimeZone::system()),
    );
    text
}

/// The reminder alone, from a feeder's status.
pub(super) fn desiccant(detail: &FeederDetail) -> String {
    match &detail.status {
        Some(snapshot) if snapshot.live => format!("Desiccant: {}", snapshot.status.desiccant),
        Some(snapshot) => format!(
            "Desiccant: {} (last read {})",
            snapshot.status.desiccant, snapshot.at
        ),
        None => "No status read yet.".to_owned(),
    }
}

/// An event as the server mirrored it, in this computer's time zone.
pub(super) fn event(event: &FeederEvent) -> String {
    let zone = TimeZone::system();
    let model = Event {
        epoch: event.epoch,
        seq: event.seq,
        at: event.at,
        body: event.body,
    };
    format!(
        "{}.{}  {}  {}",
        event.epoch,
        event.seq,
        crate::text::time(event.at, &zone),
        crate::text::event(&model, &zone),
    )
}

pub(super) fn gap(gap: &Gap) -> String {
    format!(
        "{}.{}-{}  (not kept by the feeder)",
        gap.epoch, gap.from, gap.to
    )
}

/// A journal entry: its key, kind, state and outcome.
pub(super) fn command(command: &Command) -> String {
    format!(
        "{}  {}  {}  {}  {}",
        command.key,
        word(&command.kind),
        word(&command.state),
        command
            .outcome
            .as_ref()
            .map_or_else(|| "-".to_owned(), outcome),
        command.sent,
    )
}

pub(super) fn outcome(outcome: &CommandOutcome) -> String {
    match outcome {
        CommandOutcome::Done => "done".to_owned(),
        CommandOutcome::Settings { settings: s } => format!("settings: {}", settings(s)),
        CommandOutcome::Refused { refusal } => format!("refused: {}", word(refusal)),
        CommandOutcome::Feeding { feed } => format!("feed {feed} running"),
        CommandOutcome::Invited { invite, seconds } => {
            format!("invite {invite} made, live {seconds} s")
        }
        CommandOutcome::Fed {
            feed,
            requested,
            completed,
            outcome,
            reason,
        } => format!(
            "feed {feed}: {} ({})",
            Delivered::of(*outcome, *completed, *requested).describe(*requested, *outcome),
            word(reason)
        ),
        CommandOutcome::Unrecorded => {
            "unrecorded: the feeder restarted with no record of the feed".to_owned()
        }
    }
}

pub(super) fn user(user: &User) -> String {
    format!(
        "{}  {}  {}  expires {}",
        user.id,
        user.name,
        word(&user.role),
        or_dash(user.expires)
    )
}

pub(super) fn token(token: &Token) -> String {
    format!(
        "{}  {}  user {}  created {}  last used {}  expires {}",
        token.id,
        token.name,
        token.user,
        token.created,
        or_dash(token.last_used),
        or_dash(token.expires),
    )
}

pub(super) fn release(release: &Release) -> String {
    format!(
        "version {}  {}  build {}  {} bytes  signed by {}  added {}",
        release.version,
        release.id,
        release.build_id,
        release.image_len,
        word(&release.signer),
        release.added,
    )
}

pub(super) fn install(install: &Install) -> String {
    let target = match install.target {
        InstallTarget::Release { release } => format!("release {release}"),
        InstallTarget::Stock { sha256 } => format!("stock {sha256}"),
    };
    let mut line = format!(
        "install {}  {target}  version {}  {}  {} of {} bytes",
        install.id,
        install.version,
        word(&install.state),
        install.written,
        install.image_len,
    );
    if let Some(failure) = &install.failure {
        let _ = write!(line, "  {}", word(failure));
    }
    line
}

pub(super) fn stock_backup(backup: &StockBackup) -> String {
    format!(
        "stock {}  {}  {}  {} bytes  from {}  kept {}",
        backup.version,
        word(&backup.state),
        backup.sha256,
        backup.bytes,
        word(&backup.source),
        backup.kept,
    )
}

/// The kept backup, the feeder's banks and its last restore, a line each.
pub(super) fn stock(state: &StockState) -> String {
    let backup = state
        .backup
        .as_ref()
        .map_or_else(|| "no stock backup kept".to_owned(), stock_backup);
    let feeder = state.feeder_state.map_or_else(
        || "feeder: not read (offline, or the server's key is not an owner's)".to_owned(),
        |feeder| {
            format!(
                "feeder: bank {}  floor {}  {}",
                feeder.bank,
                feeder.floor,
                if feeder.readable {
                    "serves bank 1"
                } else {
                    "does not serve bank 1"
                },
            )
        },
    );
    let restore = state
        .restore
        .as_ref()
        .map_or_else(|| "no restore has run".to_owned(), install);
    format!("{backup}\n{feeder}\n{restore}")
}

/// Each item on its own line.
pub(super) fn lines<T>(items: &[T], line: impl Fn(&T) -> String) -> String {
    let lines: Vec<String> = items.iter().map(line).collect();
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use wellfed_server_client::api::{CommandState, FeedOutcome, FeedReason, Role};

    use super::*;

    #[test]
    fn values_read_as_the_api_spells_them() {
        assert_eq!(word(&Role::Owner), "owner");
        assert_eq!(word(&CommandState::Unknown), "unknown");
        let fed = CommandOutcome::Fed {
            feed: 4,
            requested: 2,
            completed: 1,
            outcome: FeedOutcome::Failed,
            reason: FeedReason::Done,
        };
        assert_eq!(outcome(&fed), "feed 4: 1 of 2 portions, failed (done)");
        let uncertain = CommandOutcome::Fed {
            feed: 5,
            requested: 3,
            completed: 0,
            outcome: FeedOutcome::Uncertain,
            reason: FeedReason::Interrupted,
        };
        assert_eq!(
            outcome(&uncertain),
            "feed 5: 0 to 3 of 3 portions, uncertain (interrupted)"
        );
    }
}
