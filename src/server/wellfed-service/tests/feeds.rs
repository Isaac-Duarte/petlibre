//! Feeds through the service against the simulator's feeding owner. Every
//! test that retries a feed, or loses its answer, its session, the server
//! or the feeder's boot along the way, ends with the feeder's journal
//! holding exactly one feed (`feeder::feeds`) and the history one `Fed`
//! event: a feed is never sent twice, only settled from the history.

mod feeding;

use std::time::Duration;

use wellfed_api::{
    CommandOutcome, CommandState, Connection as Link, FeedOutcome, FeedReason, FeederRefusal,
    StreamEvent,
};
use wellfed_server_testkit::feeder::{self, Control, Flash, Relay};
use wellfed_server_testkit::{TestService, ada, local, until};
use wellfed_service::{CommandError, Error, IdempotencyKey};

use self::feeding::{done, fed_events, one_feed, quick, resolved, unknown};

/// Two submits with one key at once: one feed. The other is answered from
/// the journal, or unknown while the first is in flight, and never sent.
#[test]
fn a_double_submit_feeds_once() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let server = TestService::start().await;
        let id = server.adopt(&sim).await;
        server.link(id, Link::Online).await;
        let service = server.service();
        let mut stream = service.hub().subscribe(None).live;

        let key: IdempotencyKey = [0xd1; 16];
        let user = ada().into();
        let (first, second) = tokio::join!(
            service.feed(&user, id, 2, key),
            service.feed(&user, id, 2, key)
        );
        for result in [&first, &second] {
            let answered = result.as_ref().is_ok_and(|entry| entry.feed == Some(1));
            assert!(answered || unknown(result), "{result:?}");
        }
        assert!(first.is_ok() || second.is_ok(), "one of them sent it");

        let entry = resolved(service, key).await;
        assert_eq!(entry.outcome, Some(done(1, 2)));
        assert_eq!(entry.feed, Some(1));
        let again = service.feed(&ada().into(), id, 2, key).await.unwrap();
        assert_eq!(again, entry, "a retry returns the settled entry");

        // The result reached the stream as the entry's update.
        let settled = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let stamped = stream.recv().await.unwrap();
                if let StreamEvent::Command { command } = &stamped.event
                    && command.state == CommandState::Resolved
                {
                    return command.clone();
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(settled, entry);
        one_feed(service, id, &flash).await;
        server.shutdown().await;
        sim.stop().await;
    });
}

/// The feeder takes the feed but its answer never arrives: the feed is
/// unknown, a retry sends nothing, and the `Fed` event the server syncs
/// after reconnecting settles it.
#[test]
fn a_retry_after_a_dropped_answer_feeds_once() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let relay = Relay::start(sim.address).await;
        let server = TestService::start_with(quick()).await;
        let id = server.adopt_at(&sim, &relay.address.to_string()).await;
        server.link(id, Link::Online).await;
        let service = server.service();

        relay.hold_answers();
        let key: IdempotencyKey = [0xd2; 16];
        let lost = service.feed(&ada().into(), id, 1, key).await;
        assert!(unknown(&lost), "{lost:?}");
        let entry = service.command(&ada(), key).await.unwrap();
        assert_eq!((entry.state, entry.feed), (CommandState::Unknown, None));

        let retried = service.feed(&ada().into(), id, 1, key).await;
        assert!(
            unknown(&retried),
            "an unknown feed is never resent: {retried:?}"
        );

        relay.cut();
        relay.resume();
        let entry = resolved(service, key).await;
        assert_eq!(entry.outcome, Some(done(1, 1)));
        assert_eq!(entry.feed, Some(1));
        let retried = service.feed(&ada().into(), id, 1, key).await.unwrap();
        assert_eq!(retried, entry);
        one_feed(service, id, &flash).await;
        server.shutdown().await;
        sim.stop().await;
    });
}

