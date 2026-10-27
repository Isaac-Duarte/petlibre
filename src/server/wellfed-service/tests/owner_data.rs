//! Owner data: pets and their weight logs, foods, which feeder serves
//! which pet and food over time, and refills.

use std::sync::Arc;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use wellfed_api::{
    FeederId, FoodId, NewFood, NewPet, NewRefill, NewWeight, PageQuery, PetId, PetLink, PetPatch,
    Species,
};
use wellfed_server_testkit::fixed::ManualClock;
use wellfed_server_testkit::{TestService, ada_setup, feeder, local};
use wellfed_service::{Error, Invalid, OwnerDataError, Resource, Service};
use uuid::Uuid;

/// A feeder this server never adopted.
const NOBODY: FeederId = FeederId(Uuid::from_u128(0x0192c5d0_0000_7000_8000_000000000000));

fn tom() -> NewPet {
    NewPet {
        name: " Tom ".into(),
        species: Species::Cat,
        birth_date: Some(jiff::civil::date(2020, 5, 1)),
        daily_kcal: Some(250),
    }
}

fn kibble(name: &str, bag_grams: u32) -> NewFood {
    NewFood {
        brand: "Acme".into(),
        name: name.into(),
        kcal_per_kg: 3600,
        bag_grams,
        grams_per_cup: wellfed_api::GRAMS_PER_CUP,
    }
}

fn page(after: Option<String>, limit: u32) -> PageQuery {
    PageQuery {
        after,
        limit: Some(limit),
    }
}

#[track_caller]
fn invalid<T: std::fmt::Debug>(result: Result<T, Error>, want: Invalid) {
    match result {
        Err(Error::Invalid(got)) => assert_eq!(got, want),
        other => panic!("expected {want:?}, got {other:?}"),
    }
}

#[track_caller]
fn not_found<T: std::fmt::Debug>(result: Result<T, Error>, want: Resource) {
    match result {
        Err(Error::NotFound(got)) => assert_eq!(got, want),
        other => panic!("expected {want:?} not found, got {other:?}"),
    }
}

#[track_caller]
fn refused<T: std::fmt::Debug>(result: Result<T, Error>, want: OwnerDataError) {
    match result {
        Err(Error::OwnerData(got)) => assert_eq!(got, want),
        other => panic!("expected {want:?}, got {other:?}"),
    }
}

/// The food the feeder served at `at`.
async fn food_at(service: &Service, feeder: FeederId, at: Timestamp) -> Option<FoodId> {
    let period = service.feeder_food_at(feeder, at).await.unwrap();
    period.map(|period| period.food.id)
}

