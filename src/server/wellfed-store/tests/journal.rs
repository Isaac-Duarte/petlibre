//! The command journal's rows and their states, against real SQLite.

use jiff::Timestamp;
use tempfile::TempDir;
use uuid::Uuid;
use wellfed_api::{
    ClientRole, CommandKind, CommandOutcome, CommandState, FeederId, FeederRefusal, UserId,
};
use wellfed_api::{FeedOutcome, FeedReason};
use wellfed_store::{CommandFilter, Journaled, NewCommand, NewFeeder, SealedKey, Store, Synced};

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millisecond(ms).unwrap()
}

async fn fresh() -> (TempDir, Store, FeederId) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("server.db"), at(0))
        .await
        .unwrap();
    let feeder = FeederId(Uuid::from_u128(1));
    store
        .insert_feeder(NewFeeder {
            id: feeder,
            identity: [1; 32],
            name: "kitchen",
            role: ClientRole::Server,
            address: None,
            key: SealedKey {
                nonce: [7; 12],
                sealed: vec![1, 2, 3],
            },
            created: at(1),
        })
        .await
        .unwrap();
    (dir, store, feeder)
}

fn command(key: u8, feeder: FeederId) -> NewCommand {
    NewCommand {
        key: [key; 16],
        user: UserId(Uuid::from_u128(5)),
        token: None,
        feeder,
        kind: CommandKind::Lock,
        body_hash: [key; 32],
        request_id: [key ^ 0xff; 16],
        boot_count: Some(3),
        history: Some(Synced { epoch: 7, seq: 9 }),
        sent: at(10),
    }
}

fn feed(key: u8, feeder: FeederId) -> NewCommand {
    NewCommand {
        kind: CommandKind::Feed,
        ..command(key, feeder)
    }
}

fn all(after: Option<u64>) -> CommandFilter {
    CommandFilter {
        user: None,
        feeder: None,
        state: None,
        after,
        limit: 100,
    }
}

#[tokio::test]
async fn a_key_is_journalled_once_and_its_entry_returned_after() {
    let (_dir, store, feeder) = fresh().await;

    assert_eq!(
        store.journal(&command(1, feeder)).await.unwrap(),
        Journaled::New
    );
    let mut again = command(1, feeder);
    again.body_hash = [9; 32];
    let Journaled::Existing(found) = store.journal(&again).await.unwrap() else {
        panic!("the key is taken");
    };
    assert_eq!(found.body_hash, [1; 32], "the first request's");
    assert_eq!(found.request_id, [0xfe; 16]);
    assert_eq!(found.boot_count, Some(3));
    assert_eq!(found.state, CommandState::Sent);
    assert_eq!((found.sent, found.updated), (at(10), at(10)));
    assert_eq!(store.commands(all(None)).await.unwrap().len(), 1);
}

#[tokio::test]
async fn an_answer_is_never_downgraded_and_only_an_unsent_entry_is_dropped() {
    let (_dir, store, feeder) = fresh().await;
    for key in 1..=3 {
        store.journal(&command(key, feeder)).await.unwrap();
    }

    let refused = CommandOutcome::Refused {
        refusal: FeederRefusal::Busy,
    };
    assert!(
        store
            .answer_command(&[1; 16], &refused, at(20))
            .await
            .unwrap()
    );
    assert!(!store.command_unknown(&[1; 16], at(30)).await.unwrap());
    store.forget_unsent(&[1; 16]).await.unwrap();
    let answered = store.command(&[1; 16]).await.unwrap().unwrap();
    assert_eq!(answered.state, CommandState::Answered);
    assert_eq!(answered.outcome, Some(refused.clone()));
    assert_eq!(answered.updated, at(20));

    assert!(store.command_unknown(&[2; 16], at(30)).await.unwrap());
    store.forget_unsent(&[2; 16]).await.unwrap();
    let unknown = store.command(&[2; 16]).await.unwrap().unwrap();
    assert_eq!(unknown.state, CommandState::Unknown);

    store.forget_unsent(&[3; 16]).await.unwrap();
    assert_eq!(store.command(&[3; 16]).await.unwrap(), None);

    // An entry that is gone changes nothing, and says so.
    assert!(
        !store
            .answer_command(&[3; 16], &refused, at(40))
            .await
            .unwrap()
    );
    assert!(!store.command_unknown(&[3; 16], at(40)).await.unwrap());
}

