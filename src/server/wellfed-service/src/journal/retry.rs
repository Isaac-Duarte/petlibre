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
        CommandKind::Settings | CommandKind::Lock => Resend::SameId,
        // The feeder's memory of a feed's id ends at a reboot.
        CommandKind::Feed => Resend::Never,
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
    use wellfed_api::{FeederId, FeederRefusal, UserId};
    use uuid::Uuid;

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
        for kind in [CommandKind::Settings, CommandKind::Lock, CommandKind::Feed] {
            let answered = entry(kind, CommandState::Answered, Some(refused.clone()));
            assert_eq!(retry(&answered), Retry::Replay(refused.clone()), "{kind:?}");
            let resolved = entry(kind, CommandState::Resolved, Some(CommandOutcome::Done));
            assert_eq!(retry(&resolved), Retry::Replay(CommandOutcome::Done));
            let in_flight = entry(kind, CommandState::Sent, None);
            assert_eq!(retry(&in_flight), Retry::Unknown, "{kind:?}");
        }

        for kind in [CommandKind::Settings, CommandKind::Lock] {
            let unknown = entry(kind, CommandState::Unknown, None);
            assert_eq!(retry(&unknown), Retry::Resend([4; 16]), "{kind:?}");
        }
    }
}
