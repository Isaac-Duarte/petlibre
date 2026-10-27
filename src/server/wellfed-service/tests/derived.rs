//! The derived views over what the server holds, against the simulator's
//! feeding owner: a feed it ran counts in the food left since the refill
//! and in each pet's intake, split between the pets the feeder serves.

use wellfed_api::{
    CommandOutcome, CommandState, FeedOutcome, IntakeQuery, NewFood, NewPet, NewRefill, Species,
};
use wellfed_server_testkit::feeder::{self, Flash};
use wellfed_server_testkit::{TestService, ada, local, until};
use wellfed_service::{Error, Invalid, OwnerDataError};

fn cat(name: &str) -> NewPet {
    NewPet {
        name: name.into(),
        species: Species::Cat,
        birth_date: None,
        daily_kcal: Some(200),
    }
}

#[test]
fn a_feed_counts_in_the_food_left_and_in_each_pets_share() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let server = TestService::start().await;
        let id = server.adopt(&sim).await;
        server.link(id, wellfed_api::Connection::Online).await;
        let service = server.service();

        // 120 g a cup: 10 g a portion, 36 kcal at 3600 kcal per kg.
        let food = service
            .create_food(NewFood {
                brand: "Acme".into(),
                name: "Dry".into(),
                kcal_per_kg: 3600,
                bag_grams: 2000,
                grams_per_cup: 120,
            })
            .await
            .unwrap();
        service.set_feeder_food(id, food.id).await.unwrap();
        let user = ada();
        service
            .add_refill(&user, id, NewRefill::FullBag)
            .await
            .unwrap();
        let tom = service.create_pet(cat("Tom")).await.unwrap();
        let ana = service.create_pet(cat("Ana")).await.unwrap();
        service.assign_feeder(tom.id, id).await.unwrap();
        service.assign_feeder(ana.id, id).await.unwrap();

        let key = [0xe1; 16];
        service
            .feed(&user.clone().into(), id, 4, key)
            .await
            .unwrap();
        until("the feed is settled from the history", || async {
            let entry = service.command(&ada(), key).await.unwrap();
            entry.state == CommandState::Resolved
        })
        .await;
        let entry = service.command(&ada(), key).await.unwrap();
        let Some(CommandOutcome::Fed {
            outcome, completed, ..
        }) = entry.outcome
        else {
            panic!("a fed outcome: {entry:?}");
        };
        assert_eq!((outcome, completed), (FeedOutcome::Done, 4));

        let view = service.feeder_food(id).await.unwrap();
        let estimate = view.estimate.expect("a refill is recorded");
        assert_eq!(
            (
                estimate.portions,
                estimate.dispensed_grams,
                estimate.remaining_grams
            ),
            (4, 40, 1960)
        );
        assert_eq!(estimate.portion_mg, Some(10_000));
        assert_eq!(view.unrecorded_refill, None);

        let today = service.pet_intake(tom.id, IntakeQuery::default()).await;
        let today = today.unwrap();
        assert_eq!(today.days.len(), 7);
        let day = today.days.last().unwrap();
        assert_eq!((day.grams, day.kcal, day.estimate), (20, 72, true));
        assert_eq!(day.target_percent, Some(36));
        assert!(today.anomalies.is_empty());
        let other = service.pet_intake(ana.id, IntakeQuery::default()).await;
        assert_eq!(other.unwrap().days.last().unwrap().grams, 20);

        let backwards = IntakeQuery {
            from: Some(jiff::civil::date(2026, 10, 2)),
            to: Some(jiff::civil::date(2026, 10, 1)),
        };
        let refused = service.pet_intake(tom.id, backwards).await;
        assert!(matches!(refused, Err(Error::Invalid(Invalid::IntakeDays))));
        let long = IntakeQuery {
            from: Some(jiff::civil::date(2024, 12, 31)),
            to: Some(jiff::civil::date(2026, 1, 1)),
        };
        let refused = service.pet_intake(tom.id, long).await;
        assert!(matches!(refused, Err(Error::Invalid(Invalid::IntakeDays))));
        let unknown = wellfed_api::PetId(uuid::Uuid::from_u128(1));
        let refused = service.pet_intake(unknown, IntakeQuery::default()).await;
        assert!(matches!(
            refused,
            Err(Error::OwnerData(OwnerDataError::PetNotFound))
        ));

        server.shutdown().await;
        sim.stop().await;
    });
}