#[tokio::test]
async fn a_start_marks_what_was_left_sent_unknown() {
    let (_dir, store, feeder) = fresh().await;
    for key in 1..=3 {
        store.journal(&command(key, feeder)).await.unwrap();
    }
    store
        .answer_command(&[1; 16], &CommandOutcome::Done, at(20))
        .await
        .unwrap();

    assert_eq!(store.interrupt_commands(at(40)).await.unwrap(), 2);

    let states: Vec<_> = store
        .commands(all(None))
        .await
        .unwrap()
        .into_iter()
        .map(|row| (row.key[0], row.state))
        .collect();
    assert_eq!(
        states,
        [
            (1, CommandState::Answered),
            (2, CommandState::Unknown),
            (3, CommandState::Unknown),
        ]
    );
    let unknown = CommandFilter {
        state: Some(CommandState::Unknown),
        ..all(None)
    };
    let after_first = store.commands(unknown).await.unwrap()[0].seq;
    let rest = store
        .commands(CommandFilter {
            after: Some(after_first),
            ..unknown
        })
        .await
        .unwrap();
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0].key, [3; 16]);
}

/// A feed's number comes with its answer; the feeds still open to the
/// history are those in flight, unknown, answered with a number or
/// resolved `unrecorded`; and only a `Fed` resolution replaces
/// `unrecorded`, never the other way.
#[tokio::test]
async fn feeds_open_to_the_history_and_their_resolution() {
    let (_dir, store, feeder) = fresh().await;
    for key in 1..=5 {
        store.journal(&feed(key, feeder)).await.unwrap();
    }
    store.journal(&command(6, feeder)).await.unwrap();

    let feeding = CommandOutcome::Feeding { feed: 4 };
    assert!(
        store
            .answer_command(&[1; 16], &feeding, at(11))
            .await
            .unwrap()
    );
    let refused = CommandOutcome::Refused {
        refusal: FeederRefusal::Busy,
    };
    assert!(
        store
            .answer_command(&[2; 16], &refused, at(11))
            .await
            .unwrap()
    );
    assert!(store.command_unknown(&[3; 16], at(11)).await.unwrap());
    assert!(store.command_unknown(&[4; 16], at(11)).await.unwrap());

    let entry = store.command(&[1; 16]).await.unwrap().unwrap();
    assert_eq!(entry.feed, Some(4));
    assert_eq!(entry.history, Some(Synced { epoch: 7, seq: 9 }));
    assert!(store.feed_claimed(feeder, 7, 4).await.unwrap());
    assert!(!store.feed_claimed(feeder, 7, 5).await.unwrap());
    assert!(
        !store.feed_claimed(feeder, 8, 4).await.unwrap(),
        "another epoch's feed 4 is another feed"
    );

    let open = |rows: Vec<wellfed_store::CommandRow>| -> Vec<u8> {
        rows.iter().map(|row| row.key[0]).collect()
    };
    // The refusal is final, and the lock is no feed; 5 is still in flight.
    assert_eq!(open(store.open_feeds(feeder).await.unwrap()), [1, 3, 4, 5]);

    let fed_as = |feed| CommandOutcome::Fed {
        feed,
        requested: 1,
        completed: 1,
        outcome: FeedOutcome::Done,
        reason: FeedReason::Done,
    };
    let fed = fed_as(5);
    assert!(
        !store.resolve_command(&[5; 16], &fed, at(12)).await.unwrap(),
        "in flight"
    );
    assert!(
        !store.resolve_command(&[2; 16], &fed, at(12)).await.unwrap(),
        "refused"
    );
    assert!(store.resolve_command(&[3; 16], &fed, at(12)).await.unwrap());
    let unrecorded = CommandOutcome::Unrecorded;
    assert!(
        store
            .resolve_command(&[4; 16], &unrecorded, at(12))
            .await
            .unwrap()
    );
    assert!(
        !store
            .resolve_command(&[3; 16], &unrecorded, at(13))
            .await
            .unwrap()
    );
    assert_eq!(open(store.open_feeds(feeder).await.unwrap()), [1, 4, 5]);

    let later = fed_as(6);
    assert!(
        store
            .resolve_command(&[4; 16], &later, at(14))
            .await
            .unwrap()
    );
    let entry = store.command(&[4; 16]).await.unwrap().unwrap();
    assert_eq!((entry.state, entry.feed), (CommandState::Resolved, Some(6)));
    assert_eq!(entry.outcome, Some(later));
    assert!(
        !store.resolve_command(&[4; 16], &fed, at(15)).await.unwrap(),
        "final"
    );
    assert!(
        !store
            .answer_command(&[3; 16], &feeding, at(15))
            .await
            .unwrap(),
        "an answer never replaces a resolution"
    );
    assert_eq!(open(store.open_feeds(feeder).await.unwrap()), [1, 5]);
}
