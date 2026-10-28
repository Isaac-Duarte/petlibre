//! What a retry does with its key's journal entry: replay the recorded
//! answer, resend under the first request id, or answer unknown unsent.

use wellfed_api::{CommandKind, CommandOutcome, CommandState};
use wellfed_store::CommandRow;

use crate::RequestId;

/// Whether a write whose outcome is unknown may go out again under its
/// request id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Resend {
    /// The feeder answers a repeated id from memory, and a write that
    /// left it runs again harmlessly or is refused as stale.
    SameId,
    /// Never: an uncertain write of this kind is resolved, not resent.
    Never,
}

/// The resend rule of each kind. The match is exhaustive, so a new kind
/// cannot be journalled without choosing one.
const fn resend(kind: CommandKind) -> Resend {
    match kind {
        // A cancel names its feed: run again, it stops nothing new, and
        // once the feed has ended it is refused `NotFound`. A revoke run
        // again finds the key gone (`NotFound`). A Wi-Fi change names its
        // base, as a settings change does. A tune played twice is only
        // heard twice. A desiccant reset run again dates the replacement
        // to the day it runs, which is what a retry of it asks.
        CommandKind::Settings
        | CommandKind::Desiccant
        | CommandKind::Lock
        | CommandKind::Cancel
        | CommandKind::Revoke
        | CommandKind::Wifi
        | CommandKind::Play => Resend::SameId,
        // The feeder's memory of a feed's id ends at a reboot.
        CommandKind::Feed => Resend::Never,
        // An invite's secret is not kept, so a resend could only make a
        // second invite nobody holds: the caller asks for a new one.
        CommandKind::Invite => Resend::Never,
        // A restart forgets the ids, so a reboot resent after one would
        // restart the feeder again; the boot count says whether it ran.
        CommandKind::Reboot => Resend::Never,
        // The feeder is archived as its key is revoked, and nothing sends
        // to an archived feeder.
        CommandKind::Remove => Resend::Never,
    }
}

/// What a retry does with its key's entry.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Retry {
    /// Answer with the recorded outcome; nothing is sent.
    Replay(CommandOutcome),
    /// Send it again under its request id.
    Resend(RequestId),
    /// Answer unknown; nothing is sent.
    Unknown,
}

/// The retry of `row`'s request, its key and body already matched.
pub(super) fn retry(row: &CommandRow) -> Retry {
    match row.state {
        CommandState::Answered | CommandState::Resolved => {
            row.outcome.clone().map_or(Retry::Unknown, Retry::Replay)
        }
        // Another attempt in this server is sending it.
        CommandState::Sent => Retry::Unknown,
        CommandState::Unknown => match resend(row.kind) {
            Resend::SameId => Retry::Resend(row.request_id),
            Resend::Never => Retry::Unknown,
        },
    }
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;
    use uuid::Uuid;
    use wellfed_api::{FeederId, FeederRefusal, UserId};

    use super::*;

    fn entry(
        kind: CommandKind,
        state: CommandState,
        outcome: Option<CommandOutcome>,
    ) -> CommandRow {
        CommandRow {
            seq: 1,
            key: [1; 16],
            user: UserId(Uuid::from_u128(1)),
            token: None,
            feeder: FeederId(Uuid::from_u128(2)),
            kind,
            body_hash: [3; 32],
            request_id: [4; 16],
            boot_count: Some(5),
            history: None,
            state,
            outcome,
            feed: None,
            sent: Timestamp::UNIX_EPOCH,
            updated: Timestamp::UNIX_EPOCH,
        }
    }

    /// A feed whose outcome is unknown is answered unknown and never sent
    /// again, whatever the feeder may still remember of its id.
    #[test]
    fn an_unknown_feed_is_never_resent() {
        let feed = entry(CommandKind::Feed, CommandState::Unknown, None);

        assert_eq!(resend(CommandKind::Feed), Resend::Never);
        assert_eq!(retry(&feed), Retry::Unknown);
    }

    #[test]
    fn a_retry_replays_an_answer_waits_out_a_send_and_resends_only_by_rule() {
        let refused = CommandOutcome::Refused {
            refusal: FeederRefusal::Busy,
        };
        for kind in CommandKind::ALL {
            let answered = entry(kind, CommandState::Answered, Some(refused.clone()));
            assert_eq!(retry(&answered), Retry::Replay(refused.clone()), "{kind:?}");
            let resolved = entry(kind, CommandState::Resolved, Some(CommandOutcome::Done));
            assert_eq!(retry(&resolved), Retry::Replay(CommandOutcome::Done));
            let in_flight = entry(kind, CommandState::Sent, None);
            assert_eq!(retry(&in_flight), Retry::Unknown, "{kind:?}");
        }

        for kind in CommandKind::ALL {
            let unknown = entry(kind, CommandState::Unknown, None);
            let want = match resend(kind) {
                Resend::SameId => Retry::Resend([4; 16]),
                Resend::Never => Retry::Unknown,
            };
            assert_eq!(retry(&unknown), want, "{kind:?}");
        }
        let resent = CommandKind::ALL.map(resend);
        assert_eq!(
            resent,
            [
                Resend::SameId,
                Resend::SameId,
                Resend::Never,
                Resend::SameId,
                Resend::Never,
                Resend::SameId,
                Resend::SameId,
                Resend::Never,
                Resend::SameId,
                Resend::Never,
                Resend::SameId,
            ],
            "the rule of each kind, in the order of CommandKind::ALL"
        );
    }
}
