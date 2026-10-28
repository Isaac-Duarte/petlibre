//! The audit log against real SQLite: account changes and the journal's
//! entries in one order, each journal entry read as the journal holds it.

use jiff::Timestamp;
use tempfile::TempDir;
use uuid::Uuid;
use wellfed_api::{
    AuditAction, ClientRole, CommandKind, CommandOutcome, CommandState, FeederId, TokenId, UserId,
};
use wellfed_store::{AuditFilter, AuditRecord, NewAudit, NewCommand, NewFeeder, SealedKey, Store};

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millisecond(ms).unwrap()
}

const ADA: UserId = UserId(Uuid::from_u128(5));
const BOB: UserId = UserId(Uuid::from_u128(6));

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

fn command(key: u8, user: UserId, feeder: FeederId) -> NewCommand {
    NewCommand {
        key: [key; 16],
        user,
        token: Some(TokenId(Uuid::from_u128(9))),
        feeder,
        kind: CommandKind::Lock,
        body_hash: [key; 32],
        request_id: [key; 16],
        boot_count: None,
        history: None,
        sent: at(i64::from(key) * 100),
    }
}

async fn change(store: &Store, ms: i64, user: UserId, name: &str) {
    let action = AuditAction::PasswordSet {
        account: user,
        name: name.into(),
    };
    store
        .record_audit(NewAudit {
            at: at(ms),
            user: Some(user),
            user_name: Some(name),
            token: None,
            action: &action,
        })
        .await
        .unwrap();
}

fn everyone(after: Option<u64>, limit: u32) -> AuditFilter {
    AuditFilter {
        user: None,
        after,
        limit,
    }
}

#[tokio::test]
async fn changes_and_journal_entries_share_one_order() {
    let (_dir, store, feeder) = fresh().await;

    change(&store, 50, ADA, "ada").await;
    store.journal(&command(1, ADA, feeder)).await.unwrap();
    change(&store, 150, BOB, "bob").await;
    store.journal(&command(2, BOB, feeder)).await.unwrap();
    store
        .answer_command(&[1; 16], &CommandOutcome::Done, at(120))
        .await
        .unwrap();

    let rows = store.audit(everyone(None, 100)).await.unwrap();
    let seen: Vec<_> = rows
        .iter()
        .map(|row| {
            (
                row.at,
                row.user,
                matches!(row.record, AuditRecord::Command(_)),
            )
        })
        .collect();
    assert_eq!(
        seen,
        [
            (at(50), Some(ADA), false),
            (at(100), Some(ADA), true),
            (at(150), Some(BOB), false),
            (at(200), Some(BOB), true),
        ]
    );
    let AuditRecord::Command(first) = &rows[1].record else {
        panic!("a journal entry");
    };
    assert_eq!(
        (first.state, first.outcome.clone()),
        (CommandState::Answered, Some(CommandOutcome::Done)),
        "as the journal holds it now"
    );
    assert_eq!(rows[1].token, first.token);

    let bobs = AuditFilter {
        user: Some(BOB),
        ..everyone(None, 100)
    };
    let bobs = store.audit(bobs).await.unwrap();
    assert_eq!(bobs.len(), 2);
    assert!(bobs.iter().all(|row| row.user == Some(BOB)));

    let after = store.audit(everyone(Some(rows[1].seq), 1)).await.unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].seq, rows[2].seq);
}

/// A journal entry takes its user's name as it is recorded; a write that
/// was never sent leaves the log with its journal row.
#[tokio::test]
async fn an_unsent_write_leaves_the_log() {
    let (_dir, store, feeder) = fresh().await;

    store.journal(&command(1, ADA, feeder)).await.unwrap();
    store.journal(&command(2, ADA, feeder)).await.unwrap();
    store.forget_unsent(&[2; 16]).await.unwrap();

    let rows = store.audit(everyone(None, 100)).await.unwrap();
    assert_eq!(rows.len(), 1);
    let AuditRecord::Command(kept) = &rows[0].record else {
        panic!("a journal entry");
    };
    assert_eq!(kept.key, [1; 16]);
    assert_eq!(rows[0].user_name, None, "no account by that id");
}
