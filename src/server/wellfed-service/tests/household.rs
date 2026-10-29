//! The household, and its zone on every feeder: each keeps the zone's POSIX
//! rule in its time settings, set on connect and whenever the zone changes,
//! so its clock and plans follow DST with the server down.

use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use wellfed_api::{
    ClientRole, Connection as Link, EventBody, FeederId, HouseholdPatch, NextMeal, PLANS, Plan,
    PlansChange, Source, TimeFormat, Units, Weekdays,
};
use wellfed_model::{Answer, Op, Pushed, TimeSync};
use wellfed_server_testkit::feeder::{self, Connection, Flash, Running, open_owner};
use wellfed_server_testkit::fixed::ManualClock;
use wellfed_server_testkit::{TestService, WAIT, ZONE, ada_setup, config, local, until};
use wellfed_service::{Caller, Error, Invalid, Resource, ZoneError};

const BERLIN: &str = "CET-1CEST,M3.5.0,M10.5.0/3";
const DENVER: &str = "MST7MDT,M3.2.0,M11.1.0";

/// The zone rule the feeder's time settings hold, as its owner reads them.
async fn feeder_zone(sim: &Running) -> Option<String> {
    let mut owner = open_owner(sim).await;
    let id = rand_id();
    let answer = owner.ask(id, &Op::GetTime).await.unwrap();
    let Answer::Time(view) = answer else {
        panic!("{answer:?}");
    };
    view.zone
}

/// The owner, changing the household.
fn owner() -> Caller {
    wellfed_server_testkit::ada().into()
}

/// A request id no other request of the owner's used, so a write's
/// remembered answer is never replayed to a read.
fn rand_id() -> [u8; 16] {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0x4000);
    u128::from(NEXT.fetch_add(1, Ordering::Relaxed)).to_le_bytes()
}

async fn holds(sim: &Running, rule: &str) {
    until(&format!("the feeder keeps {rule}"), || async {
        feeder_zone(sim).await.as_deref() == Some(rule)
    })
    .await;
}

/// The service set up in [`ZONE`], with `sim` adopted under an owner's key.
async fn adopted(server: &TestService, sim: &Running) -> FeederId {
    server.service().setup(ada_setup()).await.unwrap();
    let address = sim.address.to_string();
    let id = server.adopt_as(sim, &address, ClientRole::Owner).await;
    server.link(id, Link::Online).await;
    id
}

#[test]
fn setup_makes_the_household_and_an_owner_changes_it() {
    local(async {
        let server = TestService::start().await;
        let service = server.service();
        assert!(matches!(
            service.household().await,
            Err(Error::NotFound(Resource::Household))
        ));
        let state = service.setup_state().await.unwrap();
        assert_eq!((state.open, state.zone.as_deref()), (true, Some("UTC")));

        service.setup(ada_setup()).await.unwrap();
        let household = service.household().await.unwrap();
        assert_eq!(household.zone, ZONE);
        assert_eq!(
            (household.units, household.time_format),
            (Units::Grams, TimeFormat::H24)
        );

        let patch = HouseholdPatch {
            name: Some(" Lovelace ".into()),
            units: Some(Units::Ounces),
            time_format: Some(TimeFormat::H12),
            zone: Some("america/denver".into()),
        };
        let changed = service.update_household(&owner(), patch).await.unwrap();
        assert_eq!(changed.name, "Lovelace");
        assert_eq!(changed.zone, "America/Denver", "the canonical name");
        assert_eq!(service.household().await.unwrap(), changed);

        let unnamed = HouseholdPatch {
            name: Some(" ".into()),
            ..HouseholdPatch::default()
        };
        assert!(matches!(
            service.update_household(&owner(), unnamed).await,
            Err(Error::Invalid(Invalid::HouseholdName))
        ));
        server.shutdown().await;
    });
}

#[test]
fn an_unknown_zone_is_refused_by_name() {
    local(async {
        let server = TestService::start().await;
        let service = server.service();
        let mut setup = ada_setup();
        setup.zone = "Mars/Olympus".into();
        assert!(matches!(
            service.setup(setup).await,
            Err(Error::Zone(ZoneError::Unknown(name))) if name == "Mars/Olympus"
        ));
        assert!(service.setup_open().await.unwrap(), "nothing was made");
        service.setup(ada_setup()).await.unwrap();
        let unknown = HouseholdPatch {
            zone: Some("Mars/Olympus".into()),
            ..HouseholdPatch::default()
        };
        assert!(matches!(
            service.update_household(&owner(), unknown).await,
            Err(Error::Zone(ZoneError::Unknown(_)))
        ));
        assert_eq!(service.household().await.unwrap().zone, ZONE);
        // A zone that changes irregularly is refused the same way, by the
        // mirror's `Zone`; its tests find one in the bundled tzdb.
        server.shutdown().await;
    });
}

