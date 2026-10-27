//! The mirror: history sync across the server's outages, a feeder that
//! goes away and comes back, and the clock and samples a connected feeder
//! keeps up.

use std::time::Duration;

use wellfed_api::{Connection as Link, EventBody, EventQuery, Source, StreamEvent};
use wellfed_model::{Answer, Op};
use wellfed_server_testkit::feeder::{self, Flash, open_owner};
use wellfed_server_testkit::{TestService, WAIT, ada, config, local};
use wellfed_service::Error;

#[test]
fn history_recorded_while_the_server_was_down_arrives_on_restart() {
    local(async {
        let sim = feeder::start().await;
        let server = TestService::start().await;
        let id = server.adopt(&sim).await;
        server.link(id, Link::Online).await;
        let store = server.stop().await;

        // The owner changes the lock twice while the server is away.
        let mut owner = open_owner(&sim).await;
        for (n, locked) in [(1u8, false), (2, true)] {
            let answer = owner.ask([n; 16], &Op::SetLock { locked }).await.unwrap();
            assert_eq!(answer, Answer::Done { generation: None });
        }
        let answer = owner.ask([3; 16], &Op::Status).await.unwrap();
        let Answer::Status(status) = answer else {
            panic!("{answer:?}");
        };
        let newest = status.last_event.unwrap();
        drop(owner);

        let server = TestService::reopen(store).await;
        server.link(id, Link::Online).await;
        server.synced(id, newest).await;
        let page = server
            .service()
            .events(id, EventQuery::default())
            .await
            .unwrap();
        let seqs: Vec<u64> = page.events.iter().map(|event| event.seq).collect();
        assert!(
            seqs.is_sorted() && seqs.windows(2).all(|w| w[0] < w[1]),
            "{seqs:?}"
        );
        let locks: Vec<bool> = page
            .events
            .iter()
            .filter_map(|event| match event.body {
                EventBody::Lock { locked, .. } => Some(locked),
                _ => None,
            })
            .collect();
        assert_eq!(locks, [false, true]);
        assert!(page.gaps.is_empty());
        server.shutdown().await;
        sim.stop().await;
    });
}

#[test]
fn a_feeder_that_goes_away_is_offline_until_it_returns() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let address = sim.address;
        let server = TestService::start().await;
        let id = server.adopt(&sim).await;
        server.link(id, Link::Online).await;
        let service = server.service();
        let mut updates = service.hub().subscribe(None);

        sim.stop().await;
        server.link(id, Link::Offline).await;
        assert!(matches!(
            service.set_lock(&ada(), id, false, Some([5; 16])).await,
            Err(Error::Offline)
        ));
        assert!(matches!(service.settings(id).await, Err(Error::Offline)));
        let detail = service.feeder(id).await.unwrap();
        assert!(!detail.status.unwrap().live, "the cached status, marked");

        let sim = feeder::restart(&flash, address).await;
        server.link(id, Link::Online).await;
        service
            .set_lock(&ada(), id, false, Some([5; 16]))
            .await
            .unwrap();

        let mut seen = Vec::new();
        while let Ok(stamped) = updates.live.try_recv() {
            if let StreamEvent::Connection { connection, .. } = stamped.event {
                seen.push(connection);
            }
        }
        assert!(seen.starts_with(&[Link::Offline]), "{seen:?}");
        assert_eq!(seen.last(), Some(&Link::Online));
        server.shutdown().await;
        sim.stop().await;
    });
}

#[test]
fn the_clock_is_set_again_on_schedule_and_samples_are_kept() {
    local(async {
        let sim = feeder::start().await;
        let mut config = config();
        config.timing.time_sync = Duration::from_millis(300);
        let server = TestService::start_with(config).await;
        let mut updates = server.service().hub().subscribe(None);
        let id = server.adopt(&sim).await;

        // The connect's sync, then at least two on schedule; and the
        // simulator's power sample, every 5 s.
        let (mut syncs, mut sample) = (0, None);
        tokio::time::timeout(WAIT, async {
            while syncs < 3 || sample.is_none() {
                match &updates.live.recv().await.unwrap().event {
                    StreamEvent::Event { event, .. }
                        if matches!(
                            event.body,
                            EventBody::ClockSync {
                                by: Source::Client { .. },
                                ..
                            }
                        ) =>
                    {
                        syncs += 1;
                    }
                    StreamEvent::Sample { sample: got, .. } => sample = Some(*got),
                    _ => {}
                }
            }
        })
        .await
        .expect("three clock syncs and a sample");

        let reader = server.store().open().await;
        let sample = sample.unwrap();
        let kept = reader.samples(id, sample.value.metric(), 10).await.unwrap();
        assert!(kept.iter().any(|row| row.uptime_ms == sample.uptime_ms));
        server.shutdown().await;
        sim.stop().await;
    });
}
