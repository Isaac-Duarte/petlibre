//! Feeds over HTTP: the route requires its key, answers `202` with the
//! journal entry once the feeder has reserved the feed, and the result
//! reaches the stream and `GET /commands/{key}`. Two submits under one key
//! feed once, a key reused for another feed is `422`, a member may feed
//! and a viewer may not, a cancel goes through the journal, and a feed
//! whose answer is lost is settled from its `Fed` event, never resent.

use std::time::Duration;

use uuid::Uuid;
use wellfed_api::{
    AdoptFeeder, ClientRole, Command, CommandKind, CommandOutcome, CommandState, Connection,
    Credentials, ErrorCode, FeedOutcome, FeedReason, FeederId, FeederRefusal, NewUser, PREFIX,
    Role, StreamEvent, StreamQuery,
};
use wellfed_server_client::{Client, Method, StatusCode, Updates};
use wellfed_server_testkit::feeder::{self, Control, Flash, Relay, Running};
use wellfed_server_testkit::{
    TestServer, WAIT, ada_setup, config, local, refused, refused_for, until,
};

fn key(n: u128) -> Uuid {
    Uuid::from_u128(0x0192c5d0_3333_7000_8000_000000000000 + n)
}

/// A logged-in client for a new account of `role`, made by `owner`.
async fn account(server: &TestServer, owner: &Client, name: &str, role: Role) -> Client {
    let password = "temporary pass".to_owned();
    let new = NewUser {
        name: name.into(),
        password: password.clone(),
        role,
        expires: None,
    };
    owner.create_user(&new).await.unwrap();
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

/// Adopt `sim` over HTTP and wait until the server is connected.
async fn adopt(http: &Client, sim: &Running) -> FeederId {
    let id = http
        .adopt(&AdoptFeeder {
            invite: feeder::invite(sim, ClientRole::Server).await,
            name: "kitchen".into(),
            address: Some(sim.address.to_string()),
        })
        .await
        .unwrap()
        .id;
    online(http).await;
    id
}

async fn online(http: &Client) {
    until("the server connects to the feeder", || async {
        http.feeders().await.unwrap()[0].connection == Connection::Online
    })
    .await;
}

/// The next journal entry the stream reports resolved.
async fn next_resolved(updates: &mut Updates) -> Command {
    loop {
        let update = updates.next().await.expect("the stream is open").unwrap();
        if let StreamEvent::Command { command } = update.event
            && command.state == CommandState::Resolved
        {
            return command;
        }
    }
}

#[test]
fn a_feed_is_answered_with_its_number_and_its_result_streams() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let server = TestServer::start().await;
        let mut http = server.client();
        http.setup(&ada_setup()).await.unwrap();
        let kitchen = adopt(&http, &sim).await;
        let mut updates = http.stream(&StreamQuery::default(), None).await.unwrap();

        // Two submits under one key at once: one feed.
        let (first, second) =
            tokio::join!(http.feed(kitchen, 2, key(1)), http.feed(kitchen, 2, key(1)));
        for result in [first, second] {
            match result {
                Ok(entry) => {
                    assert_eq!((entry.key, entry.kind), (key(1), CommandKind::Feed));
                    assert_eq!(entry.feed, Some(1));
                }
                unknown => refused(
                    unknown,
                    StatusCode::GATEWAY_TIMEOUT,
                    ErrorCode::OutcomeUnknown,
                ),
            }
        }

        let settled = tokio::time::timeout(WAIT, next_resolved(&mut updates))
            .await
            .expect("the feed's result reaches the stream");
        let fed = CommandOutcome::Fed {
            feed: 1,
            requested: 2,
            completed: 2,
            outcome: FeedOutcome::Done,
            reason: FeedReason::Done,
        };
        assert_eq!((settled.key, settled.outcome.clone()), (key(1), Some(fed)));
        assert_eq!(http.command(key(1)).await.unwrap(), settled);
        assert_eq!(http.feed(kitchen, 2, key(1)).await.unwrap(), settled);

        // The same key for another feed: refused, and nothing is sent.
        refused(
            http.feed(kitchen, 3, key(1)).await,
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::KeyReused,
        );
        assert_eq!(feeder::feeds(&flash), 1);
        server.stop().await;
        sim.stop().await;
    });
}

#[test]
fn a_feed_needs_its_key_and_a_well_formed_body() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let server = TestServer::start().await;
        let mut http = server.client();
        http.setup(&ada_setup()).await.unwrap();
        let kitchen = adopt(&http, &sim).await;

        let path = format!("{PREFIX}/feeders/{kitchen}/feed");
        let requests = [
            ("no key", None, r#"{"portions":1}"#),
            ("a malformed key", Some("not-a-uuid"), r#"{"portions":1}"#),
            (
                "no portions",
                Some("0192c5d0-3333-7000-8000-0000000000aa"),
                "{}",
            ),
        ];
        for (what, key, body) in requests {
            let mut request = http
                .request(Method::POST, &path)
                .header("content-type", "application/json")
                .body(body);
            if let Some(key) = key {
                request = request.header("Idempotency-Key", key);
            }
            let response = request.send().await.unwrap();
            let result = wellfed_server_client::expect_status(response, StatusCode::ACCEPTED).await;
            refused_for(what, result, StatusCode::BAD_REQUEST, ErrorCode::Invalid);
        }
        assert_eq!(feeder::feeds(&flash), 0, "nothing reached the feeder");
        server.stop().await;
        sim.stop().await;
    });
}

