//! The simulator's feeding owner over the device API: a feed's number is
//! its journal id, its result is a `Fed` event, a cancel ends it, and a
//! restart in the middle of one resolves it as uncertain under its own
//! number, never as a new feed. A feed asks for 1 to 48 portions, and a
//! Dispense press takes the same owner.

use std::future::Future;
use std::rc::Rc;
use std::time::Duration;

use tokio::task::LocalSet;
use wellfed_connect::Connection;
use wellfed_feeder_sim::feeding;
use wellfed_feeder_sim::flash::Flash;
use wellfed_feeder_sim::harness::Running;
use wellfed_feeder_sim::serve::Control;
use wellfed_model::{
    Answer, Blocked, EventBody, FeedOutcome, FeedReason, FoodLevel, Op, Pushed, Readiness, Refusal,
    Source,
};
use wellfed_session::noise::Keypair;

const OWNER: [u8; 32] = [0x21; 32];

fn local(test: impl Future<Output = ()>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(LocalSet::new().run_until(test));
}

async fn start(flash: &Rc<Flash>) -> Running {
    Running::start(flash, Some(*Keypair::from_secret(OWNER).public())).await
}

async fn open(sim: &Running) -> Connection {
    Connection::open(
        &Keypair::from_secret(OWNER),
        sim.identity,
        sim.address,
        [0xa1; 32],
    )
    .await
    .unwrap()
}

/// The next `Fed` event the subscription pushes.
async fn next_fed(connection: &mut Connection) -> EventBody {
    loop {
        let pushed = connection.next(Duration::from_secs(5)).await.unwrap();
        if let Pushed::Event(event) = pushed
            && matches!(event.body, EventBody::Fed { .. })
        {
            return event.body;
        }
    }
}

