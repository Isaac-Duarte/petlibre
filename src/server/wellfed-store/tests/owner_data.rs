//! Pets, foods, each feeder's food over time and refills, against real
//! SQLite.

use jiff::Timestamp;
use wellfed_api::{
    ClientRole, FeederId, Food, FoodId, Pet, PetId, PetLink, Refill, RefillId, Species, UserId,
    Weight, WeightId,
};
use wellfed_store::{Error, FoodSet, NewFeeder, SealedKey, Store, WeightCursor};
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

fn food(n: u128, bag_grams: u32) -> Food {
    Food {
        id: FoodId(Uuid::from_u128(0x100 + n)),
        brand: "Acme".into(),
        name: format!("recipe {n}"),
        kcal_per_kg: 3600,
        bag_grams,
        grams_per_cup: 110,
    }
}

fn pet(n: u128, name: &str) -> Pet {
    Pet {
        id: PetId(Uuid::from_u128(0x200 + n)),
        name: name.into(),
        species: Species::Cat,
        birth_date: Some(jiff::civil::date(2020, 5, 1)),
        daily_kcal: Some(250),
        feeders: Vec::new(),
    }
}

/// Which food the feeder served at `ms`.
async fn food_at(store: &Store, feeder: FeederId, ms: i64) -> Option<FoodId> {
    let period = store.feeder_food_at(feeder, at(ms)).await.unwrap();
    period.map(|period| period.food.id)
}

#[tokio::test]
async fn a_pet_round_trips_with_its_feeders_and_its_deletion_takes_its_log() {
    let (_dir, store, feeder) = fresh().await;
    let (tom, ada) = (pet(1, "Tom"), pet(2, "Ada"));
    store.insert_pet(&tom, at(5)).await.unwrap();
    store.insert_pet(&ada, at(5)).await.unwrap();

    store.assign(tom.id, feeder, at(10)).await.unwrap();
    store.assign(tom.id, feeder, at(20)).await.unwrap();
    let held = store.pet(tom.id).await.unwrap().unwrap();
    assert_eq!(held.feeders, vec![feeder], "a second assignment is a no-op");
    assert_eq!(held.birth_date, tom.birth_date);
    let names: Vec<_> = store
        .pets()
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(names, ["Ada", "Tom"], "by name");

    let weight = Weight {
        id: WeightId(Uuid::from_u128(9)),
        at: at(10),
        grams: 4200,
    };
    store.insert_weight(tom.id, &weight).await.unwrap();
    assert!(store.delete_pet(tom.id).await.unwrap());
    assert!(store.pet(tom.id).await.unwrap().is_none());
    assert!(store.weights(tom.id, None, 10).await.unwrap().is_empty());
    assert!(
        !store.unassign(tom.id, feeder, at(30)).await.unwrap(),
        "the link went with it"
    );
}

#[tokio::test]
async fn a_weight_log_pages_by_reading_time_even_when_backdated() {
    let (_dir, store, _) = fresh().await;
    let tom = pet(1, "Tom");
    store.insert_pet(&tom, at(5)).await.unwrap();

    // Recorded in this order, read at these times: the vet's is older.
    for (id, ms) in [(1, 300), (2, 100), (3, 200), (4, 200)] {
        let weight = Weight {
            id: WeightId(Uuid::from_u128(id)),
            at: at(ms),
            grams: 4000,
        };
        store.insert_weight(tom.id, &weight).await.unwrap();
    }

    let first = store.weights(tom.id, None, 2).await.unwrap();
    let ids: Vec<_> = first.iter().map(|w| w.id.0.as_u128()).collect();
    assert_eq!(ids, [2, 3]);
    let last = first.last().unwrap();
    let after = WeightCursor {
        at: last.at,
        id: last.id,
    };
    let rest = store.weights(tom.id, Some(after), 10).await.unwrap();
    let ids: Vec<_> = rest.iter().map(|w| w.id.0.as_u128()).collect();
    assert_eq!(ids, [4, 1], "a tie at 200 ms breaks by id");
}