#[test]
fn every_feeder_keeps_the_households_zone_and_a_change_reaches_it() {
    local(async {
        let sim = feeder::start().await;
        let server = TestService::start().await;
        adopted(&server, &sim).await;
        holds(&sim, BERLIN).await;

        let denver = HouseholdPatch {
            zone: Some("America/Denver".into()),
            ..HouseholdPatch::default()
        };
        server
            .service()
            .update_household(&owner(), denver)
            .await
            .unwrap();
        holds(&sim, DENVER).await;

        server.shutdown().await;
        sim.stop().await;
    });
}

#[test]
fn a_feeder_offline_at_the_change_gets_the_zone_when_it_returns() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let address = sim.address;
        let server = TestService::start().await;
        let id = adopted(&server, &sim).await;
        holds(&sim, BERLIN).await;

        sim.stop().await;
        server.link(id, Link::Offline).await;
        let denver = HouseholdPatch {
            zone: Some("America/Denver".into()),
            ..HouseholdPatch::default()
        };
        server
            .service()
            .update_household(&owner(), denver)
            .await
            .unwrap();

        let sim = feeder::restart(&flash, address).await;
        server.link(id, Link::Online).await;
        holds(&sim, DENVER).await;

        server.shutdown().await;
        sim.stop().await;
    });
}

/// The periodic clock sync checks the rule again, so a feeder whose time
/// settings were changed behind the server's back gets the household's back
/// without reconnecting.
#[test]
fn each_time_sync_puts_the_rule_back() {
    local(async {
        let sim = feeder::start().await;
        let mut quick = config();
        quick.timing.time_sync = Duration::from_millis(300);
        let server = TestService::start_with(quick).await;
        let id = adopted(&server, &sim).await;
        holds(&sim, BERLIN).await;

        let mut owner = open_owner(&sim).await;
        let Answer::Time(view) = owner.ask(rand_id(), &Op::GetTime).await.unwrap() else {
            panic!("no time settings");
        };
        let put = Op::PutTime {
            base: view.generation,
            changes: wellfed_model::TimeChanges {
                zone: Some(DENVER.into()),
                ntp: None,
            },
        };
        owner.ask(rand_id(), &put).await.unwrap();
        drop(owner);

        holds(&sim, BERLIN).await;
        let feeders = server.service().feeders().await.unwrap();
        assert!(
            feeders
                .iter()
                .any(|f| f.id == id && f.connection == Link::Online),
            "on the same session"
        );
        server.shutdown().await;
        sim.stop().await;
    });
}

/// The zone survives the server's restart: the next start gives it to the
/// mirror again.
#[test]
fn the_zone_is_kept_across_a_server_restart() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let server = TestService::start().await;
        let id = adopted(&server, &sim).await;
        let denver = HouseholdPatch {
            zone: Some("America/Denver".into()),
            ..HouseholdPatch::default()
        };
        server
            .service()
            .update_household(&owner(), denver)
            .await
            .unwrap();
        holds(&sim, DENVER).await;
        let store = server.stop().await;

        // The owner sets another rule while the server is away.
        let mut owner = open_owner(&sim).await;
        let Answer::Time(view) = owner.ask(rand_id(), &Op::GetTime).await.unwrap() else {
            panic!("no time settings");
        };
        let put = Op::PutTime {
            base: view.generation,
            changes: wellfed_model::TimeChanges {
                zone: Some(BERLIN.into()),
                ntp: None,
            },
        };
        owner.ask(rand_id(), &put).await.unwrap();
        drop(owner);
        assert_eq!(feeder_zone(&sim).await.as_deref(), Some(BERLIN));

        let server = TestService::reopen(store).await;
        server.link(id, Link::Online).await;
        holds(&sim, DENVER).await;

        server.shutdown().await;
        sim.stop().await;
    });
}

/// A feeder that enrolled the server's key with the server role lets it
/// sync the clock but not set the time settings: the feeder stays online
/// and keeps the syncs' offsets.
#[test]
fn a_server_key_without_the_owner_role_keeps_the_feeder_online() {
    local(async {
        let sim = feeder::start().await;
        let server = TestService::start().await;
        server.service().setup(ada_setup()).await.unwrap();
        let id = server.adopt(&sim).await;
        server.link(id, Link::Online).await;

        let detail = server.service().feeder(id).await.unwrap();
        assert!(detail.status.is_some_and(|status| status.live));
        assert_eq!(feeder_zone(&sim).await, None);

        server.shutdown().await;
        sim.stop().await;
    });
}

