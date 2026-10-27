//! Writes through the service: settings against their generation, the
//! panel lock, and a write whose answer never comes. A retry under the
//! same key is answered once, never applied twice.

use std::time::Duration;

use wellfed_api::{
    Connection as Link, EventBody, EventKind, EventQuery, Generation, SettingsPatch, Source,
    StreamEvent,
};
use wellfed_server_testkit::feeder::{self, Relay};
use wellfed_server_testkit::fixed::NOW_MS;
use wellfed_server_testkit::{TestService, WAIT, ada, config, local, owner_user, until};
use wellfed_service::{CommandError, Error, IdempotencyKey, Invalid, Resume};

fn locks() -> EventQuery {
    EventQuery {
        kind: Some(EventKind::Lock),
        ..EventQuery::default()
    }
}

#[test]
fn settings_change_against_the_current_generation_only() {
    local(async {
        let sim = feeder::start().await;
        let server = TestService::start().await;
        let id = server.adopt(&sim).await;
        server.link(id, Link::Online).await;
        let service = server.service();

        let settings = service.settings(id).await.unwrap();
        let patch = SettingsPatch {
            volume: Some(30),
            ..SettingsPatch::default()
        };
        let changed = service
            .put_settings(&ada(), id, settings.generation, patch, Some([1; 16]))
            .await
            .unwrap();
        assert_eq!(changed.volume, 30);
        assert_eq!(changed.generation, Generation(settings.generation.0 + 1));

        // A retry under the same id is answered once, not applied again.
        let retried = service
            .put_settings(&ada(), id, settings.generation, patch, Some([1; 16]))
            .await
            .unwrap();
        assert_eq!(retried, changed);

        let stale = service
            .put_settings(&ada(), id, settings.generation, patch, Some([2; 16]))
            .await;
        let Err(Error::Command(CommandError::Stale(current))) = stale else {
            panic!("{stale:?}");
        };
        assert_eq!(current, changed);

        let loud = SettingsPatch {
            volume: Some(101),
            ..SettingsPatch::default()
        };
        assert!(matches!(
            service
                .put_settings(&ada(), id, changed.generation, loud, Some([3; 16]))
                .await,
            Err(Error::Invalid(Invalid::Volume))
        ));

        // The settings change's key names that change: reused for a lock,
        // or by another user, it is refused, and nothing is sent.
        assert!(matches!(
            service.set_lock(&ada(), id, false, Some([1; 16])).await,
            Err(Error::Command(CommandError::KeyReused))
        ));
        let bob = owner_user(2, "bob");
        let louder = SettingsPatch {
            volume: Some(40),
            ..SettingsPatch::default()
        };
        assert!(matches!(
            service
                .put_settings(&bob, id, changed.generation, louder, Some([1; 16]))
                .await,
            Err(Error::Command(CommandError::KeyReused))
        ));
        let theirs = service
            .put_settings(&bob, id, changed.generation, louder, Some([4; 16]))
            .await
            .unwrap();
        assert_eq!(theirs.generation, Generation(changed.generation.0 + 1));
        server.shutdown().await;
        sim.stop().await;
    });
}

#[test]
fn a_lock_change_is_recorded_once_and_published_live() {
    local(async {
        let sim = feeder::start().await;
        let server = TestService::start().await;
        let id = server.adopt(&sim).await;
        server.link(id, Link::Online).await;
        let service = server.service();
        let mut live = service.hub().subscribe(None);
        assert!(matches!(live.resume, Resume::Replay(ref missed) if missed.is_empty()));

        let key: IdempotencyKey = [9; 16];
        service
            .set_lock(&ada(), id, false, Some(key))
            .await
            .unwrap();
        service
            .set_lock(&ada(), id, false, Some(key))
            .await
            .unwrap();

        let event = tokio::time::timeout(WAIT, async {
            loop {
                let stamped = live.live.recv().await.unwrap();
                assert_eq!(
                    stamped.id.epoch,
                    1000 * NOW_MS.unsigned_abs(),
                    "the clock's start"
                );
                if let StreamEvent::Event { feeder, event } = &stamped.event
                    && matches!(event.body, EventBody::Lock { .. })
                {
                    assert_eq!(*feeder, id);
                    return event.clone();
                }
            }
        })
        .await
        .unwrap();
        let EventBody::Lock { locked, by } = event.body else {
            unreachable!();
        };
        assert!(!locked);
        assert!(matches!(by, Source::Client { .. }));

        let locks = service.events(id, locks()).await.unwrap();
        assert_eq!(locks.events.len(), 1, "the retry did not lock twice");
        server.shutdown().await;
        sim.stop().await;
    });
}

/// The feeder stalls after a write reaches it: the caller hears `Unknown`,
/// never a failure, and its retry under the same key is answered from the
/// feeder's memory, so the lock changes once.
#[test]
fn an_unanswered_write_is_unknown_and_its_retry_applies_once() {
    local(async {
        let sim = feeder::start().await;
        let relay = Relay::start(sim.address).await;
        let mut config = config();
        config.timing.answer = Duration::from_millis(500);
        let server = TestService::start_with(config).await;
        let id = server.adopt_at(&sim, &relay.address.to_string()).await;
        server.link(id, Link::Online).await;
        let service = server.service();

        relay.pause();
        let key = Some([0x51; 16]);
        assert!(matches!(
            service.set_lock(&ada(), id, false, key).await,
            Err(Error::Command(CommandError::Unknown))
        ));
        relay.resume();
        service.set_lock(&ada(), id, false, key).await.unwrap();

        let count = || async { service.events(id, locks()).await.unwrap().events.len() };
        until("the lock is recorded", || async { count().await > 0 }).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(count().await, 1, "the retry did not lock twice");
        server.shutdown().await;
        sim.stop().await;
    });
}
