//! Traces, statistics and the debug log against the simulated feeder: each
//! feed's trace is fetched after its `Fed` event and stored, a feeder with
//! none says so once and is not asked again, the statistics are read at
//! connect and then periodically, and the debug log runs on a session of
//! its own only while someone follows it.

mod support;

use std::time::Duration;

use jiff::Timestamp;
use wellfed_api::{ClientRole, Connection as Link, FeederRefusal, StatsSection};
use wellfed_mirror::{Diagnostics, Feeder, LogError, Mirror};
use wellfed_model::{Answer, LogLines, Op, Refusal, Trace, TraceEntry, TraceMotor};
use wellfed_server_testkit::feeder::{self, Flash, Relay};
use wellfed_server_testkit::{TempStore, WAIT, local, until};
use wellfed_store::{Store, TraceFound};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;

use support::{ID, SECRET, mirrored, mirrored_as, mirrored_with};

const READ: Diagnostics = Diagnostics::Read {
    stats_every: Duration::from_secs(3600),
};

async fn feed(mirror: &Mirror, request: u8) -> u32 {
    let answer = mirror
        .ask(ID, [request; 16], Op::Feed { portions: 1 })
        .await
        .unwrap();
    let Answer::Feeding { feed } = answer else {
        panic!("a feed answered {answer:?}");
    };
    feed
}

async fn traced(store: &Store, feed: u32) -> TraceFound {
    until("the feed's trace is fetched", || async {
        store.trace(ID, None, feed).await.unwrap().is_some()
    })
    .await;
    store.trace(ID, None, feed).await.unwrap().unwrap().found
}

/// The feeder's sessions, its own count.
async fn sessions(mirror: &Mirror) -> u8 {
    let Answer::Status(status) = mirror.ask(ID, [0x5e; 16], Op::Status).await.unwrap() else {
        panic!("a status read answered otherwise");
    };
    status.sessions
}

#[test]
fn each_feeds_trace_is_fetched_after_its_fed_event() {
    local(async {
        let temp = TempStore::new();
        let store = temp.open().await;
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let address = sim.address.to_string();
        let (mirror, _updates) =
            mirrored_with(&store, &sim, &address, WAIT, ClientRole::Server, READ).await;
        let summary = feeder::store_trace(&flash, 1);
        let food_reads = || async {
            let (from, to) = (Timestamp::UNIX_EPOCH, Timestamp::MAX);
            store.food_readings(ID, from, to).await.unwrap().len()
        };
        until("the connect's statistics are read", || async {
            food_reads().await == 1
        })
        .await;

        assert_eq!(feed(&mirror, 1).await, 1);
        let TraceFound::Trace(raw) = traced(&store, 1).await else {
            panic!("the feeder had the trace");
        };
        assert_eq!(
            food_reads().await,
            2,
            "the food section is read again after each feed, ahead of its trace"
        );
        assert_eq!(raw.summary, summary);
        assert!(raw.stored, "the simulator serves its ring");
        let trace = Trace::decode(raw.summary, raw.stored, &raw.body).unwrap();
        assert_eq!(trace.entries.len(), 7);
        assert_eq!(
            trace.entries[1],
            TraceEntry::Motor {
                at_ms: 0,
                motor: TraceMotor::Drive0
            }
        );

        // No trace of feed 2 on the feeder: its answer is kept, once.
        assert_eq!(feed(&mirror, 2).await, 2);
        assert_eq!(
            traced(&store, 2).await,
            TraceFound::Missing(FeederRefusal::NotFound)
        );
        feeder::store_trace(&flash, 2);
        feeder::store_trace(&flash, 3);
        assert_eq!(feed(&mirror, 3).await, 3);
        assert!(matches!(traced(&store, 3).await, TraceFound::Trace(_)));
        assert_eq!(
            store.trace(ID, None, 2).await.unwrap().unwrap().found,
            TraceFound::Missing(FeederRefusal::NotFound),
            "a feed the feeder had no trace of is not asked for again"
        );

        mirror.shutdown().await;
        store.close().await;
        sim.stop().await;
    });
}

