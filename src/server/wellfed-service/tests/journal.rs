//! The command journal: every write is recorded before it is sent, a
//! retry is answered from the journal whatever the feeder's ring still
//! holds, a write cut off by a restart stays unknown, and a key names one
//! request only.

use std::time::Duration;

use uuid::Uuid;
use wellfed_api::{
    CommandKind, CommandOutcome, CommandQuery, CommandState, Connection as Link, EventKind,
    EventQuery, FeederId, Role, SettingsPatch, User, UserId,
};
use wellfed_server_testkit::feeder::{self, Relay};
use wellfed_server_testkit::{TestService, ada, config, local, owner_user, until};
use wellfed_service::{CommandError, Error, IdempotencyKey, Invalid, Resource, Service};
use wellfed_store::{Journaled, NewCommand};

fn member(n: u128, name: &str) -> User {
    User {
        id: UserId(Uuid::from_u128(n)),
        name: name.into(),
        role: Role::Member,
        expires: None,
    }
}

async fn locks(service: &Service, id: FeederId) -> usize {
    let query = EventQuery {
        kind: Some(EventKind::Lock),
        limit: Some(500),
        ..EventQuery::default()
    };
    service.events(id, query).await.unwrap().events.len()
}

/// Wait until the server holds `want` lock events, then a little longer
/// to be sure no more arrive.
async fn settle_locks(service: &Service, id: FeederId, want: usize) {
    until("the lock events arrive", || async {
        locks(service, id).await >= want
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(locks(service, id).await, want);
}

fn all() -> CommandQuery {
    CommandQuery::default()
}

/// The feeder remembers its last 32 write ids. A retry that arrives after
/// 33 more writes is answered from the journal and never reaches the
/// feeder, which would otherwise run it again.
#[test]
fn a_retry_after_the_feeders_ring_moved_returns_the_first_answer() {
    local(async {
        let sim = feeder::start().await;
        let server = TestService::start().await;
        let id = server.adopt(&sim).await;
        server.link(id, Link::Online).await;
        let service = server.service();

        let first: IdempotencyKey = [0xa0; 16];
        service
            .set_lock(&ada().into(), id, true, Some(first))
            .await
            .unwrap();
        for n in 0..33_u8 {
            let locked = n % 2 == 1;
            service
                .set_lock(&ada().into(), id, locked, Some([n; 16]))
                .await
                .unwrap();
        }
        settle_locks(service, id, 34).await;

        // The panel ends unlocked: the first write, sent again, would lock
        // it and record a 35th event.
        service
            .set_lock(&ada().into(), id, true, Some(first))
            .await
            .unwrap();
        settle_locks(service, id, 34).await;

        let entry = service.command(&ada(), first).await.unwrap();
        assert_eq!(entry.state, CommandState::Answered);
        assert_eq!(entry.outcome, Some(CommandOutcome::Done));
        assert_eq!(entry.kind, CommandKind::Lock);
        assert!(entry.boot_count.is_some(), "the status read on connect");
        server.shutdown().await;
        sim.stop().await;
    });
}

/// A write in flight when the server stops is unknown after the restart,
/// never failed, and its retry under the same key goes out under the same
/// request id, which the feeder runs at most once.
#[test]
fn a_restart_between_send_and_answer_leaves_the_write_unknown() {
    local(async {
        let sim = feeder::start().await;
        let relay = Relay::start(sim.address).await;
        let server = TestService::start().await;
        let id = server.adopt_at(&sim, &relay.address.to_string()).await;
        server.link(id, Link::Online).await;

        relay.pause();
        let key: IdempotencyKey = [0x5e; 16];
        let sending = {
            let service = server.service().clone();
            tokio::spawn(async move { service.set_lock(&ada().into(), id, true, Some(key)).await })
        };
        until("the write is journalled", || async {
            let page = server.service().commands(&ada(), all()).await.unwrap();
            page.items.iter().any(|c| c.state == CommandState::Sent)
        })
        .await;

        let store = server.stop().await;
        let cut = sending.await.unwrap();
        assert!(
            matches!(cut, Err(Error::Command(CommandError::Unknown))),
            "{cut:?}"
        );
        relay.resume();

        let server = TestService::reopen(store).await;
        let service = server.service();
        let entry = service.command(&ada(), key).await.unwrap();
        assert_eq!(entry.state, CommandState::Unknown);
        assert_eq!(entry.outcome, None);
        let uncertain = CommandQuery {
            state: Some(CommandState::Unknown),
            ..CommandQuery::default()
        };
        let page = service.commands(&ada(), uncertain).await.unwrap();
        assert_eq!(page.items, [entry]);

        server.link(id, Link::Online).await;
        service
            .set_lock(&ada().into(), id, true, Some(key))
            .await
            .unwrap();
        let entry = service.command(&ada(), key).await.unwrap();
        assert_eq!(entry.state, CommandState::Answered);
        settle_locks(service, id, 1).await;
        server.shutdown().await;
        sim.stop().await;
    });
}

/// Wait until `key`'s entry is in `state`.
async fn entry_in(service: &Service, key: IdempotencyKey, state: CommandState) {
    until(&format!("the entry is {state:?}"), || async {
        service
            .command(&ada(), key)
            .await
            .is_ok_and(|entry| entry.state == state)
    })
    .await;
}

/// A retry that arrives while the first attempt is still in flight is
/// answered unknown and sends nothing; the first attempt's answer is the
/// one recorded, and the panel locks once.
#[test]
fn a_retry_while_the_first_attempt_is_in_flight_sends_nothing() {
    local(async {
        let sim = feeder::start().await;
        let relay = Relay::start(sim.address).await;
        let server = TestService::start().await;
        let id = server.adopt_at(&sim, &relay.address.to_string()).await;
        server.link(id, Link::Online).await;
        let service = server.service();

        relay.pause();
        let key: IdempotencyKey = [0x1f; 16];
        let first = {
            let service = service.clone();
            tokio::spawn(async move { service.set_lock(&ada().into(), id, true, Some(key)).await })
        };
        entry_in(service, key, CommandState::Sent).await;

        // Answered at once: a resend would wait on the stalled feeder
        // until the answer deadline, 12 s.
        let retried = tokio::time::timeout(
            Duration::from_secs(2),
            service.set_lock(&ada().into(), id, true, Some(key)),
        )
        .await
        .expect("a retry of a write in flight sends nothing and waits for nothing");
        assert!(
            matches!(retried, Err(Error::Command(CommandError::Unknown))),
            "{retried:?}"
        );
        relay.resume();
        first.await.unwrap().unwrap();

        let entry = service.command(&ada(), key).await.unwrap();
        assert_eq!(entry.state, CommandState::Answered);
        assert_eq!(entry.outcome, Some(CommandOutcome::Done));
        settle_locks(service, id, 1).await;
        server.shutdown().await;
        sim.stop().await;
    });
}

/// A caller that stops waiting mid-send does not stop the write: it is
/// sent, and its answer is recorded for a retry or a read by key.
#[test]
fn a_write_whose_caller_left_is_still_answered_in_the_journal() {
    local(async {
        let sim = feeder::start().await;
        let relay = Relay::start(sim.address).await;
        let server = TestService::start().await;
        let id = server.adopt_at(&sim, &relay.address.to_string()).await;
        server.link(id, Link::Online).await;
        let service = server.service();

        relay.pause();
        let key: IdempotencyKey = [0x2f; 16];
        let caller = {
            let service = service.clone();
            tokio::spawn(async move { service.set_lock(&ada().into(), id, true, Some(key)).await })
        };
        entry_in(service, key, CommandState::Sent).await;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        relay.resume();

        entry_in(service, key, CommandState::Answered).await;
        let entry = service.command(&ada(), key).await.unwrap();
        assert_eq!(entry.outcome, Some(CommandOutcome::Done));
        settle_locks(service, id, 1).await;
        service
            .set_lock(&ada().into(), id, true, Some(key))
            .await
            .unwrap();
        settle_locks(service, id, 1).await;
        server.shutdown().await;
        sim.stop().await;
    });
}

/// A server that dies with a write `sent` (no graceful stop records its
/// fate) finds it at the next start and marks it unknown.
#[test]
fn a_write_left_sent_by_a_crash_is_unknown_at_the_next_start() {
    local(async {
        let sim = feeder::start().await;
        let server = TestService::start().await;
        let id = server.adopt(&sim).await;
        let store = server.stop().await;

        let db = store.open().await;
        let key = [0xcc; 16];
        let journaled = db
            .journal(&NewCommand {
                key,
                user: ada().id,
                token: None,
                feeder: id,
                kind: CommandKind::Lock,
                body_hash: [0; 32],
                request_id: [0xcd; 16],
                boot_count: Some(1),
                history: None,
                sent: jiff::Timestamp::UNIX_EPOCH,
            })
            .await
            .unwrap();
        assert_eq!(journaled, Journaled::New);
        db.close().await;

        let server = TestService::open(store, config()).await;
        let entry = server.service().command(&ada(), key).await.unwrap();
        assert_eq!(entry.state, CommandState::Unknown);
        server.shutdown().await;
        sim.stop().await;
    });
}

/// A key already in the journal for another request is refused and
/// nothing is sent: another body, another kind of write, another feeder
/// or another user.
#[test]
fn a_key_reused_for_another_request_is_refused_unsent() {
    local(async {
        let sim = feeder::start().await;
        let other = feeder::start().await;
        let server = TestService::start().await;
        let id = server.adopt(&sim).await;
        let second = server.adopt(&other).await;
        server.link(id, Link::Online).await;
        server.link(second, Link::Online).await;
        let service = server.service();

        let key: IdempotencyKey = [0x77; 16];
        service
            .set_lock(&ada().into(), id, true, Some(key))
            .await
            .unwrap();
        let settings = service.settings(id).await.unwrap();
        let patch = SettingsPatch {
            volume: Some(10),
            ..SettingsPatch::default()
        };

        let reused = [
            service.set_lock(&ada().into(), id, false, Some(key)).await,
            service
                .put_settings(&ada().into(), id, settings.generation, patch, Some(key))
                .await
                .map(drop),
            service
                .set_lock(&ada().into(), second, true, Some(key))
                .await,
            service
                .set_lock(&owner_user(2, "bob").into(), id, true, Some(key))
                .await,
        ];
        for result in reused {
            assert!(
                matches!(result, Err(Error::Command(CommandError::KeyReused))),
                "{result:?}"
            );
        }

        settle_locks(service, id, 1).await;
        settle_locks(service, second, 0).await;
        assert_eq!(service.settings(id).await.unwrap(), settings);
        let page = service.commands(&ada(), all()).await.unwrap();
        assert_eq!(page.items.len(), 1, "a refused reuse is not journalled");
        server.shutdown().await;
        other.stop().await;
        sim.stop().await;
    });
}

/// A write without a key is journalled under one the server draws, and a
/// write the feeder never received (it was offline) leaves no entry, so
/// its key is free for the same write later.
#[test]
fn every_write_is_journalled_and_an_unsent_one_frees_its_key() {
    local(async {
        let sim = feeder::start().await;
        let server = TestService::start().await;
        let id = server.adopt(&sim).await;
        server.link(id, Link::Online).await;
        let service = server.service();

        service
            .set_lock(&ada().into(), id, true, None)
            .await
            .unwrap();
        service
            .set_lock(&ada().into(), id, false, None)
            .await
            .unwrap();
        let page = service.commands(&ada(), all()).await.unwrap();
        assert_eq!(page.items.len(), 2);
        assert_ne!(page.items[0].key, page.items[1].key);
        assert!(
            page.items
                .iter()
                .all(|c| c.state == CommandState::Answered && c.user == ada().id)
        );

        sim.stop().await;
        server.link(id, Link::Offline).await;
        let key: IdempotencyKey = [0x0f; 16];
        assert!(matches!(
            service.set_lock(&ada().into(), id, true, Some(key)).await,
            Err(Error::Command(CommandError::Offline))
        ));
        assert!(matches!(
            service.command(&ada(), key).await,
            Err(Error::NotFound(Resource::Command))
        ));
        assert_eq!(
            service.commands(&ada(), all()).await.unwrap().items.len(),
            2
        );
        server.shutdown().await;
    });
}

/// A member reads only their own entries, by key or listed; an owner reads
/// everyone's. Pages follow `next` to the end, and the filters narrow them.
#[test]
fn each_user_reads_their_own_entries_and_an_owner_reads_all() {
    local(async {
        let sim = feeder::start().await;
        let server = TestService::start().await;
        let id = server.adopt(&sim).await;
        server.link(id, Link::Online).await;
        let service = server.service();
        let (cy, dee) = (member(3, "cy"), member(4, "dee"));

        for n in 0..3_u8 {
            service
                .set_lock(&cy.clone().into(), id, n % 2 == 0, Some([0x30 + n; 16]))
                .await
                .unwrap();
        }
        service
            .set_lock(&dee.clone().into(), id, true, Some([0x40; 16]))
            .await
            .unwrap();
        service
            .set_lock(&ada().into(), id, false, Some([0x50; 16]))
            .await
            .unwrap();

        let theirs = service.commands(&cy, all()).await.unwrap();
        assert_eq!(theirs.items.len(), 3);
        assert!(theirs.items.iter().all(|c| c.user == cy.id));
        assert!(matches!(
            service.command(&cy, [0x40; 16]).await,
            Err(Error::NotFound(Resource::Command))
        ));
        assert_eq!(
            service.command(&dee, [0x40; 16]).await.unwrap().user,
            dee.id
        );
        assert_eq!(
            service.command(&ada(), [0x40; 16]).await.unwrap().user,
            dee.id
        );

        // An owner's pages, two at a time, oldest first.
        let mut keys = Vec::new();
        let mut after = None;
        loop {
            let page = service
                .commands(
                    &ada(),
                    CommandQuery {
                        after: after.clone(),
                        limit: Some(2),
                        ..CommandQuery::default()
                    },
                )
                .await
                .unwrap();
            assert!(page.items.len() <= 2);
            keys.extend(page.items.iter().map(|c| c.key.into_bytes()[0]));
            match page.next {
                Some(next) => after = Some(next),
                None => break,
            }
        }
        assert_eq!(keys, [0x30, 0x31, 0x32, 0x40, 0x50]);

        let filtered = CommandQuery {
            feeder: Some(id),
            state: Some(CommandState::Answered),
            ..CommandQuery::default()
        };
        assert_eq!(
            service
                .commands(&ada(), filtered)
                .await
                .unwrap()
                .items
                .len(),
            5
        );
        let unknown = CommandQuery {
            state: Some(CommandState::Unknown),
            ..CommandQuery::default()
        };
        assert!(
            service
                .commands(&ada(), unknown)
                .await
                .unwrap()
                .items
                .is_empty()
        );
        let elsewhere = CommandQuery {
            feeder: Some(FeederId(Uuid::from_u128(9))),
            ..CommandQuery::default()
        };
        assert!(
            service
                .commands(&ada(), elsewhere)
                .await
                .unwrap()
                .items
                .is_empty()
        );

        for (query, want) in [
            (Some(0), Invalid::CommandLimit),
            (Some(1001), Invalid::CommandLimit),
        ] {
            let bad = CommandQuery {
                limit: query,
                ..CommandQuery::default()
            };
            assert!(matches!(
                service.commands(&ada(), bad).await,
                Err(Error::Invalid(got)) if got == want
            ));
        }
        // Past the largest row number SQLite holds is no cursor either.
        let past = (i64::MAX.unsigned_abs() + 1).to_string();
        for after in ["x".to_owned(), "-1".to_owned(), past] {
            let bad = CommandQuery {
                after: Some(after.clone()),
                ..CommandQuery::default()
            };
            assert!(
                matches!(
                    service.commands(&ada(), bad).await,
                    Err(Error::Invalid(Invalid::CommandCursor))
                ),
                "{after}"
            );
        }
        let last = CommandQuery {
            after: Some(i64::MAX.to_string()),
            ..CommandQuery::default()
        };
        assert!(
            service
                .commands(&ada(), last)
                .await
                .unwrap()
                .items
                .is_empty()
        );
        server.shutdown().await;
        sim.stop().await;
    });
}
