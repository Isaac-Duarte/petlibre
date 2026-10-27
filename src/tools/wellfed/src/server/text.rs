//! The API's bodies as text for a person: one line an item, in the API's
//! own words (`online`, `feeder_refused`), so the text and `--json` name
//! things alike.

use std::fmt::Write as _;

use wellfed_server_client::api::{
    Command, CommandOutcome, Feeder, FeederDetail, FeederEvent, Gap, Release, Settings, Token, User,
};
use serde::Serialize;

/// A value as the API spells it: a bare word for an enum's name, its JSON
/// otherwise.
pub(super) fn word(value: &impl Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(word)) => word,
        Ok(other) => other.to_string(),
        Err(_) => "?".to_owned(),
    }
}

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

    let status = &snapshot.status;
    let read = if snapshot.live { "live" } else { "last read" };
    // Writing to a String cannot fail.
    let _ = write!(
        text,
        "\nStatus ({read} {}): build {}, bank {}, boot {}, up {} s\n\
         clock {}, locked {}, readiness {}, Wi-Fi {} dBm, sessions {}",
        snapshot.at,
        status.build,
        status.bank,
        status.boot_count,
        status.uptime_ms / 1000,
        or_dash(status.clock),
        or_dash(status.locked),
        status
            .readiness
            .map_or_else(|| "-".to_owned(), |r| word(&r)),
        status.rssi,
        status.sessions,
    );
    text
}

pub(super) fn settings(settings: &Settings) -> String {
    format!(
        "Generation {}: lights {}, sound {}, volume {}{}",
        settings.generation,
        on_off(settings.lights),
        on_off(settings.sound),
        settings.volume,
        if settings.writable {
            ""
        } else {
            " (not writable now)"
        },
    )
}

const fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

pub(super) fn event(event: &FeederEvent) -> String {
    format!(
        "{}.{}  {}  {}",
        event.epoch,
        event.seq,
        or_dash(event.at),
        serde_json::to_string(&event.body).unwrap_or_default(),
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
        CommandOutcome::Fed {
            feed,
            requested,
            completed,
            outcome,
            reason,
        } => format!(
            "feed {feed}: {completed} of {requested} portions, {} ({})",
            word(outcome),
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
    }
}