/// Breakfast at 07:30 Denver time, on each side of the fall change
/// (2026-11-01, 02:00 MDT back to 01:00 MST): 13:30Z on October 31 at
/// -6 h, 14:30Z on November 2 at -7 h.
const BEFORE_FALL: i64 = 1_793_453_400;
const AFTER_FALL: i64 = 1_793_629_800;
const DAY: i64 = 86_400;

/// Set the feeder's clock, as an owner's client does, to `unix_s` less two
/// seconds, at UTC offsets: the zone rule the feeder keeps overrides them.
async fn set_clock(owner: &mut Connection, unix_s: i64) {
    let sync = TimeSync {
        unix_ms: u64::try_from(unix_s * 1000 - 2000).unwrap(),
        offset_s: 0,
        next_change_s: 0,
        next_offset_s: 0,
    };
    let answer = owner.ask(rand_id(), &Op::TimeSync(sync)).await.unwrap();
    assert!(matches!(answer, Answer::Done { .. }), "{answer:?}");
}

/// When the next scheduled feed from plan 0 at or after `from` ended.
async fn fed_from(owner: &mut Connection, from: i64) -> Timestamp {
    tokio::time::timeout(WAIT, async {
        loop {
            let Pushed::Event(event) = owner.next(WAIT).await.unwrap() else {
                continue;
            };
            if let (EventBody::Fed { by, .. }, Some(at)) = (event.body, event.at)
                && by == (Source::Schedule { plan: 0 })
                && at.as_second() >= from
            {
                return at;
            }
        }
    })
    .await
    .expect("the meal runs")
}

/// The feeder serves each meal at its local time across a DST change with
/// the server stopped, from the zone rule the server gave it; the server
/// back afterwards reckons the next meal at the same local time.
#[test]
fn meals_keep_their_local_time_across_a_dst_change_with_the_server_down() {
    local(async {
        let sim = feeder::start().await;
        let server = TestService::start().await;
        let id = adopted(&server, &sim).await;
        let denver = HouseholdPatch {
            zone: Some("America/Denver".into()),
            ..HouseholdPatch::default()
        };
        let service = server.service();
        service.update_household(&owner(), denver).await.unwrap();
        holds(&sim, DENVER).await;

        let breakfast = Plan {
            minute: 7 * 60 + 30,
            portions: 1,
            days: Weekdays::DAILY,
            enabled: true,
            call: 0,
        };
        let mut plans = [None; PLANS];
        plans[0] = Some(breakfast);
        let read = service.plans(id).await.unwrap();
        service
            .put_plans(&owner(), id, read.generation, PlansChange { plans }, None)
            .await
            .unwrap();
        until("the server holds the plans", || async {
            service.feeder(id).await.unwrap().next_meal.is_some()
        })
        .await;
        let store = server.stop().await;

        // The owner's client moves the clock, at UTC; nothing else speaks
        // to the feeder.
        let mut owner = open_owner(&sim).await;
        owner.subscribe(None, false).await.unwrap();
        set_clock(&mut owner, BEFORE_FALL).await;
        let fed = fed_from(&mut owner, BEFORE_FALL - 2).await;
        assert_eq!(fed.as_second() / 60, BEFORE_FALL / 60, "07:30 MDT");

        set_clock(&mut owner, AFTER_FALL).await;
        let fed = fed_from(&mut owner, AFTER_FALL - 2).await;
        assert_eq!(fed.as_second() / 60, AFTER_FALL / 60, "07:30 MST");
        drop(owner);
        sim.stop().await;

        // The server back on October 31, half an hour after the 07:30 MDT
        // meal, with the feeder offline: from its copy, the next is 07:30
        // MST on November 1, past the change.
        let clock = ManualClock::new();
        clock.advance_to(Timestamp::from_second(BEFORE_FALL + 1800).unwrap());
        let server = TestService::reopen_with_clock(store, Arc::new(clock)).await;
        let detail = server.service().feeder(id).await.unwrap();
        assert_eq!(
            detail.next_meal,
            Some(NextMeal {
                at: Timestamp::from_second(AFTER_FALL - DAY).unwrap(),
                slot: 0,
                portions: 1,
            })
        );
        server.shutdown().await;
    });
}
