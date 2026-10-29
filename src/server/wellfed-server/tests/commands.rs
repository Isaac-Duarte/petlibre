//! The command journal over HTTP: an entry by its key, the list in pages,
//! and a key reused for another request refused `422 key_reused`.

use uuid::Uuid;
use wellfed_api::{
    AdoptFeeder, ClientRole, CommandKind, CommandOutcome, CommandQuery, CommandState, Connection,
    Desiccant, ErrorCode, PREFIX, SettingsPatch,
};
use wellfed_server_client::{Lock, Method, StatusCode};
use wellfed_server_testkit::{TestServer, ada_setup, feeder, local, refused, refused_for, until};

fn key(n: u128) -> Uuid {
    Uuid::from_u128(0x0192c5d0_2222_7000_8000_000000000000 + n)
}

#[test]
fn the_journal_reads_by_key_and_in_pages() {
    local(async {
        let sim = feeder::start().await;
        let server = TestServer::start().await;
        let mut http = server.client();
        http.setup(&ada_setup()).await.unwrap();
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

        for n in 0..3 {
            let lock = if n % 2 == 0 {
                Lock::Locked
            } else {
                Lock::Unlocked
            };
            http.set_lock(kitchen, lock, key(n)).await.unwrap();
        }

        let entry = http.command(key(1)).await.unwrap();
        assert_eq!(entry.key, key(1));
        assert_eq!(entry.feeder, kitchen);
        assert_eq!(entry.kind, CommandKind::Lock);
        assert_eq!(entry.state, CommandState::Answered);
        assert_eq!(entry.outcome, Some(CommandOutcome::Done));

        // A key reused for another request: refused, and nothing is sent.
        refused(
            http.set_lock(kitchen, Lock::Locked, key(1)).await,
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::KeyReused,
        );
        // The same request again is a retry, answered from the journal.
        http.set_lock(kitchen, Lock::Unlocked, key(1))
            .await
            .unwrap();

        let first = http
            .commands(&CommandQuery {
                limit: Some(2),
                ..CommandQuery::default()
            })
            .await
            .unwrap();
        let keys: Vec<_> = first.items.iter().map(|c| c.key).collect();
        assert_eq!(keys, [key(0), key(1)]);
        let rest = http
            .commands(&CommandQuery {
                after: first.next,
                limit: Some(2),
                ..CommandQuery::default()
            })
            .await
            .unwrap();
        let keys: Vec<_> = rest.items.iter().map(|c| c.key).collect();
        assert_eq!(keys, [key(2)]);
        assert_eq!(rest.next, None);

        let uncertain = http
            .commands(&CommandQuery {
                feeder: Some(kitchen),
                state: Some(CommandState::Unknown),
                ..CommandQuery::default()
            })
            .await
            .unwrap();
        assert!(uncertain.items.is_empty());

        refused(
            http.command(key(9)).await,
            StatusCode::NOT_FOUND,
            ErrorCode::NotFound,
        );
        for query in ["limit=0", "limit=1001", "after=x", "state=lost"] {
            let response = http
                .request(Method::GET, &format!("{PREFIX}/commands?{query}"))
                .send()
                .await
                .unwrap();
            let result = wellfed_server_client::expect_status(response, StatusCode::OK).await;
            refused_for(query, result, StatusCode::BAD_REQUEST, ErrorCode::Invalid);
        }

        http.logout().await.unwrap();
        refused(
            http.commands(&CommandQuery::default()).await,
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthenticated,
        );
        refused(
            http.command(key(0)).await,
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthenticated,
        );
        server.stop().await;
        sim.stop().await;
    });
}

/// A desiccant reset through the server is journalled like any write and
/// dated by the feeder, which the server's clock sync makes valid; the
/// period is a settings field the server checks before sending.
#[test]
fn the_desiccant_reminder_resets_through_the_journal() {
    local(async {
        let sim = feeder::start().await;
        let server = TestServer::start().await;
        let mut http = server.client();
        http.setup(&ada_setup()).await.unwrap();
        let kitchen = http
            .adopt(&AdoptFeeder {
                invite: feeder::invite(&sim, ClientRole::Server).await,
                name: "kitchen".into(),
                address: Some(sim.address.to_string()),
            })
            .await
            .unwrap()
            .id;
        // Read live, so the session is open: the connect sequence records
        // a status with the clock set before the feeder shows online, and
        // that copy answers while the server is still connecting.
        until("the server sets the feeder's clock", || async {
            http.feeder(kitchen).await.is_ok_and(|detail| {
                detail
                    .status
                    .is_some_and(|s| s.live && s.status.clock.is_some())
            })
        })
        .await;

        http.reset_desiccant(kitchen, key(1)).await.unwrap();
        let entry = http.command(key(1)).await.unwrap();
        assert_eq!(entry.kind, CommandKind::Desiccant);
        assert_eq!(entry.outcome, Some(CommandOutcome::Done));
        let detail = http.feeder(kitchen).await.unwrap();
        assert_eq!(
            detail.status.map(|snapshot| snapshot.status.desiccant),
            Some(Desiccant::Remaining {
                days: 14,
                due: false
            })
        );

        let settings = http.settings(kitchen).await.unwrap();
        assert!(settings.desiccant_replaced.is_some());
        let too_long = SettingsPatch {
            desiccant_days: Some(91),
            ..SettingsPatch::default()
        };
        refused(
            http.put_settings(kitchen, settings.generation, &too_long, key(2))
                .await,
            StatusCode::BAD_REQUEST,
            ErrorCode::Invalid,
        );
        let monthly = SettingsPatch {
            desiccant_days: Some(30),
            ..SettingsPatch::default()
        };
        let changed = http
            .put_settings(kitchen, settings.generation, &monthly, key(3))
            .await
            .unwrap();
        assert_eq!(changed.desiccant_days, 30);
        server.stop().await;
        sim.stop().await;
    });
}
