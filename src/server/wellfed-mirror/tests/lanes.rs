//! The command lanes against the simulated feeder: a long operation takes
//! turns with short commands, a short command waits at most one step, and
//! a cut session leaves every request in flight unknown; the task's own
//! clock and zone requests share the session with the lanes.

mod support;

use std::cell::{Cell, RefCell};
use std::time::Duration;

use wellfed_api::ClientRole;
use wellfed_connect::ANSWER;
use wellfed_mirror::{AskError, MirrorEvent, Zone};
use wellfed_model::{Answer, Op};
use wellfed_server_testkit::feeder::{self, Relay};
use wellfed_server_testkit::fixed::FixedClock;
use wellfed_server_testkit::{TempStore, WAIT, local};

use support::{ID, mirrored, mirrored_as};

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
/// arriving, so the two lanes are in flight together again and again:
/// every step and every read gets the answer to its own request, none is
/// lost, and no read waits out the operation. The order between a read
/// and the next step is pinned by the stalled-feeder tests above; here
/// the feeder answers at once, so the count of steps a read saw finish is
/// a bound, not a check of the lanes' order.
#[test]
fn under_a_burst_of_steps_every_command_gets_its_own_answer() {
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
                let answer = if step % 2 == 0 {
                    let answer = long.step([step; 16], Op::GetSettings).await;
                    matches!(answer, Ok(Answer::Settings(_)))
                } else {
                    let answer = long.step([step; 16], Op::Status).await;
                    matches!(answer, Ok(Answer::Status(_)))
                };
                assert!(answer, "step {step} got another request's answer");
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

/// The feeder stalls with a lock write in flight when the household's zone
/// changes, so the task's own clock and zone requests queue behind the
/// write. Each answer reaches its own request: the write gets its `Done`,
/// and the zone rule the task read and set is on the feeder.
#[test]
fn the_tasks_own_requests_and_a_write_in_flight_each_get_their_own_answer() {
    local(async {
        let temp = TempStore::new();
        let store = temp.open().await;
        let sim = feeder::start().await;
        let relay = Relay::start(sim.address).await;
        let address = relay.address.to_string();
        // Only an owner's key may set the feeder's zone.
        let (mirror, _updates) = mirrored_as(&store, &sim, &address, WAIT, ClientRole::Owner).await;
        let zone = Zone::new("Europe/Berlin", FixedClock.now()).unwrap();
        let rule = zone.rule().expect("Berlin has a rule").to_owned();

        relay.pause();
        let write = mirror.ask(ID, [2; 16], Op::SetLock { locked: false });
        let change = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            mirror.set_zone(zone);
            tokio::time::sleep(Duration::from_millis(150)).await;
            relay.resume();
        };
        let (write, ()) = tokio::join!(write, change);

        assert_eq!(write, Ok(Answer::Done { generation: None }));
        let time = mirror.ask(ID, [3; 16], Op::GetTime).await;
        let Ok(Answer::Time(view)) = time else {
            panic!("the feeder's time settings: {time:?}");
        };
        assert_eq!(view.zone, Some(rule));
        mirror.shutdown().await;
        store.close().await;
        sim.stop().await;
    });
}

/// The feeder stalls past the session's answer window with a lock write
/// in flight and the task's own clock request queued behind it. The write
/// is unknown, as any request unanswered in time is; the clock request
/// running out of time does not end the session, and the next command is
/// served on it. The callers' own window is longer here, so the write's
/// outcome is the session's.
#[test]
fn the_tasks_own_request_unanswered_in_time_keeps_the_session() {
    local(async {
        let temp = TempStore::new();
        let store = temp.open().await;
        let sim = feeder::start().await;
        let relay = Relay::start(sim.address).await;
        let address = relay.address.to_string();
        let (mirror, mut updates) = mirrored(&store, &sim, &address, ANSWER * 2).await;
        let zone = Zone::new("Europe/Berlin", FixedClock.now()).unwrap();

        relay.pause();
        let write = mirror.ask(ID, [2; 16], Op::SetLock { locked: false });
        let change = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            mirror.set_zone(zone);
            tokio::time::sleep(ANSWER + Duration::from_millis(500)).await;
            relay.resume();
        };
        let (write, ()) = tokio::join!(write, change);
        assert_eq!(write, Err(AskError::Unknown));

        let status = mirror.ask(ID, [3; 16], Op::Status).await;
        assert!(matches!(status, Ok(Answer::Status(_))), "{status:?}");
        while let Ok(update) = updates.try_recv() {
            assert!(
                !matches!(update, MirrorEvent::Link(_)),
                "the session ended: {update:?}"
            );
        }
        mirror.shutdown().await;
        store.close().await;
        sim.stop().await;
    });
}
