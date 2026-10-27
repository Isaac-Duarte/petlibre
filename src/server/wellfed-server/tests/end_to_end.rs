//! The server end to end: real HTTP in front, and behind it the service
//! speaking the device API (Noise over TCP) to the simulated feeder.

use wellfed_api::{
    AdoptFeeder, ClientRole, Connection, EventBody, EventKind, EventQuery, Settings, SettingsPatch,
    Source, StreamEvent, StreamQuery,
};
use wellfed_server_client::reqwest::header::ETAG;
use wellfed_server_client::{
    Error, Lock, Method, StatusCode, Update, Updates, decode, expect_status,
};
use wellfed_server_testkit::{TestServer, WAIT, ada_login, feeder, local, until};
use uuid::Uuid;

/// The next update, failing the test when the stream ends or breaks.
async fn next(updates: &mut Updates) -> Update {
    updates.next().await.expect("the stream is open").unwrap()
}

#[test]
fn an_owner_adopts_a_feeder_and_works_it_over_http() {
    local(async {
        let sim = feeder::start().await;
        let server = TestServer::start().await;
        let mut http = server.client();

        http.setup(&ada_login()).await.unwrap();

        let kitchen = http
            .adopt(&AdoptFeeder {
                invite: feeder::invite(&sim, ClientRole::Server).await,
                name: "kitchen".into(),
                address: Some(sim.address.to_string()),
            })
            .await
            .unwrap()
            .id;

        until("the server connects to the feeder", || async {
            http.feeders().await.unwrap()[0].connection == Connection::Online
        })
        .await;

        let power = StreamQuery {
            samples: Some("power".into()),
        };
        let mut updates = http.stream(&power, None).await.unwrap();

        let detail = http.feeder(kitchen).await.unwrap();
        let status = detail.status.unwrap();
        assert!(status.live);
        assert!(status.status.clock.is_some(), "the server set the clock");

        // Settings: an edit names its generation; a retry with the same
        // key is answered once; a stale edit gets the current settings.
        let read = http
            .request(Method::GET, &format!("/api/v1/feeders/{kitchen}/settings"))
            .send()
            .await
            .unwrap();
        let read = expect_status(read, StatusCode::OK).await.unwrap();
        let etag = read.headers()[ETAG].to_str().unwrap().to_owned();
        let settings: Settings = decode(read, StatusCode::OK).await.unwrap();
        assert_eq!(etag, format!("\"{}\"", settings.generation));
        assert_eq!(http.settings(kitchen).await.unwrap(), settings);

        let key = Uuid::from_u128(0x0192c5d0_1111_7000_8000_000000000001);
        let patch = SettingsPatch {
            volume: Some(35),
            ..SettingsPatch::default()
        };
        let changed = http
            .put_settings(kitchen, settings.generation, &patch, key)
            .await
            .unwrap();
        assert_eq!(changed.volume, 35);
        let retried = http
            .put_settings(kitchen, settings.generation, &patch, key)
            .await
            .unwrap();
        assert_eq!(retried, changed);
        let other = Uuid::from_u128(0x0192c5d0_1111_7000_8000_000000000002);
        let stale = http.put_settings(kitchen, settings.generation, &patch, other);
        let Err(Error::Stale(current)) = stale.await else {
            panic!("a stale edit is answered with the current settings");
        };
        assert_eq!(*current, changed);

        let lock = Uuid::from_u128(0x0192c5d0_1111_7000_8000_000000000003);
        http.set_lock(kitchen, Lock::Unlocked, lock).await.unwrap();

        // The lock's event and a power sample reach the stream.
        let (mut lock_seen, mut power_seen, mut first) = (None, false, None);
        tokio::time::timeout(WAIT, async {
            while !(lock_seen.is_some() && power_seen) {
                let Update { id, event } = next(&mut updates).await;
                if first.is_none() {
                    first = Some(id.clone());
                }
                match event {
                    StreamEvent::Event { event, .. }
                        if matches!(event.body, EventBody::Lock { locked: false, .. }) =>
                    {
                        lock_seen = Some(id);
                    }
                    StreamEvent::Sample { .. } => power_seen = true,
                    _ => {}
                }
            }
        })
        .await
        .expect("the lock and a sample arrive live");

        // A reconnect from the first update gets everything after it, the
        // lock among them; one from another run is told to resync.
        let first = first.unwrap();
        let mut replay = http
            .stream(&StreamQuery::default(), Some(&first))
            .await
            .unwrap();
        tokio::time::timeout(WAIT, async {
            loop {
                let Update { id, event } = next(&mut replay).await;
                assert_ne!(id, first, "replay starts after the id");
                assert!(
                    !matches!(event, StreamEvent::Sample { .. }),
                    "not asked for"
                );
                if Some(&id) == lock_seen.as_ref() {
                    return;
                }
            }
        })
        .await
        .expect("the replay includes the lock");
        let mut stale = http
            .stream(&StreamQuery::default(), Some("0000000000000001-0"))
            .await
            .unwrap();
        let Update { id, event } = next(&mut stale).await;
        assert_eq!((id.as_str(), event), ("", StreamEvent::Resync));

        let locks = EventQuery {
            kind: Some(EventKind::Lock),
            ..EventQuery::default()
        };
        let page = http.events(kitchen, &locks).await.unwrap();
        let [event] = page.events.as_slice() else {
            panic!("{page:?}");
        };
        assert!(matches!(
            event.body,
            EventBody::Lock {
                locked: false,
                by: Source::Client { .. }
            }
        ));

        server.stop().await;
        sim.stop().await;
    });
}