#[test]
fn a_feed_that_ended_while_the_server_was_away_is_traced_when_it_connects() {
    local(async {
        let temp = TempStore::new();
        let store = temp.open().await;
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let address = sim.address.to_string();
        let (mirror, _updates) =
            mirrored_with(&store, &sim, &address, WAIT, ClientRole::Server, READ).await;
        feeder::store_trace(&flash, 1);
        assert_eq!(feed(&mirror, 1).await, 1);
        traced(&store, 1).await;
        assert!(mirror.stop(ID).await);

        // The owner feeds while the server is down.
        let mut owner = feeder::open_owner(&sim).await;
        feeder::store_trace(&flash, 2);
        let answer = owner.ask([0x77; 16], &Op::Feed { portions: 1 }).await;
        assert_eq!(answer.unwrap(), Answer::Feeding { feed: 2 });
        let ended = tokio::time::timeout(WAIT, async {
            while !sim_idle(&mut owner).await {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        assert!(ended.await.is_ok(), "the owner's feed ends");
        drop(owner);

        let _updates = mirror
            .start(Feeder {
                id: ID,
                identity: sim.identity,
                secret: SECRET,
                address: Some(address),
            })
            .await;
        assert!(matches!(traced(&store, 2).await, TraceFound::Trace(_)));

        mirror.shutdown().await;
        store.close().await;
        sim.stop().await;
    });
}

/// Whether the simulator has no feed in flight.
async fn sim_idle(owner: &mut wellfed_connect::Connection) -> bool {
    let Ok(Answer::Status(status)) = owner.ask([0x78; 16], &Op::Status).await else {
        return false;
    };
    status.readiness == Some(wellfed_model::Readiness::Ready)
}

#[test]
fn the_statistics_are_read_at_connect_and_then_periodically() {
    local(async {
        let temp = TempStore::new();
        let store = temp.open().await;
        let sim = feeder::start().await;
        let address = sim.address.to_string();
        let every = Diagnostics::Read {
            stats_every: Duration::from_millis(300),
        };
        let (mirror, _updates) =
            mirrored_with(&store, &sim, &address, WAIT, ClientRole::Server, every).await;

        until("every section the server may read is stored", || async {
            store.latest_stats(ID).await.unwrap().len() == 6
        })
        .await;
        let sections: Vec<_> = store
            .latest_stats(ID)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.stats.section())
            .collect();
        // The memory section is an owner's.
        assert_eq!(
            sections,
            [
                StatsSection::Counters,
                StatsSection::Flash,
                StatsSection::Power,
                StatsSection::Food,
                StatsSection::Feeds,
                StatsSection::Portions,
            ]
        );
        let rows = store.latest_stats(ID).await.unwrap();
        assert!(
            rows.iter().all(|row| row.boot_count == Some(1)),
            "each read names the boot it counts in"
        );

        let (from, to) = (Timestamp::UNIX_EPOCH, Timestamp::MAX);
        until("the food section is read again", || async {
            store.food_readings(ID, from, to).await.unwrap().len() >= 2
        })
        .await;
        let readings = store.food_readings(ID, from, to).await.unwrap();
        assert_eq!(readings[0].food.emptied, 0);
        assert_eq!(readings[0].boot_count, Some(1));

        mirror.shutdown().await;
        store.close().await;
        sim.stop().await;
    });
}

#[test]
fn an_owners_key_follows_the_debug_log_only_while_someone_follows() {
    local(async {
        let temp = TempStore::new();
        let store = temp.open().await;
        let sim = feeder::start().await;
        let address = sim.address.to_string();
        let (mirror, _updates) = mirrored_as(&store, &sim, &address, WAIT, ClientRole::Owner).await;
        assert_eq!(sessions(&mirror).await, 1);

        let mut first = mirror.follow_log(ID).await.unwrap();
        let mut seen = Vec::new();
        while !seen
            .iter()
            .any(|line: &String| line.contains("op=tail-log"))
        {
            let batch = tokio::time::timeout(WAIT, first.recv())
                .await
                .expect("a batch in time")
                .expect("the tail is open");
            seen.extend(batch.lines);
        }
        assert_eq!(sessions(&mirror).await, 2, "the tail has its own session");
        let second = mirror.follow_log(ID).await.unwrap();
        assert_eq!(sessions(&mirror).await, 2, "a second follower joins it");

        drop(first);
        drop(second);
        until("the tail's session closes", || async {
            sessions(&mirror).await == 1
        })
        .await;

        mirror.shutdown().await;
        store.close().await;
        sim.stop().await;
    });
}

#[test]
fn the_debug_log_is_refused_to_a_key_that_is_not_an_owners() {
    local(async {
        let temp = TempStore::new();
        let store = temp.open().await;
        let sim = feeder::start().await;
        let address = sim.address.to_string();
        let (mirror, _updates) = mirrored(&store, &sim, &address, WAIT).await;

        let refused = mirror.follow_log(ID).await;
        assert!(
            matches!(refused, Err(LogError::Refused(Refusal::Forbidden))),
            "{refused:?}"
        );
        assert_eq!(sessions(&mirror).await, 1, "the refused session is closed");

        mirror.shutdown().await;
        store.close().await;
        sim.stop().await;
    });
}

/// Read `log` until it closes, as a follower's stream would end.
async fn ends(mut log: Receiver<LogLines>, what: &str) {
    let ended = tokio::time::timeout(WAIT, async {
        loop {
            match log.recv().await {
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return,
            }
        }
    });
    assert!(ended.await.is_ok(), "{what}: the follower's log ends");
}

/// The first batch a follower gets.
async fn first_batch(log: &mut Receiver<LogLines>) {
    tokio::time::timeout(WAIT, log.recv())
        .await
        .expect("a batch in time")
        .expect("the tail is open");
}

#[test]
fn a_followers_log_ends_when_its_tail_does() {
    local(async {
        let temp = TempStore::new();
        let store = temp.open().await;
        let sim = feeder::start().await;
        let relay = Relay::start(sim.address).await;
        let address = relay.address.to_string();
        let (mirror, _updates) = mirrored_as(&store, &sim, &address, WAIT, ClientRole::Owner).await;

        // The feeder drops the tail's session.
        let mut log = mirror.follow_log(ID).await.unwrap();
        first_batch(&mut log).await;
        relay.cut();
        ends(log, "a cut session").await;

        // A follower after it starts another tail.
        until("the mirror is back", || async {
            mirror.link(ID) == Some(Link::Online)
        })
        .await;
        let mut log = mirror.follow_log(ID).await.unwrap();
        first_batch(&mut log).await;
        let joined = mirror.follow_log(ID).await.unwrap();

        // The feeder's task stops.
        assert!(mirror.stop(ID).await);
        ends(log, "a stopped feeder").await;
        ends(joined, "a joined follower").await;

        mirror.shutdown().await;
        store.close().await;
        sim.stop().await;
    });
}