#[test]
fn a_member_feeds_and_cancels_through_the_journal_and_a_viewer_may_not() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let server = TestServer::start().await;
        let mut owner = server.client();
        owner.setup(&ada_setup()).await.unwrap();
        let kitchen = adopt(&owner, &sim).await;
        let http = account(&server, &owner, "bob", Role::Member).await;
        let viewer = account(&server, &owner, "vic", Role::Viewer).await;

        refused(
            viewer.feed(kitchen, 1, key(5)).await,
            StatusCode::FORBIDDEN,
            ErrorCode::Forbidden,
        );
        refused(
            viewer.cancel_feed(kitchen, 1, key(6)).await,
            StatusCode::FORBIDDEN,
            ErrorCode::Forbidden,
        );

        sim.control(Control::StallFeeds(true));
        let entry = http.feed(kitchen, 3, key(2)).await.unwrap();
        assert_eq!(entry.state, CommandState::Answered);
        assert_eq!(entry.outcome, Some(CommandOutcome::Feeding { feed: 1 }));
        http.cancel_feed(kitchen, 1, key(3)).await.unwrap();
        http.cancel_feed(kitchen, 1, key(3)).await.unwrap();

        until("the cancelled feed is settled", || async {
            http.command(key(2))
                .await
                .is_ok_and(|entry| entry.state == CommandState::Resolved)
        })
        .await;
        let entry = http.command(key(2)).await.unwrap();
        let Some(CommandOutcome::Fed { outcome, .. }) = entry.outcome else {
            panic!("{entry:?}");
        };
        assert_eq!(outcome, FeedOutcome::Cancelled);
        let cancel = http.command(key(3)).await.unwrap();
        assert_eq!(cancel.kind, CommandKind::Cancel);
        assert_eq!(cancel.outcome, Some(CommandOutcome::Done));

        let late = http.cancel_feed(kitchen, 1, key(4)).await;
        refused(late, StatusCode::CONFLICT, ErrorCode::FeederRefused);
        let late = http.command(key(4)).await.unwrap();
        assert_eq!(
            late.outcome,
            Some(CommandOutcome::Refused {
                refusal: FeederRefusal::NotFound
            })
        );
        assert_eq!(feeder::feeds(&flash), 1);
        server.stop().await;
        sim.stop().await;
    });
}

/// The server's half of the two-path rule over HTTP: a feed whose answer
/// is lost is answered `outcome_unknown`, a retry under its key sends
/// nothing, and the feed is settled from its `Fed` event, readable by key
/// and reported on the stream, so a client resolves it without resending
/// it on either path.
#[test]
fn a_feed_whose_answer_is_lost_is_unknown_never_resent_and_settled_from_its_fed_event() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let relay = Relay::start(sim.address).await;
        let mut config = config();
        config.timing.answer = Duration::from_secs(2);
        let server = TestServer::start_with(config).await;
        let mut http = server.client();
        http.setup(&ada_setup()).await.unwrap();
        let kitchen = http
            .adopt(&AdoptFeeder {
                invite: feeder::invite(&sim, ClientRole::Server).await,
                name: "kitchen".into(),
                address: Some(relay.address.to_string()),
            })
            .await
            .unwrap()
            .id;
        online(&http).await;
        let mut updates = http.stream(&StreamQuery::default(), None).await.unwrap();

        relay.hold_answers();
        refused(
            http.feed(kitchen, 1, key(7)).await,
            StatusCode::GATEWAY_TIMEOUT,
            ErrorCode::OutcomeUnknown,
        );
        let entry = http.command(key(7)).await.unwrap();
        assert_eq!((entry.state, entry.feed), (CommandState::Unknown, None));
        refused_for(
            "a retry of the unknown feed",
            http.feed(kitchen, 1, key(7)).await,
            StatusCode::GATEWAY_TIMEOUT,
            ErrorCode::OutcomeUnknown,
        );

        relay.cut();
        relay.resume();
        let settled = tokio::time::timeout(WAIT, next_resolved(&mut updates))
            .await
            .expect("the feed's settlement reaches the stream");
        let fed = CommandOutcome::Fed {
            feed: 1,
            requested: 1,
            completed: 1,
            outcome: FeedOutcome::Done,
            reason: FeedReason::Done,
        };
        assert_eq!((settled.key, settled.outcome.clone()), (key(7), Some(fed)));
        assert_eq!(http.command(key(7)).await.unwrap(), settled);
        assert_eq!(http.feed(kitchen, 1, key(7)).await.unwrap(), settled);
        assert_eq!(feeder::feeds(&flash), 1, "fed once");
        server.stop().await;
        sim.stop().await;
    });
}