#[tokio::test]
async fn the_food_at_a_time_is_the_last_change_made_by_then() {
    let (_dir, store, feeder) = fresh().await;
    let (dry, wet) = (food(1, 2000), food(2, 400));
    store.insert_food(&dry, at(1)).await.unwrap();
    store.insert_food(&wet, at(1)).await.unwrap();

    assert_eq!(food_at(&store, feeder, 1000).await, None, "none set yet");
    let set = store
        .set_feeder_food(feeder, dry.id, at(100))
        .await
        .unwrap();
    assert_eq!(set, FoodSet::Started(at(100)));
    let set = store
        .set_feeder_food(feeder, wet.id, at(200))
        .await
        .unwrap();
    assert_eq!(set, FoodSet::Started(at(200)));

    assert_eq!(food_at(&store, feeder, 99).await, None, "before the first");
    assert_eq!(
        food_at(&store, feeder, 100).await,
        Some(dry.id),
        "from its start"
    );
    assert_eq!(food_at(&store, feeder, 199).await, Some(dry.id));
    assert_eq!(
        food_at(&store, feeder, 200).await,
        Some(wet.id),
        "the change's own instant is the new food's"
    );
    assert_eq!(food_at(&store, feeder, 10_000).await, Some(wet.id));

    let period = store
        .feeder_food_at(feeder, at(150))
        .await
        .unwrap()
        .unwrap();
    assert_eq!((period.from, period.to), (at(100), Some(at(200))));
    let current = store
        .feeder_food_at(feeder, at(250))
        .await
        .unwrap()
        .unwrap();
    assert_eq!((current.from, current.to), (at(200), None));
}

#[tokio::test]
async fn a_change_is_never_dated_before_the_one_it_follows() {
    let (_dir, store, feeder) = fresh().await;
    let (dry, wet) = (food(1, 2000), food(2, 400));
    store.insert_food(&dry, at(1)).await.unwrap();
    store.insert_food(&wet, at(1)).await.unwrap();

    store
        .set_feeder_food(feeder, dry.id, at(500))
        .await
        .unwrap();
    // The clock stepped back: the change starts with the last one, and
    // wins it.
    let set = store
        .set_feeder_food(feeder, wet.id, at(300))
        .await
        .unwrap();
    assert_eq!(set, FoodSet::Started(at(500)));
    assert_eq!(food_at(&store, feeder, 500).await, Some(wet.id));
    assert_eq!(food_at(&store, feeder, 400).await, None);

    // Choosing the food it serves keeps its period.
    let set = store
        .set_feeder_food(feeder, wet.id, at(900))
        .await
        .unwrap();
    assert_eq!(set, FoodSet::Unchanged(at(500)));

    let history = store.food_history(feeder, None, 10).await.unwrap();
    let periods: Vec<_> = history
        .iter()
        .map(|(_, p)| (p.food.id, p.from, p.to))
        .collect();
    assert_eq!(
        periods,
        [(dry.id, at(500), Some(at(500))), (wet.id, at(500), None)]
    );
    let (first, _) = history[0];
    let rest = store.food_history(feeder, Some(first), 10).await.unwrap();
    assert_eq!(rest.len(), 1, "a page after the first period");
}

#[tokio::test]
async fn a_food_a_feeder_served_cannot_be_deleted() {
    let (_dir, store, feeder) = fresh().await;
    let (dry, spare) = (food(1, 2000), food(2, 400));
    store.insert_food(&dry, at(1)).await.unwrap();
    store.insert_food(&spare, at(1)).await.unwrap();
    store
        .set_feeder_food(feeder, dry.id, at(100))
        .await
        .unwrap();

    assert!(matches!(
        store.delete_food(dry.id).await,
        Err(Error::InUse("food"))
    ));
    assert!(store.delete_food(spare.id).await.unwrap());
    assert!(!store.delete_food(spare.id).await.unwrap());
}

#[tokio::test]
async fn refills_list_in_order_and_the_newest_is_the_last() {
    let (_dir, store, feeder) = fresh().await;
    let dry = food(1, 2000);
    store.insert_food(&dry, at(1)).await.unwrap();
    let refill = |n: u128, grams| Refill {
        id: RefillId(Uuid::from_u128(0x300 + n)),
        feeder,
        at: at(100 * i64::try_from(n).unwrap()),
        grams,
        full_bag: false,
        food: Some(dry.id),
        user: UserId(Uuid::from_u128(5)),
    };

    assert_eq!(store.last_refill(feeder).await.unwrap(), None);
    for (n, grams) in [(1, 500), (2, 2000), (3, 250)] {
        store.insert_refill(&refill(n, grams)).await.unwrap();
    }

    let page = store.refills(feeder, None, 2).await.unwrap();
    let grams: Vec<_> = page.iter().map(|(_, r)| r.grams).collect();
    assert_eq!(grams, [500, 2000]);
    let rest = store.refills(feeder, Some(page[1].0), 10).await.unwrap();
    assert_eq!(rest.len(), 1);
    assert_eq!(
        store.last_refill(feeder).await.unwrap(),
        Some(refill(3, 250))
    );

    assert!(
        store
            .delete_refill(feeder, refill(3, 250).id)
            .await
            .unwrap()
    );
    assert_eq!(
        store.last_refill(feeder).await.unwrap(),
        Some(refill(2, 2000))
    );
    assert!(
        matches!(store.delete_food(dry.id).await, Err(Error::InUse("food"))),
        "a refill names it"
    );
}

