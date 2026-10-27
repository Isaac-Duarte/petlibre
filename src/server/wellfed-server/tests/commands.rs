//! The command journal over HTTP: an entry by its key, the list in pages,
//! and a key reused for another request refused `422 key_reused`.

use wellfed_api::{
    AdoptFeeder, ClientRole, CommandKind, CommandOutcome, CommandQuery, CommandState, Connection,
    ErrorCode, PREFIX,
};
use wellfed_server_client::{Lock, Method, StatusCode};
use wellfed_server_testkit::{TestServer, ada_setup, feeder, local, refused, refused_for, until};
use uuid::Uuid;

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
