//! The guards on settling a feed from the history, each pinned by a case
//! it decides: another client's feed is never the server's, a feed is
//! `unrecorded` only after a restart, once the feeder is past feeding and
//! with nothing missing from the history after it, and a lost answer is
//! not matched while another feed is in flight.

mod feeding;

use std::time::Duration;

use wellfed_api::{CommandKind, CommandOutcome, CommandState, Connection as Link, FeederId};
use wellfed_model::{Answer, Op};
use wellfed_server_testkit::feeder::{self, Control, Flash, Relay, Running};
use wellfed_server_testkit::{TestService, ada, local, until};
use wellfed_service::IdempotencyKey;
use wellfed_store::NewCommand;

use self::feeding::{done, fed_events, one_feed, quick, resolved, unknown};

/// A server that reaches `sim` through a relay, with a quick answer
/// deadline, connected.
async fn through_relay(sim: &Running) -> (TestService, Relay, FeederId) {
    let relay = Relay::start(sim.address).await;
    let server = TestService::start_with(quick()).await;
    let id = server.adopt_at(sim, &relay.address.to_string()).await;
    server.link(id, Link::Online).await;
    (server, relay, id)
}

/// The entry is still unknown, with no number, after the server has had
/// time to read what it now holds.
async fn still_unknown(server: &TestService, id: FeederId, key: IdempotencyKey) {
    server.link(id, Link::Online).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let entry = server.service().command(&ada(), key).await.unwrap();
    assert_eq!((entry.state, entry.feed), (CommandState::Unknown, None));
}

/// A feed the owner's own client asks for directly, on the simulator.
async fn direct_feed(sim: &Running) -> Answer {
    let mut owner = feeder::open_owner(sim).await;
    owner
        .ask([0x71; 16], &Op::Feed { portions: 1 })
        .await
        .unwrap()
}

/// A feed sent while the feeder's own client is feeding is refused busy,
/// and its answer is lost. The other client's `Fed` event is never the
/// server's: the entry stays unknown, and once the feeder restarts with
/// no record of the server's feed it is `unrecorded`.
#[test]
fn another_clients_feed_never_settles_the_servers() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let (server, relay, id) = through_relay(&sim).await;
        let service = server.service();

        sim.control(Control::StallFeeds(true));
        assert_eq!(direct_feed(&sim).await, Answer::Feeding { feed: 1 });
        relay.hold_answers();
        let key: IdempotencyKey = [0xe1; 16];
        assert!(unknown(&service.feed(&ada().into(), id, 1, key).await));
        sim.control(Control::StallFeeds(false));
        relay.cut();
        relay.resume();

        until("the other client's feed is synced", || async {
            fed_events(service, id).await.len() == 1
        })
        .await;
        still_unknown(&server, id, key).await;

        sim.control(Control::Reboot);
        let entry = resolved(service, key).await;
        assert_eq!(entry.outcome, Some(CommandOutcome::Unrecorded));
        assert_eq!(feeder::feeds(&flash), 1, "the other client's feed only");
        server.shutdown().await;
        sim.stop().await;
    });
}

/// A feed that never reached the feeder, which has not restarted: the
/// history may still hold its result, so it stays unknown.
#[test]
fn a_feed_stays_unknown_while_the_feeder_has_not_restarted() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let (server, relay, id) = through_relay(&sim).await;

        relay.pause();
        let key: IdempotencyKey = [0xe2; 16];
        assert!(unknown(
            &server.service().feed(&ada().into(), id, 1, key).await
        ));
        relay.cut();
        relay.resume();

        still_unknown(&server, id, key).await;
        assert_eq!(feeder::feeds(&flash), 0);
        server.shutdown().await;
        sim.stop().await;
    });
}

/// After a restart the feeder feeds for another client: until it is past
/// feeding its history may not be whole, so the server's feed stays
/// unknown, and is `unrecorded` once that feed is over.
#[test]
fn a_feed_stays_unknown_while_the_restarted_feeder_feeds() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let (server, relay, id) = through_relay(&sim).await;
        let service = server.service();

        relay.pause();
        let key: IdempotencyKey = [0xe3; 16];
        assert!(unknown(&service.feed(&ada().into(), id, 1, key).await));
        sim.control(Control::Reboot);
        tokio::time::sleep(Duration::from_millis(200)).await;
        sim.control(Control::StallFeeds(true));
        assert_eq!(direct_feed(&sim).await, Answer::Feeding { feed: 1 });
        relay.resume();

        still_unknown(&server, id, key).await;
        sim.control(Control::StallFeeds(false));
        let entry = resolved(service, key).await;
        assert_eq!(entry.outcome, Some(CommandOutcome::Unrecorded));
        assert_eq!(feeder::feeds(&flash), 1, "the other client's feed only");
        server.shutdown().await;
        sim.stop().await;
    });
}

/// A gap in the history after a lost feed may hide its `Fed` event, so a
/// restart with no record of it leaves it unknown.
#[test]
fn a_feed_stays_unknown_after_a_gap_in_the_history() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let (server, relay, id) = through_relay(&sim).await;

        relay.pause();
        let key: IdempotencyKey = [0xe4; 16];
        assert!(unknown(
            &server.service().feed(&ada().into(), id, 1, key).await
        ));
        let held = server.service().command(&ada(), key).await.unwrap();
        let store = server.store().open().await;
        let since = store.command(&key).await.unwrap().unwrap().history.unwrap();
        let next = since.seq + 1;
        store
            .record_gap(id, since.epoch, next, next, held.sent)
            .await
            .unwrap();
        store.close().await;
        sim.control(Control::Reboot);
        tokio::time::sleep(Duration::from_millis(200)).await;
        relay.resume();

        server.synced(id, next + 1).await;
        still_unknown(&server, id, key).await;
        assert_eq!(feeder::feeds(&flash), 0);
        server.shutdown().await;
        sim.stop().await;
    });
}

/// A feed whose answer was lost is not matched to a `Fed` event while
/// another feed is in flight, whose answer could claim it; once none is,
/// the event settles it.
#[test]
fn a_lost_answer_waits_while_another_feed_is_in_flight() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let (server, relay, id) = through_relay(&sim).await;
        let service = server.service();

        relay.hold_answers();
        let key: IdempotencyKey = [0xe5; 16];
        assert!(unknown(&service.feed(&ada().into(), id, 1, key).await));
        let flight: IdempotencyKey = [0xe6; 16];
        let store = server.store().open().await;
        let entry = store.command(&key).await.unwrap().unwrap();
        store
            .journal(&NewCommand {
                key: flight,
                kind: CommandKind::Feed,
                body_hash: [0xe6; 32],
                request_id: [0xe6; 16],
                user: entry.user,
                token: None,
                feeder: id,
                boot_count: entry.boot_count,
                history: entry.history,
                sent: entry.sent,
            })
            .await
            .unwrap();
        relay.cut();
        relay.resume();

        until("the feed's event is synced", || async {
            fed_events(service, id).await.len() == 1
        })
        .await;
        still_unknown(&server, id, key).await;

        store.forget_unsent(&flight).await.unwrap();
        store.close().await;
        // Any event reads the history again: a lock change makes one.
        service
            .set_lock(&ada().into(), id, true, None)
            .await
            .unwrap();
        let entry = resolved(service, key).await;
        assert_eq!(entry.outcome, Some(done(1, 1)));
        one_feed(service, id, &flash).await;
        server.shutdown().await;
        sim.stop().await;
    });
}
