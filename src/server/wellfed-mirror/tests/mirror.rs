//! The mirror on its own, against the simulated feeder: updates follow the
//! store, a command whose caller left is never sent, a long operation
//! takes turns with short commands, and a stopped feeder is let go
//! (offline last) once its task has ended.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

use wellfed_api::{ClientRole, Connection as Link, EventBody, EventKind, FeederEvent, FeederId};
use wellfed_mirror::{AskError, Config, Discovery, Feeder, Invite, Mirror, MirrorEvent, Timing};
use wellfed_model::{Answer, Op};
use wellfed_server_testkit::feeder::{self, Relay, Running};
use wellfed_server_testkit::fixed::{FixedClock, SeededRandom};
use wellfed_server_testkit::{TempStore, WAIT, local, until};
use wellfed_store::{NewFeeder, SealedKey, Store};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::TryRecvError;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const ID: FeederId = FeederId(Uuid::from_u128(7));

fn config(answer: Duration) -> Config {
    Config {
        discovery: Discovery::Off,
        timing: Timing {
            retry_min: Duration::from_millis(50),
            retry_max: Duration::from_millis(400),
            time_sync: Duration::from_secs(3600),
            answer,
        },
    }
}

/// A mirror on `store`, with `sim` enrolled (reached at `address`) and
/// started; its updates from the first, once it is online.
async fn mirrored(
    store: &Store,
    sim: &Running,
    address: &str,
    answer: Duration,
) -> (Mirror, Receiver<MirrorEvent>) {
    let mirror = Mirror::new(
        store.clone(),
        config(answer),
        Arc::new(FixedClock),
        Arc::new(SeededRandom::fresh()),
        CancellationToken::new(),
    );
    let invite = feeder::invite(sim, ClientRole::Server).await;
    let invite = Invite::parse(&invite).expect("the kit's invite parses");
    let identity = invite.feeder;
    let at = mirror.locate(identity, Some(address)).await.unwrap();
    let secret = [0x33; 32];
    let enrolled = mirror.enroll(invite, "test", at, secret).await.unwrap();
    assert_eq!(enrolled.role, ClientRole::Server);

    // The mirror reads the cursor from the feeder's row; the key it uses
    // is the one it is given, so the stored one is a stand-in.
    store
        .insert_feeder(NewFeeder {
            id: ID,
            identity,
            name: "kitchen",
            role: enrolled.role,
            address: Some(address),
            key: SealedKey {
                nonce: [0; 12],
                sealed: Vec::new(),
            },
            created: FixedClock.now(),
        })
        .await
        .unwrap();
    let mut updates = mirror
        .start(Feeder {
            id: ID,
            identity,
            secret,
            address: Some(address.into()),
        })
        .await;

    let mut links = Vec::new();
    while links.last() != Some(&Link::Online) {
        if let MirrorEvent::Link(link) = next(&mut updates).await {
            links.push(link);
        }
    }
    assert_eq!(links, [Link::Connecting, Link::Online]);
    (mirror, updates)
}

async fn next(updates: &mut Receiver<MirrorEvent>) -> MirrorEvent {
    tokio::time::timeout(WAIT, updates.recv())
        .await
        .expect("an update in time")
        .expect("the broadcast is open")
}

/// The next new event the mirror broadcasts.
async fn next_event(updates: &mut Receiver<MirrorEvent>) -> FeederEvent {
    loop {
        if let MirrorEvent::Event(event) = next(updates).await {
            return event;
        }
    }
}

async fn locks(store: &Store, epoch: u32) -> Vec<bool> {
    let rows = store
        .events(ID, epoch, None, 100, Some(EventKind::Lock))
        .await
        .unwrap();
    rows.iter()
        .filter_map(
            |row| match wellfed_mirror::stored_event(epoch, row)?.body {
                EventBody::Lock { locked, .. } => Some(locked),
                _ => None,
            },
        )
        .collect()
}

#[test]
fn an_answered_command_and_its_event_are_stored_before_they_are_broadcast() {
    local(async {
        let temp = TempStore::new();
        let store = temp.open().await;
        let sim = feeder::start().await;
        let address = sim.address.to_string();
        let (mirror, mut updates) = mirrored(&store, &sim, &address, WAIT).await;

        let lock = Op::SetLock { locked: false };
        let answer = mirror.ask(ID, [1; 16], lock.clone()).await.unwrap();
        assert_eq!(answer, Answer::Done { generation: None });
        let event = loop {
            let event = next_event(&mut updates).await;
            if matches!(event.body, EventBody::Lock { .. }) {
                break event;
            }
        };
        assert_eq!(locks(&store, event.epoch).await, [false], "stored first");

        // The same request id is answered from the feeder's memory.
        let again = mirror.ask(ID, [1; 16], lock).await.unwrap();
        assert_eq!(again, answer);
        let status = mirror.ask(ID, [2; 16], Op::Status).await.unwrap();
        assert!(matches!(status, Answer::Status(_)));
        assert_eq!(locks(&store, event.epoch).await, [false], "once");

        mirror.shutdown().await;
        store.close().await;
        sim.stop().await;
    });
}

