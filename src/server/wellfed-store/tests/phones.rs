//! Phone keys against real SQLite: a key asked for again keeps its
//! token's record, and the reconcile marks a key whose account or token no
//! longer may hold one.

use jiff::Timestamp;
use wellfed_api::{ClientRole, FeederId, RevokeReason, TokenId, UserId};
use wellfed_store::{NewFeeder, NewPhone, Recorded, SealedKey, Store};
use tempfile::TempDir;
use uuid::Uuid;

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
            role: ClientRole::Owner,
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

fn phone(feeder: FeederId, key: u8, token: u128, until: i64) -> NewPhone {
    NewPhone {
        feeder,
        key: [key; 32],
        user: UserId(Uuid::from_u128(0x50)),
        token: TokenId(Uuid::from_u128(token)),
        role: ClientRole::Member,
        invited: at(10),
        invite_until: at(until),
    }
}

#[tokio::test]
async fn a_key_asked_for_again_raises_its_deadline_and_another_tokens_is_taken() {
    let (_dir, store, feeder) = fresh().await;

    assert_eq!(
        store.record_phone(&phone(feeder, 1, 7, 100)).await.unwrap(),
        Recorded::New
    );
    assert_eq!(
        store.record_phone(&phone(feeder, 1, 7, 50)).await.unwrap(),
        Recorded::Again { until: at(100) }
    );
    let kept = store.phone(feeder, &[1; 32]).await.unwrap().unwrap();
    assert_eq!(kept.invite_until, at(100), "never lowered");
    assert_eq!(
        store.record_phone(&phone(feeder, 1, 8, 200)).await.unwrap(),
        Recorded::Taken
    );

    let stale = store
        .settle_invite_until(feeder, &[1; 32], at(50), at(60))
        .await
        .unwrap();
    assert!(!stale, "another request's bound is not lowered");
    let settled = store
        .settle_invite_until(feeder, &[1; 32], at(100), at(60))
        .await
        .unwrap();
    assert!(settled);
    let set = store.phone(feeder, &[1; 32]).await.unwrap().unwrap();
    assert_eq!(set.invite_until, at(60));
}

/// The key's account and token were never stored (or are gone): the
/// reconcile marks it as a deleted account's, once.
#[tokio::test]
async fn the_reconcile_marks_a_key_whose_account_is_gone() {
    let (_dir, store, feeder) = fresh().await;
    store.record_phone(&phone(feeder, 2, 7, 100)).await.unwrap();

    assert_eq!(store.reconcile_phones(at(20)).await.unwrap(), [feeder]);

    let marked = store.phone(feeder, &[2; 32]).await.unwrap().unwrap();
    let revoking = marked.revoking.unwrap();
    assert_eq!(
        (revoking.reason, revoking.since),
        (RevokeReason::Deleted, at(20))
    );
    assert!(store.reconcile_phones(at(30)).await.unwrap().is_empty());
}