#[test]
fn a_pet_is_made_changed_and_deleted_with_its_weight_log() {
    local(async {
        let server = TestService::start().await;
        let service = server.service();

        let pet = service.create_pet(tom()).await.unwrap();
        assert_eq!(pet.name, "Tom", "trimmed");
        assert!(pet.feeders.is_empty());
        assert_eq!(service.pet(pet.id).await.unwrap(), pet);
        let mut bad = tom();
        bad.name = "  ".into();
        invalid(service.create_pet(bad).await, Invalid::PetName);
        let mut bad = tom();
        bad.daily_kcal = Some(0);
        invalid(service.create_pet(bad).await, Invalid::DailyKcal);

        let patch = PetPatch {
            species: Some(Species::Dog),
            birth_date: Some(None),
            daily_kcal: Some(Some(900)),
            ..PetPatch::default()
        };
        let changed = service.update_pet(pet.id, patch).await.unwrap();
        assert_eq!(
            (changed.species, changed.birth_date, changed.daily_kcal),
            (Species::Dog, None, Some(900))
        );
        assert_eq!(changed.name, "Tom", "an absent field is kept");
        assert_eq!(service.pets().await.unwrap(), [changed]);

        // A vet's reading from last week sorts before today's.
        let now = service.weights(pet.id, PageQuery::default()).await.unwrap();
        assert!(now.items.is_empty() && now.next.is_none());
        let today = service
            .add_weight(
                pet.id,
                NewWeight {
                    grams: 4300,
                    at: None,
                },
            )
            .await
            .unwrap();
        let earlier = Timestamp::from_millisecond(wellfed_server_testkit::fixed::NOW_MS)
            .unwrap()
            .checked_sub(SignedDuration::from_hours(24 * 7))
            .unwrap();
        let vet = service
            .add_weight(
                pet.id,
                NewWeight {
                    grams: 4100,
                    at: Some(earlier),
                },
            )
            .await
            .unwrap();
        invalid(
            service
                .add_weight(pet.id, NewWeight { grams: 0, at: None })
                .await,
            Invalid::Weight,
        );

        let first = service.weights(pet.id, page(None, 1)).await.unwrap();
        assert_eq!(first.items, std::slice::from_ref(&vet));
        let rest = service.weights(pet.id, page(first.next, 1)).await.unwrap();
        assert_eq!((rest.items, rest.next), (vec![today.clone()], None));
        invalid(
            service.weights(pet.id, page(Some("nope".into()), 1)).await,
            Invalid::PageCursor,
        );
        invalid(
            service.weights(pet.id, page(None, 1001)).await,
            Invalid::PageLimit,
        );

        service.delete_weight(pet.id, today.id).await.unwrap();
        not_found(
            service.delete_weight(pet.id, today.id).await,
            Resource::Weight,
        );
        service.delete_pet(pet.id).await.unwrap();
        not_found(service.pet(pet.id).await, Resource::Pet);
        not_found(
            service.weights(pet.id, PageQuery::default()).await,
            Resource::Pet,
        );
        server.shutdown().await;
    });
}

#[test]
fn pets_and_feeders_are_many_to_many() {
    local(async {
        let (kitchen_sim, hall_sim) = (feeder::start().await, feeder::start().await);
        let server = TestService::start().await;
        let service = server.service();
        service.setup(ada_setup()).await.unwrap();
        let kitchen = server.adopt(&kitchen_sim).await;
        let hall = server.adopt(&hall_sim).await;

        let tom = service.create_pet(tom()).await.unwrap();
        let mut ada_cat = self::tom();
        ada_cat.name = "Ada".into();
        let ada_cat = service.create_pet(ada_cat).await.unwrap();

        service.assign_feeder(tom.id, kitchen).await.unwrap();
        let both = service.assign_feeder(tom.id, hall).await.unwrap();
        let mut want = vec![kitchen, hall];
        want.sort();
        assert_eq!(both.feeders, want, "a pet eats from several feeders");
        let again = service.assign_feeder(tom.id, hall).await.unwrap();
        assert_eq!(again.feeders, want, "assigning twice is a no-op");
        let shared = service.assign_feeder(ada_cat.id, kitchen).await.unwrap();
        assert_eq!(shared.feeders, [kitchen], "a feeder serves several pets");

        assert!(matches!(
            service.assign_feeder(tom.id, NOBODY).await,
            Err(Error::NotFound(Resource::Feeder))
        ));
        let left = service.unassign_feeder(tom.id, hall).await.unwrap();
        assert_eq!(left.feeders, [kitchen]);
        refused(
            service.unassign_feeder(tom.id, hall).await,
            OwnerDataError::NotAssigned,
        );
        let listed = service.pets().await.unwrap();
        let feeders: Vec<_> = listed
            .iter()
            .map(|p| (p.name.as_str(), p.feeders.clone()))
            .collect();
        assert_eq!(feeders, [("Ada", vec![kitchen]), ("Tom", vec![kitchen])]);
        server.shutdown().await;
    });
}

