//! Battery mode on the simulator: the loop in virtual time for what runs
//! on the batteries (the radio, meals, the floor, the brownout hold), and
//! the device API over real TCP for what a client sees (the supply and its
//! charge in the status, an update refused, the session lost and back).

use std::cell::RefCell;
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr};
use std::rc::Rc;
use std::time::Duration;

use tokio::task::LocalSet;
use wellfed_connect::Connection;
use wellfed_core::clock::wall::{Origin, Sync};
use wellfed_core::liveness::Radio;
use wellfed_core::schedule::{PLANS, Plan, Table, Weekdays};
use wellfed_device_api::Reader;
use wellfed_device_api::mailbox::{PlansChange, PlansResult};
use wellfed_feeder::feeder::Platform;
use wellfed_feeder::feeder::supply::{RADIO_OFF_MS, RELEASE_MS, Released};
use wellfed_feeder::feeder::{self, supply};
use wellfed_feeder::store::events::Read;
use wellfed_feeder_sim::banks::Banks;
use wellfed_feeder_sim::flash::Flash;
use wellfed_feeder_sim::harness;
use wellfed_feeder_sim::platform::SimSupply;
use wellfed_feeder_sim::serve::Control;
use wellfed_feeder_sim::sim::{Record, Sim};
use wellfed_model::{Answer, Op, Refusal, UpdateRefusal};
use wellfed_proto::api::RequestId;
use wellfed_proto::api::{Battery, Blocked, EventBody, FeedOutcome, MissedReason, Source, Supply};
use wellfed_session::noise::Keypair;

const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
/// 2026-10-28T07:00:00Z, a Wednesday.
const SEVEN: u64 = 1_793_145_600_000 + 7 * HOUR_MS;

/// Three fresh D cells: about 4757 mV at the pack, 100 %.
const FRESH_PIN_MV: u32 = 2306;
/// A pack under the floor (1650 at the pin): about 3.3 V.
const FLAT_PIN_MV: u32 = 1600;
/// The ROM's brownout flag.
const BROWNOUT: u32 = 1 << 18;

/// A simulated feeder on its own flash, its uptime, and what its loop
/// recorded.
struct Feeder {
    sim: Sim,
    now: u32,
    records: Vec<Record>,
}

impl Feeder {
    /// A feeder whose boot's reset flags are `reset_flags`, on `supply`.
    fn boot(reset_flags: u32, supply: SimSupply) -> Self {
        let flash = Flash::memory();
        let banks = Banks::new();
        let random = Box::new(|| Some([0x5a; 32]));
        let mut sim = Sim::open(&flash, &banks, None, random, 0, 1, None).unwrap();
        sim.platform().set_reset_flags(reset_flags);
        sim.platform().set_supply(supply);
        Self {
            sim,
            now: 0,
            records: Vec::new(),
        }
    }

    fn mains() -> Self {
        Self::boot(0, cells(true, FRESH_PIN_MV))
    }

    fn supply(&mut self, supply: SimSupply) {
        self.sim.platform().set_supply(supply);
    }

    /// Run the loop for `ms`, a pass at least every second.
    fn run(&mut self, ms: u64) {
        let end = u64::from(self.now) + ms;
        while u64::from(self.now) < end {
            let records = &mut self.records;
            let delay = self.sim.pass(self.now, |record| records.push(record));
            let step = u64::from(delay.clamp(1, 1000)).min(end - u64::from(self.now));
            self.now += u32::try_from(step.max(1)).unwrap();
        }
    }

    /// An owner's sync: the wall clock reads `unix_ms` now, at UTC.
    fn sync(&mut self, unix_ms: u64) {
        let sync = Sync {
            unix_ms,
            offset_s: 0,
            next_change_s: 0,
            next_offset_s: 0,
        };
        let now = self.now;
        self.sim
            .platform()
            .time_sync(now, sync, Origin::Owner)
            .unwrap();
    }

    fn wall(&mut self) -> u64 {
        let now = self.now;
        self.sim.platform().wall_ms(now).expect("a valid clock")
    }

    fn run_until(&mut self, unix_ms: u64) {
        let wall = self.wall();
        self.run(unix_ms - wall);
    }

    /// One daily plan at `minute` past midnight, saved now.
    fn plan(&mut self, minute: u16) {
        let mut table: Table = [None; PLANS];
        table[0] = Some(Plan {
            minute,
            portions: 1,
            days: Weekdays::EVERY_DAY,
            enabled: true,
            call: 0,
        });
        let at_s = u32::try_from(self.wall() / 1000).ok();
        let change = PlansChange::Replace { table, at_s };
        let queued = self.sim.platform().plans_change(change);
        assert!(matches!(queued, PlansResult::Current { pending: true, .. }));
        self.run(1);
    }

