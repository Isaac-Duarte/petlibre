//! The simulator's feeding owner over the device API: a feed's number is
//! its journal id, its result is a `Fed` event, a cancel ends it, and a
//! restart in the middle of one resolves it as uncertain under its own
//! number, never as a new feed.

use std::future::Future;
use std::rc::Rc;
use std::time::Duration;

use wellfed_connect::Connection;
use wellfed_feeder_sim::feeding;
use wellfed_feeder_sim::flash::Flash;
use wellfed_feeder_sim::harness::Running;
use wellfed_feeder_sim::serve::Control;
use wellfed_model::{
    Answer, Blocked, EventBody, FeedOutcome, FeedReason, Op, Pushed, Readiness, Refusal, Source,
};
use wellfed_session::noise::Keypair;
use tokio::task::LocalSet;

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
            by: Source::Client { .. },
        } = next_fed(&mut owner).await
        else {
            panic!("a feed by the client");
        };
        assert_eq!((feed, requested, completed), (1, 2, 2));
        assert_eq!((outcome, reason), (FeedOutcome::Done, FeedReason::Done));
        assert_eq!(readiness(&mut owner, 3).await, Some(Readiness::Ready));

        let next = owner.ask([4; 16], &Op::Feed { portions: 1 }).await.unwrap();
        assert_eq!(next, Answer::Feeding { feed: 2 });
        next_fed(&mut owner).await;
        assert_eq!(feeding::reserved(&flash), Some(2));
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