#[test]
fn a_food_is_checked_and_one_a_feeder_served_is_kept() {
    local(async {
        let sim = feeder::start().await;
        let server = TestService::start().await;
        let service = server.service();
        service.setup(ada_setup()).await.unwrap();
        let kitchen = server.adopt(&sim).await;

        let dry = service.create_food(kibble(" Indoor ", 2000)).await.unwrap();
        assert_eq!((dry.name.as_str(), dry.grams_per_cup), ("Indoor", 110));
        for (food, want) in [
            (kibble("", 2000), Invalid::FoodName),
            (kibble("x", 0), Invalid::BagSize),
            (
                NewFood {
                    kcal_per_kg: 10_001,
                    ..kibble("x", 2000)
                },
                Invalid::Kcal,
            ),
            (
                NewFood {
                    grams_per_cup: 0,
                    ..kibble("x", 2000)
                },
                Invalid::GramsPerCup,
            ),
        ] {
            invalid(service.create_food(food).await, want);
        }
        let patch = wellfed_api::FoodPatch {
            grams_per_cup: Some(95),
            ..wellfed_api::FoodPatch::default()
        };
        let dry = service.update_food(dry.id, patch).await.unwrap();
        assert_eq!(dry.grams_per_cup, 95);
        assert_eq!(service.foods().await.unwrap(), std::slice::from_ref(&dry));

        service.set_feeder_food(kitchen, dry.id).await.unwrap();
        refused(service.delete_food(dry.id).await, OwnerDataError::FoodInUse);
        let spare = service.create_food(kibble("Spare", 400)).await.unwrap();
        service.delete_food(spare.id).await.unwrap();
        not_found(service.food(spare.id).await, Resource::Food);
        not_found(
            service.set_feeder_food(kitchen, spare.id).await,
            Resource::Food,
        );
        assert!(matches!(
            service.set_feeder_food(NOBODY, dry.id).await,
            Err(Error::NotFound(Resource::Feeder))
        ));
        server.shutdown().await;
    });
}

#[test]
fn the_food_at_a_time_is_the_one_the_feeder_served_then() {
    local(async {
        let sim = feeder::start().await;
        let clock = ManualClock::new();
        let server = TestService::start_with_clock(Arc::new(clock.clone())).await;
        let service = server.service();
        service.setup(ada_setup()).await.unwrap();
        let kitchen = server.adopt(&sim).await;
        let dry = service.create_food(kibble("Dry", 2000)).await.unwrap();
        let wet = service.create_food(kibble("Wet", 400)).await.unwrap();

        let none = service.feeder_food(kitchen).await.unwrap();
        assert_eq!((none.current, none.last_refill), (None, None));

        let first = service.set_feeder_food(kitchen, dry.id).await.unwrap();
        let t0 = first.from;
        clock.advance(Duration::from_secs(3600));
        let second = service.set_feeder_food(kitchen, wet.id).await.unwrap();
        let t1 = second.from;
        assert_eq!(t1, t0 + SignedDuration::from_hours(1));

        let ms = SignedDuration::from_millis(1);
        assert_eq!(food_at(service, kitchen, t0 - ms).await, None);
        assert_eq!(food_at(service, kitchen, t0).await, Some(dry.id));
        assert_eq!(food_at(service, kitchen, t1 - ms).await, Some(dry.id));
        assert_eq!(food_at(service, kitchen, t1).await, Some(wet.id));
        let current = service.feeder_food(kitchen).await.unwrap().current.unwrap();
        assert_eq!(
            (current.food.id, current.from, current.to),
            (wet.id, t1, None)
        );

        // The same food again keeps its period; the history pages.
        clock.advance(Duration::from_secs(60));
        let same = service.set_feeder_food(kitchen, wet.id).await.unwrap();
        assert_eq!(same.from, t1);
        let page1 = service.food_history(kitchen, page(None, 1)).await.unwrap();
        let only = &page1.items[0];
        assert_eq!((only.food.id, only.from, only.to), (dry.id, t0, Some(t1)));
        let page2 = service
            .food_history(kitchen, page(page1.next, 1))
            .await
            .unwrap();
        assert_eq!(page2.items, [current]);
        assert_eq!(page2.next, None);
        server.shutdown().await;
    });
}