/// The server stops with the feed on its way: the feed is unknown at the
/// restart, the feeder runs it with no one to answer, and the history the
/// restarted server syncs settles it.
#[test]
fn a_server_restart_mid_feed_feeds_once() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let relay = Relay::start(sim.address).await;
        let server = TestService::start().await;
        let id = server.adopt_at(&sim, &relay.address.to_string()).await;
        server.link(id, Link::Online).await;

        relay.pause();
        let key: IdempotencyKey = [0xd3; 16];
        let sending = {
            let service = server.service().clone();
            tokio::spawn(async move { service.feed(&ada().into(), id, 2, key).await })
        };
        until("the feed is journalled", || async {
            server
                .service()
                .command(&ada(), key)
                .await
                .is_ok_and(|entry| entry.state == CommandState::Sent)
        })
        .await;
        let store = server.stop().await;
        let cut = sending.await.unwrap();
        assert!(unknown(&cut), "{cut:?}");
        relay.resume();

        let server = TestService::reopen(store).await;
        let service = server.service();
        let retried = service.feed(&ada().into(), id, 2, key).await;
        let settled = retried
            .as_ref()
            .is_ok_and(|e| e.state == CommandState::Resolved);
        assert!(unknown(&retried) || settled, "{retried:?}");

        let entry = resolved(service, key).await;
        assert_eq!(entry.outcome, Some(done(1, 2)));
        one_feed(service, id, &flash).await;
        server.shutdown().await;
        sim.stop().await;
    });
}

/// The session drops while the feed runs: a retry is answered from the
/// journal, and the feed's result arrives with the next session.
#[test]
fn a_cut_session_mid_feed_feeds_once() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let relay = Relay::start(sim.address).await;
        let server = TestService::start().await;
        let id = server.adopt_at(&sim, &relay.address.to_string()).await;
        server.link(id, Link::Online).await;
        let service = server.service();

        sim.control(Control::StallFeeds(true));
        let key: IdempotencyKey = [0xd4; 16];
        let entry = service.feed(&ada().into(), id, 2, key).await.unwrap();
        assert_eq!(entry.state, CommandState::Answered);
        assert_eq!(entry.outcome, Some(CommandOutcome::Feeding { feed: 1 }));
        assert_eq!(entry.feed, Some(1));

        relay.cut();
        let retried = service.feed(&ada().into(), id, 2, key).await.unwrap();
        assert_eq!(retried, entry, "answered from the journal, not resent");
        sim.control(Control::StallFeeds(false));

        let entry = resolved(service, key).await;
        assert_eq!(entry.outcome, Some(done(1, 2)));
        one_feed(service, id, &flash).await;
        server.shutdown().await;
        sim.stop().await;
    });
}

/// The feeder restarts while the feed runs: its next boot resolves the
/// feed as uncertain under its own number, which settles the entry. The
/// feed is never sent again.
#[test]
fn a_feeder_reboot_mid_feed_feeds_once_and_is_uncertain() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let server = TestService::start().await;
        let id = server.adopt(&sim).await;
        server.link(id, Link::Online).await;
        let service = server.service();

        sim.control(Control::StallFeeds(true));
        let key: IdempotencyKey = [0xd5; 16];
        let entry = service.feed(&ada().into(), id, 2, key).await.unwrap();
        assert_eq!(entry.feed, Some(1));
        sim.control(Control::Reboot);

        let entry = resolved(service, key).await;
        assert_eq!(
            entry.outcome,
            Some(CommandOutcome::Fed {
                feed: 1,
                requested: 2,
                completed: 0,
                outcome: FeedOutcome::Uncertain,
                reason: FeedReason::Output,
            })
        );
        let retried = service.feed(&ada().into(), id, 2, key).await.unwrap();
        assert_eq!(retried, entry, "an uncertain feed is never resent");
        one_feed(service, id, &flash).await;
        server.shutdown().await;
        sim.stop().await;
    });
}

/// A feed lost before the feeder took it, then a restart: the history
/// holds no record, so the entry is `unrecorded`, and nothing is resent.
#[test]
fn a_feed_the_restarted_feeder_has_no_record_of_is_unrecorded() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let relay = Relay::start(sim.address).await;
        let server = TestService::start_with(quick()).await;
        let id = server.adopt_at(&sim, &relay.address.to_string()).await;
        server.link(id, Link::Online).await;
        let service = server.service();

        relay.pause();
        let key: IdempotencyKey = [0xd6; 16];
        let lost = service.feed(&ada().into(), id, 1, key).await;
        assert!(unknown(&lost), "{lost:?}");
        sim.control(Control::Reboot);
        tokio::time::sleep(Duration::from_millis(200)).await;
        relay.resume();

        let entry = resolved(service, key).await;
        assert_eq!(entry.outcome, Some(CommandOutcome::Unrecorded));
        assert_eq!(entry.feed, None);
        let retried = service.feed(&ada().into(), id, 1, key).await.unwrap();
        assert_eq!(retried, entry, "never resent");
        assert_eq!(feeder::feeds(&flash), 0, "the feeder never took it");
        assert!(fed_events(service, id).await.is_empty());
        server.shutdown().await;
        sim.stop().await;
    });
}

