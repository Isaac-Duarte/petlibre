//! Bearer tokens' rows, against real SQLite: found by their hash with
//! their user as the user is now, and gone with the user.

use std::sync::atomic::{AtomicU64, Ordering};

use jiff::Timestamp;
use wellfed_api::{Household, Role, TimeFormat, Token, TokenId, Units, UserId};
use wellfed_store::{NewToken, NewUser, Store};
use tempfile::TempDir;
use uuid::Uuid;

async fn fresh() -> (TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("server.db"), at(0))
        .await
        .unwrap();
    (dir, store)
}

fn fresh_id() -> Uuid {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    Uuid::from_u128(u128::from(NEXT.fetch_add(1, Ordering::Relaxed)))
}

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millisecond(ms).unwrap()
}

fn user(name: &str) -> NewUser<'_> {
    NewUser {
        id: UserId(fresh_id()),
        name,
        password_hash: "$argon2id$hash",
        role: Role::Owner,
        expires: None,
        created: at(1_000),
    }
}

fn household() -> Household {
    Household {
        name: "Home".into(),
        units: Units::Grams,
        time_format: TimeFormat::H24,
        zone: "Europe/Berlin".into(),
    }
}

#[tokio::test]
async fn a_token_is_found_by_its_hash_with_its_user_as_they_are_now() {
    let (_dir, store) = fresh().await;
    let ada = user("ada");
    let ada_id = ada.id;
    store.create_first_owner(ada, &household()).await.unwrap();
    let bob = user("bob");
    let bob_id = bob.id;
    store.create_user(bob).await.unwrap();
    let token = |n: u8, user| NewToken {
        id: TokenId(fresh_id()),
        hash: if n == 1 { &[1; 32] } else { &[2; 32] },
        user,
        name: "phone",
        created: at(5),
        expires: (n == 2).then(|| at(9_000)),
    };
    let (adas, bobs) = (token(1, ada_id), token(2, bob_id));
    let (adas_id, bobs_id) = (adas.id, bobs.id);
    store.create_token(adas).await.unwrap();
    store.create_token(bobs).await.unwrap();

    let found = store.token_use(&[2; 32]).await.unwrap().unwrap();
    assert_eq!(found.token.id, bobs_id);
    assert_eq!(found.token.expires, Some(at(9_000)));
    assert_eq!(found.token.last_used, None);
    assert_eq!(found.user.name, "bob");
    assert!(store.token_use(&[3; 32]).await.unwrap().is_none());

    store.touch_token(bobs_id, at(50)).await.unwrap();
    store.touch_token(bobs_id, at(40)).await.unwrap();
    let touched = store.token(bobs_id).await.unwrap().unwrap();
    assert_eq!(touched.last_used, Some(at(50)), "never moves back");

    store.update_user(bob_id, Role::Viewer, None).await.unwrap();
    let found = store.token_use(&[2; 32]).await.unwrap().unwrap();
    assert_eq!(found.user.role, Role::Viewer, "the role is read on use");

    let ids = |tokens: Vec<Token>| -> Vec<_> { tokens.into_iter().map(|token| token.id).collect() };
    assert_eq!(ids(store.tokens(None).await.unwrap()), [adas_id, bobs_id]);
    assert_eq!(ids(store.tokens(Some(bob_id)).await.unwrap()), [bobs_id]);

    store.delete_user(bob_id).await.unwrap();
    assert!(
        store.token_use(&[2; 32]).await.unwrap().is_none(),
        "gone with bob"
    );
    assert!(store.delete_token(adas_id).await.unwrap());
    assert!(!store.delete_token(adas_id).await.unwrap());
    assert_eq!(store.tokens(None).await.unwrap(), []);
}
