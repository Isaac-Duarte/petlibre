//! Pets, foods and refills over HTTP: everyone reads them, only an owner
//! writes them, and each refusal has its code.

use wellfed_api::{
    AdoptFeeder, ClientRole, ErrorCode, FeederId, FoodPatch, NewFood, NewPet, NewRefill, NewUser,
    NewWeight, PageQuery, PetPatch, RefillId, Role, Species, WeightId,
};
use wellfed_server_client::{Client, StatusCode};
use wellfed_server_testkit::{TestServer, ada_setup, feeder, local, refused, refused_for};
use uuid::Uuid;

/// A feeder no server has adopted: a permission is checked before the id.
const NOBODY: FeederId = FeederId(Uuid::from_u128(0x0192c5d0_0000_7000_8000_000000000000));

const PASSWORD: &str = "temporary pass";

async fn owner(server: &TestServer) -> Client {
    let mut client = server.client();
    client.setup(&ada_setup()).await.unwrap();
    client
}

/// A new account's client, logged in.
async fn account(server: &TestServer, owner: &Client, name: &str, role: Role) -> Client {
    let new = NewUser {
        name: name.into(),
        password: PASSWORD.into(),
        role,
        expires: None,
    };
    owner.create_user(&new).await.unwrap();
    let mut client = server.client();
    client
        .login(&wellfed_api::Credentials {
            name: name.into(),
            password: PASSWORD.into(),
        })
        .await
        .unwrap();
    client
}

fn tom() -> NewPet {
    NewPet {
        name: "Tom".into(),
        species: Species::Cat,
        birth_date: None,
        daily_kcal: Some(250),
    }
}

fn kibble(name: &str) -> NewFood {
    NewFood {
        brand: "Acme".into(),
        name: name.into(),
        kcal_per_kg: 3600,
        bag_grams: 2000,
        grams_per_cup: 110,
    }
}

#[tokio::test]
async fn a_member_and_a_viewer_read_owner_data_but_every_write_is_refused() {
    let server = TestServer::start().await;
    let owner = owner(&server).await;
    let pet = owner.create_pet(&tom()).await.unwrap();
    let food = owner.create_food(&kibble("Dry")).await.unwrap();
    let weight = owner
        .add_weight(
            pet.id,
            &NewWeight {
                grams: 4200,
                at: None,
            },
        )
        .await
        .unwrap();
    let member = account(&server, &owner, "bob", Role::Member).await;
    let viewer = account(&server, &owner, "vic", Role::Viewer).await;

    for (who, client) in [("member", &member), ("viewer", &viewer)] {
        assert_eq!(
            client.pets().await.unwrap(),
            std::slice::from_ref(&pet),
            "{who}"
        );
        assert_eq!(client.pet(pet.id).await.unwrap(), pet, "{who}");
        assert_eq!(
            client.foods().await.unwrap(),
            std::slice::from_ref(&food),
            "{who}"
        );
        assert_eq!(client.food(food.id).await.unwrap(), food, "{who}");
        let weights = client.weights(pet.id, &PageQuery::default()).await.unwrap();
        assert_eq!(weights.items, std::slice::from_ref(&weight), "{who}");
        // Allowed through to the feeder, which this server does not have.
        let gone = (StatusCode::NOT_FOUND, ErrorCode::NotFound);
        refused(client.feeder_food(NOBODY).await, gone.0, gone.1);
        let all = PageQuery::default();
        refused(client.food_history(NOBODY, &all).await, gone.0, gone.1);
        refused(client.refills(NOBODY, &all).await, gone.0, gone.1);

        let (status, code) = (StatusCode::FORBIDDEN, ErrorCode::Forbidden);
        let new_weight = NewWeight { grams: 1, at: None };
        let grams = NewRefill::Grams { grams: 10 };
        let refill = RefillId(Uuid::from_u128(1));
        let writes = [
            ("create pet", client.create_pet(&tom()).await.map(drop)),
            (
                "update pet",
                client
                    .update_pet(pet.id, &PetPatch::default())
                    .await
                    .map(drop),
            ),
            ("delete pet", client.delete_pet(pet.id).await),
            (
                "assign",
                client.assign_feeder(pet.id, NOBODY).await.map(drop),
            ),
            (
                "unassign",
                client.unassign_feeder(pet.id, NOBODY).await.map(drop),
            ),
            (
                "add weight",
                client.add_weight(pet.id, &new_weight).await.map(drop),
            ),
            (
                "delete weight",
                client.delete_weight(pet.id, weight.id).await,
            ),
            (
                "create food",
                client.create_food(&kibble("x")).await.map(drop),
            ),
            (
                "update food",
                client
                    .update_food(food.id, &FoodPatch::default())
                    .await
                    .map(drop),
            ),
            ("delete food", client.delete_food(food.id).await),
            (
                "set food",
                client.set_feeder_food(NOBODY, food.id).await.map(drop),
            ),
            ("refill", client.add_refill(NOBODY, &grams).await.map(drop)),
            ("delete refill", client.delete_refill(NOBODY, refill).await),
        ];
        for (write, result) in writes {
            refused_for(&format!("{who}: {write}"), result, status, code);
        }
    }

    assert_eq!(owner.pets().await.unwrap(), [pet], "nothing changed");
    server.stop().await;
}