/// A cancel stops the feed, whose result is still its `Fed` event; once
/// the feed has ended a new cancel finds nothing, and the same one is
/// answered from the journal.
#[test]
fn a_cancel_stops_the_feed_through_the_journal() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let server = TestService::start().await;
        let id = server.adopt(&sim).await;
        server.link(id, Link::Online).await;
        let service = server.service();

        sim.control(Control::StallFeeds(true));
        let key: IdempotencyKey = [0xd7; 16];
        let entry = service.feed(&ada().into(), id, 3, key).await.unwrap();
        let feed = entry.feed.unwrap();
        let cancel: IdempotencyKey = [0xc1; 16];
        service
            .cancel_feed(&ada().into(), id, feed, Some(cancel))
            .await
            .unwrap();

        let entry = resolved(service, key).await;
        assert_eq!(
            entry.outcome,
            Some(CommandOutcome::Fed {
                feed,
                requested: 3,
                completed: 0,
                outcome: FeedOutcome::Cancelled,
                reason: FeedReason::Cancelled,
            })
        );
        service
            .cancel_feed(&ada().into(), id, feed, Some(cancel))
            .await
            .unwrap();
        let late = service
            .cancel_feed(&ada().into(), id, feed, Some([0xc2; 16]))
            .await;
        assert!(
            matches!(late, Err(Error::Refused(FeederRefusal::NotFound))),
            "{late:?}"
        );
        one_feed(service, id, &flash).await;
        server.shutdown().await;
        sim.stop().await;
    });
}

/// A key already used for one feed, sent with another body, is refused
/// and sends nothing; the feeder refusing a feed is final for its key.
#[test]
fn a_reused_key_and_a_refused_feed_send_nothing_more() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let server = TestService::start().await;
        let id = server.adopt(&sim).await;
        server.link(id, Link::Online).await;
        let service = server.service();

        sim.control(Control::StallFeeds(true));
        let key: IdempotencyKey = [0xd8; 16];
        service.feed(&ada().into(), id, 1, key).await.unwrap();
        let reused = service.feed(&ada().into(), id, 2, key).await;
        assert!(
            matches!(reused, Err(Error::Command(CommandError::KeyReused))),
            "{reused:?}"
        );

        let busy_key: IdempotencyKey = [0xd9; 16];
        let busy = service.feed(&ada().into(), id, 1, busy_key).await;
        assert!(
            matches!(busy, Err(Error::Refused(FeederRefusal::Busy))),
            "one feed at a time: {busy:?}"
        );
        sim.control(Control::StallFeeds(false));
        resolved(service, key).await;
        let again = service.feed(&ada().into(), id, 1, busy_key).await;
        assert!(
            matches!(again, Err(Error::Refused(FeederRefusal::Busy))),
            "a refusal is final for its key: {again:?}"
        );
        one_feed(service, id, &flash).await;
        server.shutdown().await;
        sim.stop().await;
    });
}

/// Two feeds lose their answers, and one `Fed` event comes back: either
/// could be its feed, so neither is settled by a guess. Both stay unknown,
/// and neither is resent.
#[test]
fn two_lost_answers_and_one_fed_event_stay_unknown() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let relay = Relay::start(sim.address).await;
        let server = TestService::start_with(quick()).await;
        let id = server.adopt_at(&sim, &relay.address.to_string()).await;
        server.link(id, Link::Online).await;
        let service = server.service();

        relay.hold_answers();
        let (first, second): (IdempotencyKey, IdempotencyKey) = ([0xda; 16], [0xdb; 16]);
        assert!(unknown(&service.feed(&ada().into(), id, 1, first).await));
        assert!(unknown(&service.feed(&ada().into(), id, 1, second).await));
        relay.cut();
        relay.resume();

        until("the feed's event is synced", || async {
            fed_events(service, id).await.len() == 1
        })
        .await;
        server.link(id, Link::Online).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        for key in [first, second] {
            let entry = service.command(&ada(), key).await.unwrap();
            assert_eq!((entry.state, entry.feed), (CommandState::Unknown, None));
            let retried = service.feed(&ada().into(), id, 1, key).await;
            assert!(unknown(&retried), "never resent: {retried:?}");
        }
        one_feed(service, id, &flash).await;
        server.shutdown().await;
        sim.stop().await;
    });
}