#[test]
fn a_refill_is_grams_or_a_full_bag_of_the_current_food() {
    local(async {
        let sim = feeder::start().await;
        let server = TestService::start().await;
        let service = server.service();
        let (owner, _) = service.setup(ada_setup()).await.unwrap();
        let kitchen = server.adopt(&sim).await;

        refused(
            service
                .add_refill(&owner, kitchen, NewRefill::FullBag)
                .await,
            OwnerDataError::NoCurrentFood,
        );
        invalid(
            service
                .add_refill(&owner, kitchen, NewRefill::Grams { grams: 0 })
                .await,
            Invalid::RefillGrams,
        );
        let weighed = service
            .add_refill(&owner, kitchen, NewRefill::Grams { grams: 500 })
            .await
            .unwrap();
        assert_eq!(
            (weighed.grams, weighed.full_bag, weighed.food, weighed.user),
            (500, false, None, owner.id)
        );

        let dry = service.create_food(kibble("Dry", 2000)).await.unwrap();
        service.set_feeder_food(kitchen, dry.id).await.unwrap();
        let bag = service
            .add_refill(&owner, kitchen, NewRefill::FullBag)
            .await
            .unwrap();
        assert_eq!(
            (bag.grams, bag.full_bag, bag.food),
            (2000, true, Some(dry.id))
        );
        assert_eq!(
            service.feeder_food(kitchen).await.unwrap().last_refill,
            Some(bag.clone())
        );

        let first = service.refills(kitchen, page(None, 1)).await.unwrap();
        assert_eq!(first.items, std::slice::from_ref(&weighed));
        let rest = service.refills(kitchen, page(first.next, 1)).await.unwrap();
        assert_eq!((rest.items, rest.next), (vec![bag.clone()], None));

        service.delete_refill(kitchen, bag.id).await.unwrap();
        not_found(
            service.delete_refill(kitchen, bag.id).await,
            Resource::Refill,
        );
        assert_eq!(
            service.feeder_food(kitchen).await.unwrap().last_refill,
            Some(weighed)
        );
        assert!(matches!(
            service.refills(NOBODY, PageQuery::default()).await,
            Err(Error::NotFound(Resource::Feeder))
        ));
        server.shutdown().await;
    });
}

#[test]
fn the_current_food_and_refills_survive_the_clock_stepping_back() {
    local(async {
        let sim = feeder::start().await;
        let clock = ManualClock::new();
        let server = TestService::start_with_clock(Arc::new(clock.clone())).await;
        let service = server.service();
        let (owner, _) = service.setup(ada_setup()).await.unwrap();
        let kitchen = server.adopt(&sim).await;
        let dry = service.create_food(kibble("Dry", 2000)).await.unwrap();
        let wet = service.create_food(kibble("Wet", 400)).await.unwrap();

        service.set_feeder_food(kitchen, dry.id).await.unwrap();
        clock.advance(Duration::from_secs(60));
        let wet_from = service.set_feeder_food(kitchen, wet.id).await.unwrap().from;
        let first = service
            .add_refill(&owner, kitchen, NewRefill::Grams { grams: 300 })
            .await
            .unwrap();

        // A correction puts the clock before the change and the refill:
        // "now" is still the newest change.
        clock.step_back(Duration::from_secs(3600));
        let current = service.feeder_food(kitchen).await.unwrap().current.unwrap();
        assert_eq!(
            (current.food.id, current.from, current.to),
            (wet.id, wet_from, None)
        );
        let bag = service
            .add_refill(&owner, kitchen, NewRefill::FullBag)
            .await
            .unwrap();
        assert_eq!((bag.grams, bag.food), (400, Some(wet.id)), "the wet bag");
        assert_eq!(bag.at, first.at, "never before the last refill");

        let listed = service
            .refills(kitchen, PageQuery::default())
            .await
            .unwrap();
        assert_eq!(listed.items, [first, bag]);
        server.shutdown().await;
    });
}

