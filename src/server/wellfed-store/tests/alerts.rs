//! Alerts and the reads their rules take: an alert is raised once per
//! standing condition and once ever per feed, cleared and acknowledged
//! once, listed by state and feeder; and the newest boot, clock sync,
//! food read and blocked spell come back as the rules need them.

use jiff::Timestamp;
use wellfed_api::{
    AlertCondition, AlertId, AlertState, Blocked, ClientRole, EventBody, FeedOutcome, FeedReason,
    FeederId, FoodStats, PetId, Source, Stats, UserId,
};
use wellfed_store::{Acked, AlertFilter, NewAlert, NewEvent, NewFeeder, SealedKey, Store};
use tempfile::TempDir;
use uuid::Uuid;

const ID: FeederId = FeederId(Uuid::from_u128(1));
const OTHER: FeederId = FeederId(Uuid::from_u128(2));

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millisecond(ms).unwrap()
}

async fn fresh() -> (TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("server.db"), at(0))
        .await
        .unwrap();
    store.refresh_projections(decode).await.unwrap();
    for (id, identity) in [(ID, 1), (OTHER, 2)] {
        store
            .insert_feeder(NewFeeder {
                id,
                identity: [identity; 32],
                name: "kitchen",
                role: ClientRole::Server,
                address: None,
                key: SealedKey {
                    nonce: [7; 12],
                    sealed: vec![1, 2, 3],
                },
                created: at(0),
            })
            .await
            .unwrap();
    }
    (dir, store)
}

fn decode(raw: &[u8]) -> Option<EventBody> {
    serde_json::from_slice(raw).ok()
}

async fn event(store: &Store, seq: u64, received: i64, valid: bool, body: EventBody) {
    let raw = serde_json::to_vec(&body).unwrap();
    store
        .record_event(
            ID,
            NewEvent {
                epoch: 1,
                seq,
                at: valid.then(|| Timestamp::from_second(received / 1000).unwrap()),
                received: at(received),
                body,
                raw: &raw,
            },
        )
        .await
        .unwrap();
}

fn alert(n: u128, feeder: FeederId, condition: AlertCondition, opened: i64) -> NewAlert {
    NewAlert {
        id: AlertId(Uuid::from_u128(n)),
        feeder,
        condition,
        since: at(opened - 10),
        opened: at(opened),
    }
}

fn filter(state: Option<AlertState>, feeder: Option<FeederId>) -> AlertFilter {
    AlertFilter {
        state,
        feeder,
        after: None,
        limit: 100,
    }
}

#[tokio::test]
async fn a_standing_condition_is_one_alert_until_it_clears() {
    let (_dir, store) = fresh().await;

    let first = store
        .raise_alert(&alert(1, ID, AlertCondition::Offline, 100))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.state, AlertState::Open);
    let twice = store
        .raise_alert(&alert(2, ID, AlertCondition::Offline, 200))
        .await
        .unwrap();
    assert_eq!(twice, None, "it still stands");
    let elsewhere = store
        .raise_alert(&alert(3, OTHER, AlertCondition::Offline, 200))
        .await
        .unwrap();
    assert!(elsewhere.is_some(), "another feeder's is its own");

    let cleared = store.clear_alert(first.id, at(300)).await.unwrap().unwrap();
    assert_eq!(
        (cleared.state, cleared.cleared),
        (AlertState::Cleared, Some(at(300)))
    );
    assert_eq!(store.clear_alert(first.id, at(400)).await.unwrap(), None);
    let again = store
        .raise_alert(&alert(4, ID, AlertCondition::Offline, 500))
        .await
        .unwrap();
    assert!(again.is_some(), "a new spell is a new alert");
    assert_eq!(store.alert(first.id).await.unwrap(), Some(cleared));
}

#[tokio::test]
async fn a_feeds_alert_is_raised_once_and_never_standing() {
    let (_dir, store) = fresh().await;
    let feed = AlertCondition::Feed {
        epoch: 1,
        feed: 7,
        outcome: FeedOutcome::Uncertain,
        reason: FeedReason::Power,
        requested: 2,
        completed: 0,
    };

    assert!(
        store
            .raise_alert(&alert(1, ID, feed, 100))
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        store.raise_alert(&alert(2, ID, feed, 200)).await.unwrap(),
        None
    );
    store
        .raise_alert(&alert(3, ID, AlertCondition::HopperEmpty, 100))
        .await
        .unwrap();

    let standing = store.standing_alerts(ID).await.unwrap();
    let kinds: Vec<&str> = standing.iter().map(|a| a.condition.code()).collect();
    assert_eq!(kinds, ["hopper_empty"], "a feed's alert never clears");
}