async fn readiness(connection: &mut Connection, id: u8) -> Option<Readiness> {
    match connection.ask([id; 16], &Op::Status).await.unwrap() {
        Answer::Status(status) => status.readiness,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_feed_is_numbered_by_the_journal_and_ends_in_its_fed_event() {
    local(async {
        let flash = Flash::memory();
        let sim = start(&flash).await;
        let mut owner = open(&sim).await;
        owner.subscribe(None, false).await.unwrap();

        let answer = owner.ask([1; 16], &Op::Feed { portions: 2 }).await.unwrap();
        assert_eq!(answer, Answer::Feeding { feed: 1 });
        let again = owner.ask([1; 16], &Op::Feed { portions: 2 }).await.unwrap();
        assert_eq!(again, answer, "a repeated id is the same feed");
        let busy = owner.ask([2; 16], &Op::Feed { portions: 1 }).await.unwrap();
        assert_eq!(busy, Answer::Refused(Refusal::Busy), "one feed at a time");

        let EventBody::Fed {
            feed,
            requested,
            completed,
            outcome,
            reason,
            hopper,
            by: Source::Client { .. },
        } = next_fed(&mut owner).await
        else {
            panic!("a feed by the client");
        };
        assert_eq!((feed, requested, completed), (1, 2, 2));
        assert_eq!((outcome, reason), (FeedOutcome::Done, FeedReason::Done));
        assert_eq!(
            hopper,
            FoodLevel::Present,
            "the simulator's hopper starts full"
        );
        assert_eq!(readiness(&mut owner, 3).await, Some(Readiness::Ready));

        let next = owner.ask([4; 16], &Op::Feed { portions: 1 }).await.unwrap();
        assert_eq!(next, Answer::Feeding { feed: 2 });
        next_fed(&mut owner).await;
        assert_eq!(feeding::reserved(&flash), Some(2));
        sim.stop().await;
    });
}

/// Every feed asks for 1 to 48 portions; anything else is refused by
/// name, with the value, before readiness is checked, and reserves no
/// number.
#[test]
fn a_feed_outside_one_to_48_portions_is_refused_by_name() {
    local(async {
        let flash = Flash::memory();
        let sim = start(&flash).await;
        sim.control(Control::StallFeeds(true));
        let mut owner = open(&sim).await;
        owner.subscribe(None, false).await.unwrap();

        for (id, asked) in [(1, 0), (2, 49), (3, u8::MAX)] {
            let answer = owner.ask([id; 16], &Op::Feed { portions: asked }).await;
            let refused = Answer::Refused(Refusal::Portions { asked });
            assert_eq!(answer.unwrap(), refused, "{asked} portions");
        }
        let most = owner
            .ask([4; 16], &Op::Feed { portions: 48 })
            .await
            .unwrap();
        assert_eq!(most, Answer::Feeding { feed: 1 }, "48 is allowed");

        // Named even while a feed runs: what to change, not when to retry.
        let during = owner.ask([5; 16], &Op::Feed { portions: 0 }).await.unwrap();
        assert_eq!(during, Answer::Refused(Refusal::Portions { asked: 0 }));
        let cancel = owner.ask([6; 16], &Op::CancelFeed { feed: 1 }).await;
        assert!(matches!(cancel, Ok(Answer::Done { .. })), "{cancel:?}");
        let EventBody::Fed {
            feed, requested, ..
        } = next_fed(&mut owner).await
        else {
            unreachable!("next_fed returns a feed")
        };
        assert_eq!((feed, requested), (1, 48));
        sim.stop().await;
    });
}

/// Dispense goes through the owner a client's feed takes: one portion, by
/// the panel. A press while a feed runs starts nothing, queues nothing and
/// is recorded once for its reason; the next feed takes the next number.
#[test]
fn a_dispense_press_feeds_one_portion_and_one_during_a_feed_is_recorded_not_queued() {
    local(async {
        let flash = Flash::memory();
        let sim = start(&flash).await;
        let mut owner = open(&sim).await;
        owner.subscribe(None, false).await.unwrap();
        // The panel starts locked; a locked press would do nothing.
        sim.type_line("hold lock").unwrap();
        loop {
            let pushed = owner.next(Duration::from_secs(10)).await.unwrap();
            if let Pushed::Event(event) = pushed
                && matches!(event.body, EventBody::Lock { locked: false, .. })
            {
                break;
            }
        }

        sim.type_line("press dispense").unwrap();
        let EventBody::Fed {
            feed,
            requested,
            completed,
            outcome,
            by,
            ..
        } = next_fed(&mut owner).await
        else {
            unreachable!("next_fed returns a feed")
        };
        assert_eq!((feed, requested, completed), (1, 1, 1));
        assert_eq!((outcome, by), (FeedOutcome::Done, Source::Panel));

        sim.control(Control::StallFeeds(true));
        let answer = owner.ask([1; 16], &Op::Feed { portions: 2 }).await.unwrap();
        assert_eq!(answer, Answer::Feeding { feed: 2 });
        sim.type_line("press dispense").unwrap();
        sim.type_line("press dispense").unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        let cancel = owner.ask([2; 16], &Op::CancelFeed { feed: 2 }).await;
        assert!(matches!(cancel, Ok(Answer::Done { .. })), "{cancel:?}");

        // Event appends wait for the outputs, so the refused press's event
        // may follow the feed's result.
        let mut bodies = Vec::new();
        while let Ok(Pushed::Event(event)) = owner.next(Duration::from_millis(500)).await {
            bodies.push(event.body);
        }
        let fed: Vec<u32> = bodies
            .iter()
            .filter_map(|body| match body {
                EventBody::Fed { feed, .. } => Some(*feed),
                _ => None,
            })
            .collect();
        assert_eq!(fed, [2], "the presses queued no feed");
        let refused: Vec<&EventBody> = bodies
            .iter()
            .filter(|body| matches!(body, EventBody::DispenseRefused { .. }))
            .collect();
        assert_eq!(
            refused,
            [&EventBody::DispenseRefused {
                reason: Blocked::Feeding
            }],
            "one event for the reason"
        );

        sim.control(Control::StallFeeds(false));
        let next = owner.ask([3; 16], &Op::Feed { portions: 1 }).await.unwrap();
        assert_eq!(next, Answer::Feeding { feed: 3 });
        next_fed(&mut owner).await;
        sim.stop().await;
    });
}

#[test]
fn a_cancel_ends_the_feed_and_finds_nothing_once_it_has_ended() {
    local(async {
        let flash = Flash::memory();
        let sim = start(&flash).await;
        sim.control(Control::StallFeeds(true));
        let mut owner = open(&sim).await;
        owner.subscribe(None, false).await.unwrap();

        let answer = owner.ask([1; 16], &Op::Feed { portions: 3 }).await.unwrap();
        assert_eq!(answer, Answer::Feeding { feed: 1 });
        let cancel = owner.ask([2; 16], &Op::CancelFeed { feed: 1 }).await;
        assert!(matches!(cancel, Ok(Answer::Done { .. })), "{cancel:?}");
        let EventBody::Fed { feed, outcome, .. } = next_fed(&mut owner).await else {
            unreachable!("next_fed returns a feed")
        };
        assert_eq!((feed, outcome), (1, FeedOutcome::Cancelled));

        let late = owner
            .ask([3; 16], &Op::CancelFeed { feed: 1 })
            .await
            .unwrap();
        assert_eq!(late, Answer::Refused(Refusal::NotFound));
        sim.stop().await;
    });
}

#[test]
fn a_restart_mid_feed_resolves_it_as_uncertain_under_its_own_number() {
    local(async {
        let flash = Flash::memory();
        let sim = start(&flash).await;
        sim.control(Control::StallFeeds(true));
        let mut owner = open(&sim).await;
        let answer = owner.ask([1; 16], &Op::Feed { portions: 2 }).await.unwrap();
        assert_eq!(answer, Answer::Feeding { feed: 1 });
        assert_eq!(
            readiness(&mut owner, 2).await,
            Some(Readiness::Blocked(Blocked::Feeding))
        );

        sim.control(Control::Reboot);
        drop(owner);
        tokio::time::sleep(Duration::from_millis(200)).await;
        let mut owner = open(&sim).await;
        owner.subscribe(None, false).await.unwrap();
        let fed = next_fed(&mut owner).await;
        assert_eq!(
            fed,
            EventBody::Fed {
                feed: 1,
                requested: 2,
                completed: 0,
                outcome: FeedOutcome::Uncertain,
                reason: FeedReason::Output,
                hopper: FoodLevel::Unread,
                by: Source::Feeder,
            }
        );

        // The id the feed went out under is forgotten with the boot, so it
        // is a new feed now, under the next number: never the same one.
        let resent = owner.ask([1; 16], &Op::Feed { portions: 2 }).await.unwrap();
        assert_eq!(resent, Answer::Feeding { feed: 2 });
        assert_eq!(feeding::reserved(&flash), Some(2));
        sim.stop().await;
    });
}

/// The next `Food` event the subscription pushes.
async fn next_food(connection: &mut Connection) -> FoodLevel {
    loop {
        let pushed = connection.next(Duration::from_secs(5)).await.unwrap();
        if let Pushed::Event(event) = pushed
            && let EventBody::Food { level } = event.body
        {
            return level;
        }
    }
}

async fn food(connection: &mut Connection, id: u8) -> FoodLevel {
    match connection.ask([id; 16], &Op::Status).await.unwrap() {
        Answer::Status(status) => status.food,
        other => panic!("{other:?}"),
    }
}

/// A feed runs whatever the food beam reads, as stock's does: on an empty
/// hopper it still delivers, and its `Fed` event says the hopper read
/// empty. The level shows in the status and changes as one `Food` event
/// after the feed that read it.
#[test]
fn a_feed_on_an_empty_hopper_runs_and_records_that_it_read_empty() {
    local(async {
        let flash = Flash::memory();
        let sim = start(&flash).await;
        let mut owner = open(&sim).await;
        owner.subscribe(None, false).await.unwrap();
        assert_eq!(food(&mut owner, 1).await, FoodLevel::Unread, "no feed yet");

        let mut id = 2;
        for (line, level) in [
            (None, FoodLevel::Present),
            (Some("empty"), FoodLevel::Empty),
            (Some("fill"), FoodLevel::Present),
        ] {
            if let Some(line) = line {
                sim.type_line(line).unwrap();
            }
            let answer = owner.ask([id; 16], &Op::Feed { portions: 2 }).await;
            assert!(matches!(answer, Ok(Answer::Feeding { .. })), "{answer:?}");
            let EventBody::Fed {
                completed,
                outcome,
                hopper,
                ..
            } = next_fed(&mut owner).await
            else {
                unreachable!("next_fed returns a feed")
            };
            assert_eq!((completed, outcome, hopper), (2, FeedOutcome::Done, level));
            assert_eq!(next_food(&mut owner).await, level);
            assert_eq!(food(&mut owner, id + 1).await, level);
            id += 2;
        }
        sim.stop().await;
    });
}