#[test]
fn a_weight_is_not_from_the_future_and_its_time_returns_as_stored() {
    local(async {
        let server = TestService::start().await;
        let service = server.service();
        let pet = service.create_pet(tom()).await.unwrap();
        let now = Timestamp::from_millisecond(wellfed_server_testkit::fixed::NOW_MS).unwrap();

        let later = now + SignedDuration::from_millis(1);
        let future = NewWeight {
            grams: 4000,
            at: Some(later),
        };
        invalid(
            service.add_weight(pet.id, future).await,
            Invalid::WeightTime,
        );

        // A reading with microseconds comes back as the store keeps it, so
        // it matches the log and its cursor.
        let fine = now - SignedDuration::from_micros(1500);
        let reading = NewWeight {
            grams: 4000,
            at: Some(fine),
        };
        let weight = service.add_weight(pet.id, reading).await.unwrap();
        assert_eq!(
            weight.at,
            Timestamp::from_millisecond(fine.as_millisecond()).unwrap()
        );
        let log = service.weights(pet.id, PageQuery::default()).await.unwrap();
        assert_eq!(log.items, [weight]);
        server.shutdown().await;
    });
}

#[test]
fn a_feeder_s_pets_keep_their_history() {
    local(async {
        let sim = feeder::start().await;
        let clock = ManualClock::new();
        let server = TestService::start_with_clock(Arc::new(clock.clone())).await;
        let service = server.service();
        service.setup(ada_setup()).await.unwrap();
        let kitchen = server.adopt(&sim).await;
        let tom = service.create_pet(tom()).await.unwrap();
        let mut ada = self::tom();
        ada.name = "Ada".into();
        let ada = service.create_pet(ada).await.unwrap();
        let t0 = Timestamp::from_millisecond(wellfed_server_testkit::fixed::NOW_MS).unwrap();
        let hour = SignedDuration::from_hours(1);
        let pets = |links: Vec<PetLink>| -> Vec<PetId> {
            links.into_iter().map(|link| link.pet).collect()
        };

        service.assign_feeder(tom.id, kitchen).await.unwrap();
        clock.advance(Duration::from_secs(3600));
        service.assign_feeder(ada.id, kitchen).await.unwrap();
        clock.advance(Duration::from_secs(3600));
        let left = service.unassign_feeder(tom.id, kitchen).await.unwrap();
        assert!(left.feeders.is_empty(), "the pet no longer lists it");

        let first = service.feeder_pets(kitchen, t0, t0 + hour).await.unwrap();
        assert_eq!(pets(first), [tom.id]);
        let second = service
            .feeder_pets(kitchen, t0 + hour, t0 + hour * 2)
            .await
            .unwrap();
        assert_eq!(
            pets(second),
            [tom.id, ada.id],
            "both shared the second hour"
        );
        let third = service
            .feeder_pets(kitchen, t0 + hour * 2, t0 + hour * 3)
            .await
            .unwrap();
        assert_eq!(pets(third), [ada.id], "Tom's link ended at the second hour");

        // Assigning again starts a new link, and a clock that stepped back
        // does not let it overlap the old one.
        clock.step_back(Duration::from_secs(7200));
        let back = service.assign_feeder(tom.id, kitchen).await.unwrap();
        assert_eq!(back.feeders, [kitchen]);
        let all = service
            .feeder_pets(kitchen, t0, t0 + hour * 3)
            .await
            .unwrap();
        let toms: Vec<_> = all
            .iter()
            .filter(|link| link.pet == tom.id)
            .map(|link| (link.from, link.to))
            .collect();
        assert_eq!(toms, [(t0, Some(t0 + hour * 2)), (t0 + hour * 2, None)]);
        assert!(matches!(
            service.feeder_pets(NOBODY, t0, t0 + hour).await,
            Err(Error::NotFound(Resource::Feeder))
        ));
        server.shutdown().await;
    });
}
