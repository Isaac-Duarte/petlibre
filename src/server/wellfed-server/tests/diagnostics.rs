//! Traces, statistics and the debug log over HTTP: a feed's trace and the
//! latest statistics are anyone's to read once the mirror has fetched
//! them, and an owner follows the debug log as Server-Sent Events while the
//! server's key on the feeder is an owner's.

use std::time::Duration;

use uuid::Uuid;
use wellfed_api::{
    AdoptFeeder, ClientRole, Connection, Credentials, ErrorCode, FeederId, FeederRefusal, LogEvent,
    NewUser, Role, StatsSection, TraceQuery, TraceRecord,
};
use wellfed_server_client::{Client, StatusCode};
use wellfed_server_testkit::feeder::{self, Flash, Running};
use wellfed_server_testkit::{TestServer, WAIT, ada_setup, config, local, refused, until};
use wellfed_service::Diagnostics;

/// The test server, reading traces and statistics as a real one does.
async fn server() -> TestServer {
    let mut config = config();
    config.diagnostics = Diagnostics::Read {
        stats_every: Duration::from_secs(3600),
    };
    TestServer::start_with(config).await
}

/// Adopt `sim` with the server's key enrolled as `role`, and wait until
/// the server is connected.
async fn adopt(http: &Client, sim: &Running, role: ClientRole) -> FeederId {
    let id = http
        .adopt(&AdoptFeeder {
            invite: feeder::invite(sim, role).await,
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
    id
}

/// A logged-in client for a new account of `role`, made by `owner`.
async fn account(server: &TestServer, owner: &Client, name: &str, role: Role) -> Client {
    let password = "temporary pass".to_owned();
    owner
        .create_user(&NewUser {
            name: name.into(),
            password: password.clone(),
            role,
            expires: None,
        })
        .await
        .unwrap();
    let mut client = server.client();
    client
        .login(&Credentials {
            name: name.into(),
            password,
        })
        .await
        .unwrap();
    client
}

#[test]
fn a_feeds_trace_and_the_latest_statistics_are_served() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let server = server().await;
        let mut http = server.client();
        http.setup(&ada_setup()).await.unwrap();
        let kitchen = adopt(&http, &sim, ClientRole::Server).await;
        let viewer = account(&server, &http, "grace", Role::Viewer).await;
        let summary = feeder::store_trace(&flash, 1);

        let fed = http.feed(kitchen, 1, Uuid::from_u128(1)).await.unwrap();
        assert_eq!(fed.feed, Some(1));
        let query = TraceQuery::default();
        until("the feed's trace is served", || async {
            viewer.feed_trace(kitchen, 1, &query).await.is_ok()
        })
        .await;
        let trace = viewer.feed_trace(kitchen, 1, &query).await.unwrap();
        assert_eq!((trace.epoch, trace.feed), (sim_epoch(&http).await, 1));
        let TraceRecord::Stored { trace } = trace.record else {
            panic!("the feeder had the trace");
        };
        assert_eq!(trace.summary, summary);
        assert_eq!(trace.entries.len(), 7);
        refused(
            viewer.feed_trace(kitchen, 9, &query).await,
            StatusCode::NOT_FOUND,
            ErrorCode::NotFound,
        );

        until("every section the server may read is served", || async {
            viewer.feeder_stats(kitchen).await.unwrap().sections.len() == 6
        })
        .await;
        let stats = viewer.feeder_stats(kitchen).await.unwrap();
        assert_eq!(stats.sections[0].section, StatsSection::Counters);
        assert!(
            stats
                .sections
                .iter()
                .all(|read| read.stats.section() == read.section)
        );

        server.stop().await;
        sim.stop().await;
    });
}

/// The epoch the server holds the feeder's history in.
async fn sim_epoch(http: &Client) -> u32 {
    http.feeders().await.unwrap()[0]
        .epoch
        .expect("the server holds the history")
}

#[test]
fn an_owner_follows_the_debug_log_and_nobody_else_does() {
    local(async {
        let sim = feeder::start().await;
        let server = server().await;
        let mut http = server.client();
        http.setup(&ada_setup()).await.unwrap();
        let kitchen = adopt(&http, &sim, ClientRole::Owner).await;
        let member = account(&server, &http, "grace", Role::Member).await;

        let mut log = http.follow_log(kitchen).await.unwrap();
        let mut seen = Vec::new();
        while !seen
            .iter()
            .any(|line: &String| line.contains("op=tail-log"))
        {
            let event = tokio::time::timeout(WAIT, log.next())
                .await
                .expect("a batch in time")
                .expect("the log is open")
                .unwrap();
            if let LogEvent::Lines { lines, .. } = event {
                seen.extend(lines);
            }
        }

        refused(
            member.follow_log(kitchen).await,
            StatusCode::FORBIDDEN,
            ErrorCode::Forbidden,
        );
        refused(
            http.follow_log(FeederId(Uuid::from_u128(9))).await,
            StatusCode::NOT_FOUND,
            ErrorCode::NotFound,
        );

        drop(log);
        server.stop().await;
        sim.stop().await;
    });
}

#[test]
fn the_debug_log_is_refused_when_the_servers_key_is_not_an_owners() {
    local(async {
        let sim = feeder::start().await;
        let server = server().await;
        let mut http = server.client();
        http.setup(&ada_setup()).await.unwrap();
        let kitchen = adopt(&http, &sim, ClientRole::Server).await;

        let refused = http.follow_log(kitchen).await;
        let Err(wellfed_server_client::Error::Problem {
            status, problem, ..
        }) = refused
        else {
            panic!("the feeder refused the server's key: {refused:?}");
        };
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(problem.code, ErrorCode::FeederRefused);
        assert_eq!(problem.refusal, Some(FeederRefusal::Forbidden));

        server.stop().await;
        sim.stop().await;
    });
}