#[test]
fn an_owner_keeps_pets_foods_and_refills_over_http() {
    local(async {
        let sim = feeder::start().await;
        let server = TestServer::start().await;
        let owner = owner(&server).await;
        let kitchen = owner
            .adopt(&AdoptFeeder {
                invite: feeder::invite(&sim, ClientRole::Server).await,
                name: "kitchen".into(),
                address: Some(sim.address.to_string()),
            })
            .await
            .unwrap()
            .id;

        // Pets, their weights and feeders.
        let pet = owner.create_pet(&tom()).await.unwrap();
        let assigned = owner.assign_feeder(pet.id, kitchen).await.unwrap();
        assert_eq!(assigned.feeders, [kitchen]);
        let patch = PetPatch {
            daily_kcal: Some(None),
            ..PetPatch::default()
        };
        let changed = owner.update_pet(pet.id, &patch).await.unwrap();
        assert_eq!((changed.daily_kcal, changed.feeders), (None, vec![kitchen]));
        let (bad, invalid) = (StatusCode::BAD_REQUEST, ErrorCode::Invalid);
        let mut nameless = tom();
        nameless.name.clear();
        refused(owner.create_pet(&nameless).await, bad, invalid);
        let none = (StatusCode::NOT_FOUND, ErrorCode::NotFound);
        refused(
            owner
                .delete_weight(pet.id, WeightId(Uuid::from_u128(7)))
                .await,
            none.0,
            none.1,
        );
        refused(owner.assign_feeder(pet.id, NOBODY).await, none.0, none.1);

        // A food, the feeder's history, and refills.
        let conflict = StatusCode::CONFLICT;
        refused(
            owner.add_refill(kitchen, &NewRefill::FullBag).await,
            conflict,
            ErrorCode::NoCurrentFood,
        );
        let dry = owner.create_food(&kibble("Dry")).await.unwrap();
        let period = owner.set_feeder_food(kitchen, dry.id).await.unwrap();
        assert_eq!((period.food.clone(), period.to), (dry.clone(), None));
        refused(
            owner.delete_food(dry.id).await,
            conflict,
            ErrorCode::FoodInUse,
        );
        let bag = owner
            .add_refill(kitchen, &NewRefill::FullBag)
            .await
            .unwrap();
        assert_eq!(
            (bag.grams, bag.full_bag, bag.food),
            (2000, true, Some(dry.id))
        );

        let food = owner.feeder_food(kitchen).await.unwrap();
        assert_eq!(food.current, Some(period.clone()));
        assert_eq!(food.last_refill, Some(bag.clone()));
        let history = owner
            .food_history(kitchen, &PageQuery::default())
            .await
            .unwrap();
        assert_eq!((history.items, history.next), (vec![period], None));
        let refills = owner.refills(kitchen, &PageQuery::default()).await.unwrap();
        assert_eq!(refills.items, std::slice::from_ref(&bag));
        let bad_page = PageQuery {
            after: Some("x".into()),
            limit: None,
        };
        refused(owner.refills(kitchen, &bad_page).await, bad, invalid);

        owner.delete_refill(kitchen, bag.id).await.unwrap();
        owner.delete_pet(pet.id).await.unwrap();
        refused(owner.pet(pet.id).await, none.0, none.1);
        server.stop().await;
    });
}