#[tokio::test]
async fn an_alert_is_acknowledged_once_and_listed_by_state() {
    let (_dir, store) = fresh().await;
    let ada = UserId(Uuid::from_u128(9));
    let bob = UserId(Uuid::from_u128(10));
    let open = store
        .raise_alert(&alert(1, ID, AlertCondition::ClockInvalid, 100))
        .await
        .unwrap()
        .unwrap();
    let other = store
        .raise_alert(&alert(2, OTHER, AlertCondition::HopperEmpty, 150))
        .await
        .unwrap()
        .unwrap();

    let Acked::Now(acked) = store
        .acknowledge_alert(open.id, ada, at(200))
        .await
        .unwrap()
    else {
        panic!("a first acknowledgement");
    };
    assert_eq!(acked.state, AlertState::Acknowledged);
    assert_eq!(
        acked.acknowledged.map(|a| (a.by, a.at)),
        Some((ada, at(200)))
    );
    let before = store
        .acknowledge_alert(open.id, bob, at(300))
        .await
        .unwrap();
    assert_eq!(before, Acked::Before(acked), "the first one stays");
    let missing = AlertId(Uuid::from_u128(99));
    let unknown = store.acknowledge_alert(missing, ada, at(300)).await;
    assert_eq!(unknown.unwrap(), Acked::NotFound);

    let ids = |rows: Vec<(u64, wellfed_api::Alert)>| -> Vec<AlertId> {
        rows.into_iter().map(|(_, alert)| alert.id).collect()
    };
    let all = store.alerts(filter(None, None)).await.unwrap();
    assert_eq!(ids(all.clone()), [open.id, other.id]);
    let acknowledged = filter(Some(AlertState::Acknowledged), None);
    assert_eq!(ids(store.alerts(acknowledged).await.unwrap()), [open.id]);
    let opened = filter(Some(AlertState::Open), None);
    assert_eq!(ids(store.alerts(opened).await.unwrap()), [other.id]);
    let mine = filter(None, Some(OTHER));
    assert_eq!(ids(store.alerts(mine).await.unwrap()), [other.id]);
    let after = AlertFilter {
        after: Some(all[0].0),
        ..filter(None, None)
    };
    assert_eq!(ids(store.alerts(after).await.unwrap()), [other.id]);

    store.clear_alert(open.id, at(400)).await.unwrap();
    let cleared = filter(Some(AlertState::Cleared), None);
    let rows = store.alerts(cleared).await.unwrap();
    assert_eq!(ids(rows.clone()), [open.id]);
    assert!(rows[0].1.acknowledged.is_some(), "kept once cleared");
}

#[tokio::test]
async fn the_rules_read_the_newest_boot_sync_food_read_and_blocked_spell() {
    let (_dir, store) = fresh().await;
    assert_eq!(store.last_boot(ID).await.unwrap(), None);
    assert!(store.blocked_since_free(ID).await.unwrap().is_empty());

    let blocked = |reason| EventBody::FeedingBlocked { reason };
    let sync = EventBody::ClockSync {
        by: Source::Feeder,
        jump_ms: None,
    };
    let boot = |bank| EventBody::Boot {
        bank,
        reset_flags: 0,
    };
    event(&store, 1, 1_000, false, boot(0)).await;
    event(&store, 2, 2_000, false, blocked(Some(Blocked::Starting))).await;
    event(&store, 3, 3_000, true, sync).await;
    event(&store, 4, 4_000, true, blocked(None)).await;
    event(&store, 5, 5_000, true, blocked(Some(Blocked::Battery))).await;
    event(&store, 6, 6_000, true, blocked(Some(Blocked::Fault))).await;
    event(&store, 7, 7_000, false, boot(1)).await;

    let last = store.last_boot(ID).await.unwrap().unwrap();
    assert_eq!((last.seq, last.value.bank, last.at), (7, 1, None));
    let synced = store.last_clock_sync(ID).await.unwrap().unwrap();
    assert_eq!(synced.seq, 3);
    let spell = store.blocked_since_free(ID).await.unwrap();
    let reasons: Vec<Option<Blocked>> = spell.iter().map(|row| row.value).collect();
    assert_eq!(reasons, [Some(Blocked::Battery), Some(Blocked::Fault)]);
    event(&store, 8, 8_000, true, blocked(None)).await;
    assert!(store.blocked_since_free(ID).await.unwrap().is_empty());

    let food = |present| {
        Stats::Food(FoodStats {
            present,
            emptied: 0,
            refilled: 0,
            feeds_since_refill: None,
        })
    };
    assert_eq!(store.last_food_reading(ID).await.unwrap(), None);
    store
        .record_stats(ID, at(10), Some(1), &food(Some(true)))
        .await
        .unwrap();
    store
        .record_stats(ID, at(20), Some(1), &food(Some(false)))
        .await
        .unwrap();
    let newest = store.last_food_reading(ID).await.unwrap().unwrap();
    assert_eq!(
        (newest.received, newest.food.present),
        (at(20), Some(false))
    );
}

#[tokio::test]
async fn a_pets_links_are_read_over_a_window() {
    let (_dir, store) = fresh().await;
    let pet = PetId(Uuid::from_u128(5));
    let created = store
        .insert_pet(
            &wellfed_api::Pet {
                id: pet,
                name: "Tom".into(),
                species: wellfed_api::Species::Cat,
                birth_date: None,
                daily_kcal: None,
                feeders: Vec::new(),
            },
            at(0),
        )
        .await;
    created.unwrap();
    store.assign(pet, ID, at(100)).await.unwrap();
    store.unassign(pet, ID, at(200)).await.unwrap();
    store.assign(pet, OTHER, at(150)).await.unwrap();

    let feeders = |rows: Vec<wellfed_api::PetLink>| -> Vec<FeederId> {
        rows.into_iter().map(|link| link.feeder).collect()
    };
    let all = store.pet_links(pet, at(0), at(1000)).await.unwrap();
    assert_eq!(feeders(all), [ID, OTHER]);
    let late = store.pet_links(pet, at(200), at(1000)).await.unwrap();
    assert_eq!(feeders(late), [OTHER]);
    let early = store.pet_links(pet, at(0), at(100)).await.unwrap();
    assert!(early.is_empty());
}
