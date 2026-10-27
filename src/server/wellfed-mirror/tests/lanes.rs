//! The command lanes against the simulated feeder: a long operation takes
//! turns with short commands, a short command waits at most one step, and
//! a cut session leaves every request in flight unknown.

mod support;

use std::cell::{Cell, RefCell};
use std::time::Duration;

use wellfed_mirror::AskError;
use wellfed_model::{Answer, Op};
use wellfed_server_testkit::feeder::{self, Relay};
use wellfed_server_testkit::{TempStore, WAIT, local};

use support::{ID, mirrored};

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

        // The read goes out beside the first step, so both answers may come
        // in one read and their callers wake in either order; the next step
        // is answered after both.
        let order = order.into_inner();
        assert_eq!(order.len(), 3, "{order:?}");
        assert_eq!(order.last(), Some(&"step 2"), "{order:?}");
        drop(long);
        mirror.shutdown().await;
        store.close().await;
        sim.stop().await;
    });
}

/// A step is on the wire when the feeder stalls; a lock write, a feed and
/// a status read are asked during it. The write goes out beside the step
/// and the feeder answers it only once its store has it, a pass or more
/// after the step; the feed and the read wait behind it on the short lane,
/// and the operation's next step waits for all three.
#[test]
fn a_feed_or_status_during_a_long_operation_is_answered_before_its_next_step() {
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
        let shorts = async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let unlock = mirror.ask(ID, [3; 16], Op::SetLock { locked: false });
            let feed = async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                mirror.ask(ID, [4; 16], Op::Feed { portions: 1 }).await
            };
            let status = async {
                tokio::time::sleep(Duration::from_millis(40)).await;
                mirror.ask(ID, [5; 16], Op::Status).await
            };
            let (unlocked, fed, status) = tokio::join!(unlock, feed, status);
            assert_eq!(unlocked, Ok(Answer::Done { generation: None }));
            // The simulator has no feeding owner, so it refuses the feed
            // (`unavailable`): an answer all the same, in its place.
            assert!(fed.is_ok(), "{fed:?}");
            assert!(matches!(status, Ok(Answer::Status(_))), "{status:?}");
            order.borrow_mut().push("short");
        };
        let stall = async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            relay.resume();
        };
        tokio::join!(steps, shorts, stall);

        assert_eq!(*order.borrow(), ["step 1", "short", "step 2"]);
        drop(long);
        mirror.shutdown().await;
        store.close().await;
        sim.stop().await;
    });
}

/// A long operation sends its steps back to back while status reads keep
/// arriving: each read is answered with at most one step finished between
/// its asking and its answer.
#[test]
fn under_a_burst_of_steps_a_short_command_waits_at_most_one_step() {
    const STEPS: u8 = 60;
    const READS: u8 = 12;

    local(async {
        let temp = TempStore::new();
        let store = temp.open().await;
        let sim = feeder::start().await;
        let address = sim.address.to_string();
        let (mirror, _updates) = mirrored(&store, &sim, &address, WAIT).await;
        let mut long = mirror.operation(ID).await.unwrap();
        let done = Cell::new(0u32);
        let waited = RefCell::new(Vec::new());

        let steps = async {
            for step in 0..STEPS {
                let op = if step % 2 == 0 {
                    Op::GetSettings
                } else {
                    Op::Status
                };
                long.step([step; 16], op).await.unwrap();
                done.set(done.get() + 1);
            }
        };
        let reads = async {
            for read in 0..READS {
                tokio::time::sleep(Duration::from_millis(3)).await;
                let asked = done.get();
                let answer = mirror.ask(ID, [0x80 | read; 16], Op::Status).await;
                assert!(matches!(answer, Ok(Answer::Status(_))), "{answer:?}");
                waited.borrow_mut().push(done.get() - asked);
            }
        };
        tokio::join!(steps, reads);

        assert_eq!(done.get(), u32::from(STEPS));
        let waited = waited.into_inner();
        assert_eq!(waited.len(), usize::from(READS));
        assert!(
            waited.iter().all(|&steps| steps <= 1),
            "steps finished while each read waited: {waited:?}"
        );
        drop(long);
        mirror.shutdown().await;
        store.close().await;
        sim.stop().await;
    });
}

/// The feeder stalls with a step and a lock write in flight, one on each
/// lane, and a status read queued behind the write. Then the feeder goes
/// away: the two in flight may have run, so both are unknown at once, and
/// the read that never went out is offline.
#[test]
fn a_session_cut_with_requests_in_flight_leaves_them_all_unknown() {
    local(async {
        let temp = TempStore::new();
        let store = temp.open().await;
        let sim = feeder::start().await;
        let relay = Relay::start(sim.address).await;
        let address = relay.address.to_string();
        let (mirror, _updates) = mirrored(&store, &sim, &address, WAIT).await;
        let mut long = mirror.operation(ID).await.unwrap();

        relay.pause();
        let started = tokio::time::Instant::now();
        let step = long.step([1; 16], Op::Status);
        let write = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            mirror.ask(ID, [2; 16], Op::SetLock { locked: false }).await
        };
        let queued = async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            mirror.ask(ID, [3; 16], Op::Status).await
        };
        let cut = async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            sim.stop().await;
            relay.resume();
        };
        let (step, write, queued, ()) = tokio::join!(step, write, queued, cut);

        assert_eq!(step, Err(AskError::Unknown));
        assert_eq!(write, Err(AskError::Unknown));
        assert_eq!(queued, Err(AskError::Offline));
        assert!(
            started.elapsed() < WAIT / 2,
            "settled by the cut, not by the answer window"
        );
        drop(long);
        mirror.shutdown().await;
        store.close().await;
    });
}

/// A short command under the id of the step in flight is held until the
/// step is answered, then sent: each gets its own answer, where one answer
/// matched to both would leave the other without one.
#[test]
fn a_command_under_an_id_in_flight_waits_for_its_answer() {
    local(async {
        let temp = TempStore::new();
        let store = temp.open().await;
        let sim = feeder::start().await;
        let relay = Relay::start(sim.address).await;
        let address = relay.address.to_string();
        let (mirror, _updates) = mirrored(&store, &sim, &address, WAIT).await;
        let mut long = mirror.operation(ID).await.unwrap();

        relay.pause();
        let step = long.step([7; 16], Op::GetSettings);
        let short = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            mirror.ask(ID, [7; 16], Op::Status).await
        };
        let stall = async {
            tokio::time::sleep(Duration::from_millis(150)).await;
            relay.resume();
        };
        let (step, short, ()) = tokio::join!(step, short, stall);

        assert!(matches!(step, Ok(Answer::Settings(_))), "{step:?}");
        assert!(matches!(short, Ok(Answer::Status(_))), "{short:?}");
        drop(long);
        mirror.shutdown().await;
        store.close().await;
        sim.stop().await;
    });
}