/// The feeder stalls with one write on the wire and a second queued
/// behind it. The first caller hears `Unknown`; the second gives up
/// before its turn, so its write is dropped unsent and never applied.
#[test]
fn a_command_whose_caller_left_is_never_sent() {
    local(async {
        let temp = TempStore::new();
        let store = temp.open().await;
        let sim = feeder::start().await;
        let relay = Relay::start(sim.address).await;
        let address = relay.address.to_string();
        let answer = Duration::from_millis(500);
        let (mirror, mut updates) = mirrored(&store, &sim, &address, answer).await;

        relay.pause();
        let unlock = mirror.ask(ID, [1; 16], Op::SetLock { locked: false });
        let lock = mirror.ask(ID, [2; 16], Op::SetLock { locked: true });
        let (unlocked, locked) = tokio::join!(unlock, lock);
        assert_eq!(unlocked, Err(AskError::Unknown));
        assert_eq!(locked, Err(AskError::Unknown));
        relay.resume();

        // The task runs its queue in order: once a later command is
        // answered, the abandoned write has had its turn.
        let event = loop {
            let event = next_event(&mut updates).await;
            if matches!(event.body, EventBody::Lock { .. }) {
                break event;
            }
        };
        until("the session serves again", || async {
            mirror.ask(ID, [3; 16], Op::Status).await.is_ok()
        })
        .await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            locks(&store, event.epoch).await,
            [false],
            "the first write landed once; the abandoned one never did"
        );

        mirror.shutdown().await;
        store.close().await;
        sim.stop().await;
    });
}

#[test]
fn a_long_operation_takes_turns_with_short_commands() {
    local(async {
        let temp = TempStore::new();
        let store = temp.open().await;
        let sim = feeder::start().await;
        let address = sim.address.to_string();
        let (mirror, _updates) = mirrored(&store, &sim, &address, WAIT).await;

        let mut long = mirror.operation(ID).await.unwrap();
        let first = long.step([1; 16], Op::Status).await.unwrap();
        assert!(matches!(first, Answer::Status(_)));
        let between = mirror.ask(ID, [2; 16], Op::Status).await.unwrap();
        assert!(
            matches!(between, Answer::Status(_)),
            "a short command between steps"
        );
        let second = long.step([3; 16], Op::GetSettings).await.unwrap();
        assert!(matches!(second, Answer::Settings(_)));

        let waiting = tokio::time::timeout(Duration::from_millis(200), mirror.operation(ID));
        assert!(waiting.await.is_err(), "one long operation at a time");
        drop(long);
        let mut next = mirror.operation(ID).await.unwrap();
        assert!(next.step([4; 16], Op::Status).await.is_ok());

        mirror.shutdown().await;
        store.close().await;
        sim.stop().await;
    });
}

/// A step is on the wire when the feeder stalls, and a short command is
/// queued behind it. The next step, sent once the first is answered, waits
/// behind the short command: a short command waits at most one step.
#[test]
fn a_short_command_queued_during_a_step_runs_before_the_next_step() {
    local(async {
        let temp = TempStore::new();
        let store = temp.open().await;
        let sim = feeder::start().await;
        let relay = Relay::start(sim.address).await;
        let address = relay.address.to_string();
        let (mirror, _updates) = mirrored(&store, &sim, &address, WAIT).await;
        let mut long = mirror.operation(ID).await.unwrap();
        let order = RefCell::new(Vec::new());

        relay.pause();
        let steps = async {
            long.step([1; 16], Op::Status).await.unwrap();
            order.borrow_mut().push("step 1");
            long.step([2; 16], Op::GetSettings).await.unwrap();
            order.borrow_mut().push("step 2");
        };
        let short = async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            mirror.ask(ID, [3; 16], Op::Status).await.unwrap();
            order.borrow_mut().push("ask");
        };
        let stall = async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            relay.resume();
        };
        tokio::join!(steps, short, stall);

        assert_eq!(*order.borrow(), ["step 1", "ask", "step 2"]);
        drop(long);
        mirror.shutdown().await;
        store.close().await;
        sim.stop().await;
    });
}

#[test]
fn a_stopped_feeder_is_let_go() {
    local(async {
        let temp = TempStore::new();
        let store = temp.open().await;
        let sim = feeder::start().await;
        let address = sim.address.to_string();
        let (mirror, mut updates) = mirrored(&store, &sim, &address, WAIT).await;
        assert_eq!(mirror.link(ID), Some(Link::Online));

        assert!(mirror.stop(ID).await);
        assert!(!mirror.stop(ID).await, "already stopped");
        assert_eq!(mirror.link(ID), None);
        assert!(mirror.subscribe(ID).is_none());
        let asked = mirror.ask(ID, [1; 16], Op::Status).await;
        assert_eq!(asked, Err(AskError::NotRunning));

        // `stop` returned after the task ended: its updates are all sent,
        // the last link is offline, and the broadcast is closed.
        let mut last = None;
        loop {
            match updates.try_recv() {
                Ok(MirrorEvent::Link(link)) => last = Some(link),
                Ok(_) => {}
                Err(TryRecvError::Closed) => break,
                Err(other) => panic!("the broadcast is closed and whole: {other:?}"),
            }
        }
        assert_eq!(last, Some(Link::Offline));

        mirror.shutdown().await;
        store.close().await;
        sim.stop().await;
    });
}