#[tokio::test]
async fn the_current_food_is_the_newest_change_whatever_the_clock_reads() {
    let (_dir, store, feeder) = fresh().await;
    let (dry, wet) = (food(1, 2000), food(2, 400));
    store.insert_food(&dry, at(1)).await.unwrap();
    store.insert_food(&wet, at(1)).await.unwrap();
    assert_eq!(store.current_food(feeder).await.unwrap(), None);

    store
        .set_feeder_food(feeder, dry.id, at(100))
        .await
        .unwrap();
    store
        .set_feeder_food(feeder, wet.id, at(500))
        .await
        .unwrap();

    // A clock reading 300 asks the history for dry; "now" is wet.
    assert_eq!(food_at(&store, feeder, 300).await, Some(dry.id));
    let current = store.current_food(feeder).await.unwrap().unwrap();
    assert_eq!(
        (current.food.id, current.from, current.to),
        (wet.id, at(500), None)
    );
}

#[tokio::test]
async fn a_refill_is_never_dated_before_the_feeder_s_last() {
    let (_dir, store, feeder) = fresh().await;
    let refill = |n: u128, ms: i64| Refill {
        id: RefillId(Uuid::from_u128(0x300 + n)),
        feeder,
        at: at(ms),
        grams: 100,
        full_bag: false,
        food: None,
        user: UserId(Uuid::from_u128(5)),
    };

    assert_eq!(store.insert_refill(&refill(1, 500)).await.unwrap(), at(500));
    assert_eq!(
        store.insert_refill(&refill(2, 300)).await.unwrap(),
        at(500),
        "a clock that stepped back"
    );
    assert_eq!(store.insert_refill(&refill(3, 900)).await.unwrap(), at(900));

    let times: Vec<_> = store
        .refills(feeder, None, 10)
        .await
        .unwrap()
        .into_iter()
        .map(|(_, r)| r.at)
        .collect();
    assert_eq!(times, [at(500), at(500), at(900)]);
}

#[tokio::test]
async fn a_feeder_s_links_keep_their_history_and_never_overlap() {
    let (_dir, store, feeder) = fresh().await;
    let (tom, ada) = (pet(1, "Tom"), pet(2, "Ada"));
    store.insert_pet(&tom, at(5)).await.unwrap();
    store.insert_pet(&ada, at(5)).await.unwrap();
    let spans = |links: Vec<PetLink>| -> Vec<_> {
        links
            .into_iter()
            .map(|link| (link.pet, link.from, link.to))
            .collect()
    };

    store.assign(tom.id, feeder, at(100)).await.unwrap();
    store.assign(ada.id, feeder, at(200)).await.unwrap();
    assert!(store.unassign(tom.id, feeder, at(300)).await.unwrap());
    assert!(!store.unassign(tom.id, feeder, at(310)).await.unwrap());
    let all = store.feeder_links(feeder, at(0), at(1000)).await.unwrap();
    assert_eq!(
        spans(all),
        [(tom.id, at(100), Some(at(300))), (ada.id, at(200), None)]
    );
    let late = store.feeder_links(feeder, at(300), at(400)).await.unwrap();
    assert_eq!(spans(late), [(ada.id, at(200), None)], "`to` is exclusive");
    let early = store.feeder_links(feeder, at(0), at(100)).await.unwrap();
    assert!(early.is_empty(), "the window's end is exclusive");

    // The clock stepped back: the new link starts where the old one ended,
    // and an unassignment never ends a link before it began.
    store.assign(tom.id, feeder, at(250)).await.unwrap();
    assert!(store.unassign(tom.id, feeder, at(50)).await.unwrap());
    let toms: Vec<_> = spans(store.feeder_links(feeder, at(0), at(1000)).await.unwrap())
        .into_iter()
        .filter(|(pet, ..)| *pet == tom.id)
        .collect();
    assert_eq!(
        toms,
        [(tom.id, at(100), Some(at(300)))],
        "the instant-long link served no time"
    );
    let held = store.pet(tom.id).await.unwrap().unwrap();
    assert!(held.feeders.is_empty(), "only open links are listed");
}
