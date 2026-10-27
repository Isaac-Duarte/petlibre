//! The mirror on its own, against the simulated feeder: updates follow the
//! store, a command whose caller left is never sent, and a stopped feeder
//! is let go (offline last) once its task has ended.

mod support;

use std::time::Duration;

use jiff::Timestamp;
use wellfed_api::{Connection as Link, EventBody, EventKind, FeederEvent, Source};
use wellfed_mirror::{AskError, MirrorEvent};
use wellfed_model::{Answer, Op};
use wellfed_server_testkit::feeder::{self, Relay};
use wellfed_server_testkit::{TempStore, WAIT, local, until};
use wellfed_store::Store;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::TryRecvError;

use support::{ID, mirrored, next};

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
        let projected = || store.lock_changes(ID, Timestamp::MIN, Timestamp::MAX);
        let changes = projected().await.unwrap();
        assert_eq!(changes.len(), 1, "projected with it");
        assert_eq!((changes[0].epoch, changes[0].seq), (event.epoch, event.seq));
        assert!(!changes[0].value.locked);
        assert!(matches!(changes[0].value.by, Source::Client { .. }));

        // The same request id is answered from the feeder's memory.
        let again = mirror.ask(ID, [1; 16], lock).await.unwrap();
        assert_eq!(again, answer);
        let status = mirror.ask(ID, [2; 16], Op::Status).await.unwrap();
        assert!(matches!(status, Answer::Status(_)));
        assert_eq!(locks(&store, event.epoch).await, [false], "once");
        assert_eq!(projected().await.unwrap(), changes, "projected once");

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