    fn radio(&mut self) -> Radio {
        self.sim.platform().radio_state()
    }

    fn events(&mut self) -> Vec<EventBody> {
        let mut bodies = Vec::new();
        let mut after = None;
        while let Some(Read::Record(record)) =
            self.sim.platform().event_after(Reader::slot(0), after)
        {
            after = Some(record.seq);
            bodies.extend(wellfed_proto::decode_padded::<EventBody>(&record.body));
        }
        bodies
    }

    fn fed(&mut self) -> Vec<(FeedOutcome, Source)> {
        self.events()
            .into_iter()
            .filter_map(|body| match body {
                EventBody::Fed { outcome, by, .. } => Some((outcome, by)),
                _ => None,
            })
            .collect()
    }

    fn supplies(&mut self) -> Vec<Supply> {
        self.events()
            .into_iter()
            .filter_map(|body| match body {
                EventBody::Supply(supply) => Some(supply),
                _ => None,
            })
            .collect()
    }

    /// The loop's battery-mode records (`PLBAT2`), in order.
    fn power_records(&self) -> Vec<supply::Record> {
        self.records
            .iter()
            .filter_map(|record| match record {
                Record::Feeder(feeder::Record::Supply(r)) => Some(*r),
                _ => None,
            })
            .collect()
    }
}

fn cells(adapter: bool, pin_mv: u32) -> SimSupply {
    SimSupply {
        adapter,
        pin_mv: Some(pin_mv),
    }
}

/// Stock's battery mode: 5 s after the adapter goes the radio is off and
/// the feeder has no address; meals still run from the running clock; the
/// adapter's return brings the radio back at once.
#[test]
fn on_batteries_the_radio_goes_off_and_meals_still_run() {
    let mut feeder = Feeder::mains();
    feeder.run(100);
    feeder.sync(SEVEN);
    feeder.plan(7 * 60 + 30);
    assert_eq!(feeder.radio(), Radio::On);

    feeder.supply(cells(false, FRESH_PIN_MV));
    feeder.run(u64::from(RADIO_OFF_MS) - 1_000);
    assert_eq!(feeder.radio(), Radio::On, "not before 5 s");
    feeder.run(2_000);
    assert_eq!(feeder.radio(), Radio::Off);
    assert_eq!(feeder.sim.platform().ipv4(), None, "the address goes");

    // The clock keeps running with the radio off, and breakfast runs.
    feeder.run_until(SEVEN + 31 * MINUTE_MS);
    assert_eq!(feeder.fed(), [(FeedOutcome::Done, Source::Schedule(0))]);
    assert!(feeder.sim.platform().wall_ms(feeder.now).is_some());

    feeder.supply(cells(true, FRESH_PIN_MV));
    feeder.run(100);
    assert_eq!(feeder.radio(), Radio::On);
    assert!(feeder.sim.platform().ipv4().is_some());

    // The adapter's loss and return are in the history, with the charge.
    let changes: Vec<_> = feeder
        .supplies()
        .into_iter()
        .map(|supply| (supply.adapter, supply.battery))
        .collect();
    let fresh = Battery::Reading {
        mv: 4757,
        percent: 100,
    };
    assert_eq!(changes, [(Some(false), fresh), (Some(true), fresh)]);
    assert_eq!(
        feeder.power_records(),
        [
            supply::Record::Radio(Radio::Off),
            supply::Record::Radio(Radio::On),
        ]
    );
}

/// Low battery has a defined result: on the pack alone under the floor,
/// no feed starts (readiness `Battery`), and a meal due meanwhile is
/// missed as blocked by it once its grace runs out; never fed late.
#[test]
fn under_the_floor_a_meal_is_missed_as_blocked_by_the_battery() {
    let mut feeder = Feeder::mains();
    feeder.run(100);
    feeder.sync(SEVEN);
    feeder.plan(7 * 60 + 30);

    feeder.supply(cells(false, FLAT_PIN_MV));
    feeder.run_until(SEVEN + 50 * MINUTE_MS);
    assert!(feeder.fed().is_empty(), "nothing fed on a flat pack");
    let missed: Vec<_> = feeder
        .events()
        .into_iter()
        .filter_map(|body| match body {
            EventBody::Missed { reason, .. } => Some(reason),
            _ => None,
        })
        .collect();
    assert_eq!(missed, [MissedReason::Blocked(Blocked::Battery)]);
    let low = feeder.supplies();
    assert!(
        low.iter()
            .any(|s| s.adapter == Some(false) && s.below_floor == Some(true)),
        "{low:?}"
    );
}

/// After a brownout on the batteries alone, the hold lifts once the pack
/// has read charged for three minutes: no adapter, no power cycle.
#[test]
fn a_brownout_hold_lifts_on_fresh_cells_without_the_adapter() {
    let mut feeder = Feeder::boot(BROWNOUT, cells(false, FRESH_PIN_MV));
    feeder.run(u64::from(RELEASE_MS) - 1_000);
    assert_eq!(feeder.power_records()[0], supply::Record::Held);
    assert!(
        !feeder
            .power_records()
            .contains(&supply::Record::Released(Released::Pack))
    );

    feeder.run(2_000);
    assert!(
        feeder
            .power_records()
            .contains(&supply::Record::Released(Released::Pack)),
        "{:?}",
        feeder.power_records()
    );
}

/// The phantom at a power cut: with no cells, the adapter goes and the
/// decaying rail reads as a pack for a moment. The radio stays on and
/// nothing is held.
#[test]
fn a_rail_decaying_at_a_cut_is_not_battery_mode() {
    let mut feeder = Feeder::boot(0, SimSupply::MAINS);
    feeder.run(1_000);
    feeder.supply(cells(false, 1871));
    feeder.run(200);
    feeder.supply(SimSupply::MAINS);
    feeder.run(10_000);
    assert_eq!(feeder.radio(), Radio::On);
    assert!(
        feeder.power_records().is_empty(),
        "{:?}",
        feeder.power_records()
    );
}

const OWNER: [u8; 32] = [0x22; 32];

fn paused(test: impl Future<Output = ()>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap();
    runtime.block_on(LocalSet::new().run_until(test));
}

async fn open(sim: &harness::Running, attempt: u8) -> Connection {
    let keys = Keypair::from_secret(OWNER);
    for retry in 0u8..100 {
        let ephemeral = [attempt ^ retry ^ 0x5c; 32];
        if let Ok(connection) = Connection::open(&keys, sim.identity, sim.address, ephemeral).await
        {
            return connection;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the feeder never answered");
}

fn id(n: u8) -> RequestId {
    [n; 16]
}

/// What a client sees: the supply with its charge in the status, an
/// update refused by name on the batteries, the session gone with the
/// radio, and the feeder back once the adapter is.
#[test]
fn a_client_sees_the_supply_and_loses_the_feeder_on_batteries() {
    paused(async {
        let flash = Flash::memory();
        let owner = *Keypair::from_secret(OWNER).public();
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        let banks: Rc<RefCell<Banks>> = Banks::new();
        let sim = harness::Running::start_with(&flash, &banks, None, Some(owner), address).await;
        sim.control(Control::Pack(Some(FRESH_PIN_MV)));
        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut connection = open(&sim, 1).await;
        let Answer::Status(status) = connection.ask(id(1), &Op::Status).await.unwrap() else {
            panic!("a status");
        };
        let supply = status.supply.expect("the supply, to a POWER session");
        assert_eq!(supply.adapter, Some(true));
        assert_eq!(
            supply.battery,
            wellfed_model::Battery::Reading {
                mv: 4757,
                percent: 100,
            }
        );

        sim.control(Control::Adapter(false));
        tokio::time::sleep(Duration::from_millis(100)).await;
        let offer = Op::OfferUpdate {
            manifest: vec![0; 200],
            downgrade: None,
        };
        let answer = connection.ask(id(2), &offer).await.unwrap();
        assert_eq!(
            answer,
            Answer::Refused(Refusal::Update(UpdateRefusal::Battery))
        );
        let Answer::Status(status) = connection.ask(id(3), &Op::Status).await.unwrap() else {
            panic!("a status");
        };
        assert_eq!(status.supply.and_then(|s| s.adapter), Some(false));

        // The radio goes: the session with it.
        tokio::time::sleep(Duration::from_millis(u64::from(RADIO_OFF_MS) + 1_000)).await;
        assert!(connection.ask(id(4), &Op::Status).await.is_err());

        sim.control(Control::Adapter(true));
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut connection = open(&sim, 2).await;
        let Answer::Status(status) = connection.ask(id(5), &Op::Status).await.unwrap() else {
            panic!("a status");
        };
        assert_eq!(status.supply.and_then(|s| s.adapter), Some(true));
        sim.stop().await;
    });
}
