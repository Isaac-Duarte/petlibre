//! Bearer tokens over HTTP: a phone's reads and writes without a cookie or
//! an `Origin`, the tokens that stop (revoked, expired, a disabled or
//! deleted account), who manages whose, and a secret that is shown once
//! and never again.

use std::io::Write;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use wellfed_api::{
    AdoptFeeder, ClientRole, Connection, CreatedToken, ErrorCode, FeederId, NewToken, NewUser,
    Role, SESSION_COOKIE, Token, User, UserPatch,
};
use wellfed_server_client::reqwest::header::{AUTHORIZATION, COOKIE};
use wellfed_server_client::{Auth, Client, Lock, StatusCode, expect_status};
use wellfed_server_testkit::fixed::ManualClock;
use wellfed_server_testkit::{TestServer, ada_setup, feeder, local, refused, refused_for, until};
use uuid::Uuid;

/// A feeder no server has adopted: a permission is checked before the id.
const NOBODY: FeederId = FeederId(Uuid::from_u128(0x0192c5d0_0000_7000_8000_000000000000));

const PASSWORD: &str = "temporary pass";

fn key(n: u128) -> Uuid {
    Uuid::from_u128(0x0192c5d0_3333_7000_8000_000000000000 + n)
}

/// The owner's client, after setup.
async fn owner(server: &TestServer) -> (Client, User) {
    let mut client = server.client();
    let user = client.setup(&ada_setup()).await.unwrap();
    (client, user)
}

/// A new account's client, logged in.
async fn account(server: &TestServer, owner: &Client, name: &str, role: Role) -> (Client, User) {
    let new = NewUser {
        name: name.into(),
        password: PASSWORD.into(),
        role,
        expires: None,
    };
    let user = owner.create_user(&new).await.unwrap();
    let mut client = server.client();
    client
        .login(&wellfed_api::Credentials {
            name: name.into(),
            password: PASSWORD.into(),
        })
        .await
        .unwrap();
    (client, user)
}

/// A token for `client`'s user, and a client that sends only it.
async fn phone(
    server: &TestServer,
    client: &Client,
    expires: Option<jiff::Timestamp>,
) -> (Client, CreatedToken) {
    let new = NewToken {
        name: "phone".into(),
        expires,
    };
    let created = client.create_token(&new).await.unwrap();
    let phone = server
        .client()
        .with_auth(Some(Auth::Token(created.secret.clone())));
    (phone, created)
}

#[test]
fn a_token_reads_and_writes_without_an_origin_and_the_journal_names_it() {
    local(async {
        let sim = feeder::start().await;
        let server = TestServer::start().await;
        let (owner, ada) = owner(&server).await;
        let kitchen = owner
            .adopt(&AdoptFeeder {
                invite: feeder::invite(&sim, ClientRole::Server).await,
                name: "kitchen".into(),
                address: Some(sim.address.to_string()),
            })
            .await
            .unwrap()
            .id;
        until("the server connects to the feeder", || async {
            owner.feeders().await.unwrap()[0].connection == Connection::Online
        })
        .await;
        let (phone, created) = phone(&server, &owner, None).await;

        assert_eq!(phone.session().await.unwrap(), ada);
        assert_eq!(phone.feeders().await.unwrap()[0].id, kitchen);
        phone.set_lock(kitchen, Lock::Locked, key(0)).await.unwrap();

        // The bare request: a bearer token and no Origin, as a phone sends.
        let url = phone.url(&format!("/api/v1/feeders/{kitchen}/lock"));
        let bare = phone
            .http()
            .post(&url)
            .header(AUTHORIZATION, format!("Bearer {}", created.secret))
            .header("idempotency-key", key(1).to_string())
            .json(&serde_json::json!({ "locked": false }))
            .send()
            .await
            .unwrap();
        expect_status(bare, StatusCode::NO_CONTENT).await.unwrap();

        // A cookie still needs this server's Origin.
        let Some(Auth::Session(session)) = owner.auth() else {
            panic!("the owner logged in");
        };
        let cookie = owner
            .http()
            .post(&url)
            .header(COOKIE, format!("{SESSION_COOKIE}={session}"))
            .header("idempotency-key", key(2).to_string())
            .json(&serde_json::json!({ "locked": true }))
            .send()
            .await
            .unwrap();
        refused(
            expect_status(cookie, StatusCode::NO_CONTENT).await,
            StatusCode::FORBIDDEN,
            ErrorCode::CrossOrigin,
        );
        owner.set_lock(kitchen, Lock::Locked, key(3)).await.unwrap();

        let token = Some(created.token.id);
        assert_eq!(owner.command(key(0)).await.unwrap().token, token);
        assert_eq!(owner.command(key(1)).await.unwrap().token, token);
        assert_eq!(owner.command(key(3)).await.unwrap().token, None);
        server.stop().await;
    });
}

/// A request with `Authorization` is judged by it alone: a bad token
/// beside a good cookie is refused, and skips no Origin check for it.
#[tokio::test]
async fn a_bearer_header_never_falls_back_to_the_cookie() {
    let server = TestServer::start().await;
    let (owner, _) = owner(&server).await;
    let Some(Auth::Session(session)) = owner.auth() else {
        panic!("the owner logged in");
    };
    let cookie = format!("{SESSION_COOKIE}={session}");

    for authorization in ["Bearer plt_00", "Bearer", "Basic YWRhOnB3"] {
        let read = owner
            .http()
            .get(owner.url("/api/v1/session"))
            .header(COOKIE, &cookie)
            .header(AUTHORIZATION, authorization)
            .send()
            .await
            .unwrap();
        refused_for(
            authorization,
            expect_status(read, StatusCode::OK).await,
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthenticated,
        );

        let new = NewToken {
            name: "sneaky".into(),
            expires: None,
        };
        let write = owner
            .http()
            .post(owner.url("/api/v1/tokens"))
            .header(COOKIE, &cookie)
            .header(AUTHORIZATION, authorization)
            .json(&new)
            .send()
            .await
            .unwrap();
        let refused_as = if authorization.starts_with("Bearer plt") {
            ErrorCode::Unauthenticated
        } else {
            ErrorCode::CrossOrigin
        };
        let status = match refused_as {
            ErrorCode::Unauthenticated => StatusCode::UNAUTHORIZED,
            _ => StatusCode::FORBIDDEN,
        };
        refused_for(
            authorization,
            expect_status(write, StatusCode::CREATED).await,
            status,
            refused_as,
        );
    }
    assert_eq!(owner.tokens().await.unwrap(), [], "nothing was made");
    server.stop().await;
}

#[tokio::test]
async fn revoked_expired_and_disabled_tokens_are_refused() {
    let clock = ManualClock::new();
    let server = TestServer::start_with_clock(Arc::new(clock.clone())).await;
    let (owner, _) = owner(&server).await;
    let (member, bob) = account(&server, &owner, "bob", Role::Member).await;
    let now = wellfed_service::Clock::now(&clock);

    let past = NewToken {
        name: "late".into(),
        expires: Some(now),
    };
    refused(
        member.create_token(&past).await,
        StatusCode::BAD_REQUEST,
        ErrorCode::Invalid,
    );

    let (revoked, revoked_token) = phone(&server, &member, None).await;
    revoked.feeders().await.unwrap();
    member.revoke_token(revoked_token.token.id).await.unwrap();
    refused(
        revoked.feeders().await,
        StatusCode::UNAUTHORIZED,
        ErrorCode::Unauthenticated,
    );

    let (expiring, _) = phone(&server, &member, Some(now + Duration::from_secs(600))).await;
    let (lasting, _) = phone(&server, &member, None).await;
    expiring.feeders().await.unwrap();
    clock.advance(Duration::from_secs(900));
    refused(
        expiring.feeders().await,
        StatusCode::UNAUTHORIZED,
        ErrorCode::Unauthenticated,
    );
    lasting.feeders().await.unwrap();

    // The account's expiry overrides a token that has none.
    let expire = UserPatch {
        expires: Some(Some(now + Duration::from_secs(1_000))),
        ..UserPatch::default()
    };
    owner.update_user(bob.id, &expire).await.unwrap();
    lasting.feeders().await.unwrap();
    clock.advance(Duration::from_secs(200));
    refused(
        lasting.feeders().await,
        StatusCode::FORBIDDEN,
        ErrorCode::AccountDisabled,
    );

    // Extending the account brings its kept tokens back (and ends its
    // sessions, so only a new login opens one).
    let extend = UserPatch {
        expires: Some(None),
        ..UserPatch::default()
    };
    owner.update_user(bob.id, &extend).await.unwrap();
    lasting.feeders().await.unwrap();

    server.stop().await;
}

#[tokio::test]
async fn a_member_manages_their_own_tokens_and_an_owner_anyones() {
    let server = TestServer::start().await;
    let (owner, ada) = owner(&server).await;
    let (member, bob) = account(&server, &owner, "bob", Role::Member).await;
    let (viewer, vic) = account(&server, &owner, "vic", Role::Viewer).await;
    let (_, owners) = phone(&server, &owner, None).await;
    let (bobs_phone, bobs) = phone(&server, &member, None).await;
    let (_, vics) = phone(&server, &viewer, None).await;

    let ids = |tokens: Vec<Token>| -> Vec<_> { tokens.into_iter().map(|t| t.id).collect() };
    assert_eq!(ids(member.tokens().await.unwrap()), [bobs.token.id]);
    assert_eq!(ids(bobs_phone.tokens().await.unwrap()), [bobs.token.id]);
    assert_eq!(ids(viewer.tokens().await.unwrap()), [vics.token.id]);
    assert_eq!(
        ids(owner.tokens().await.unwrap()),
        [owners.token.id, bobs.token.id, vics.token.id]
    );
    let users: Vec<_> = owner
        .tokens()
        .await
        .unwrap()
        .iter()
        .map(|t| t.user)
        .collect();
    assert_eq!(users, [ada.id, bob.id, vic.id]);

    // Another user's token is not found, and stays.
    refused(
        member.revoke_token(owners.token.id).await,
        StatusCode::NOT_FOUND,
        ErrorCode::NotFound,
    );
    refused(
        viewer.revoke_token(bobs.token.id).await,
        StatusCode::NOT_FOUND,
        ErrorCode::NotFound,
    );
    bobs_phone.feeders().await.unwrap();

    owner.revoke_token(bobs.token.id).await.unwrap();
    refused(
        bobs_phone.feeders().await,
        StatusCode::UNAUTHORIZED,
        ErrorCode::Unauthenticated,
    );
    refused(
        owner.revoke_token(bobs.token.id).await,
        StatusCode::NOT_FOUND,
        ErrorCode::NotFound,
    );
    viewer.revoke_token(vics.token.id).await.unwrap();
    assert_eq!(ids(owner.tokens().await.unwrap()), [owners.token.id]);
    server.stop().await;
}

/// Only a login makes a token, so revoking a leaked one is enough.
#[tokio::test]
async fn a_token_cannot_make_another() {
    let server = TestServer::start().await;
    let (owner, _) = owner(&server).await;
    let (phone, _) = phone(&server, &owner, None).await;

    let new = NewToken {
        name: "another".into(),
        expires: None,
    };
    refused(
        phone.create_token(&new).await,
        StatusCode::FORBIDDEN,
        ErrorCode::SessionRequired,
    );
    for name in ["", "   ", "x".repeat(65).as_str()] {
        let new = NewToken {
            name: name.into(),
            expires: None,
        };
        refused_for(
            name,
            owner.create_token(&new).await,
            StatusCode::BAD_REQUEST,
            ErrorCode::Invalid,
        );
    }
    assert_eq!(owner.tokens().await.unwrap().len(), 1);
    server.stop().await;
}

/// A stolen owner's token cannot turn itself into a login: setting any
/// password and adding an account take a session, as making a token does.
#[tokio::test]
async fn a_token_cannot_set_a_password_or_add_an_account() {
    let server = TestServer::start().await;
    let (owner, ada) = owner(&server).await;
    let (_, bob) = account(&server, &owner, "bob", Role::Member).await;
    let (phone, _) = phone(&server, &owner, None).await;
    let session_required = |what, result: Result<_, _>| {
        refused_for(
            what,
            result.map(drop),
            StatusCode::FORBIDDEN,
            ErrorCode::SessionRequired,
        );
    };

    let reset = UserPatch {
        password: Some("stolen password".into()),
        ..UserPatch::default()
    };
    session_required("own reset", phone.update_user(ada.id, &reset).await);
    session_required("other's reset", phone.update_user(bob.id, &reset).await);
    let change = wellfed_api::PasswordChange {
        current: "correct horse".into(),
        password: "stolen password".into(),
    };
    session_required(
        "own change",
        phone.change_password(&change).await.map(|()| ada.clone()),
    );
    let new = NewUser {
        name: "mallory".into(),
        password: "stolen password".into(),
        role: Role::Owner,
        expires: None,
    };
    session_required("new account", phone.create_user(&new).await);

    // Nothing changed: the old passwords still log in, and no account was
    // added. A token's other owner writes still go through.
    let mut again = server.client();
    again
        .login(&wellfed_server_testkit::ada_login())
        .await
        .unwrap();
    let names: Vec<_> = owner
        .users()
        .await
        .unwrap()
        .into_iter()
        .map(|user| user.name)
        .collect();
    assert_eq!(names, ["ada", "bob"]);
    let demote = UserPatch {
        role: Some(Role::Viewer),
        ..UserPatch::default()
    };
    assert_eq!(
        phone.update_user(bob.id, &demote).await.unwrap().role,
        Role::Viewer
    );
    owner.update_user(bob.id, &reset).await.unwrap();
    server.stop().await;
}

/// A token carries its user, not a role: a role change applies to it at
/// once, and deleting the user stops it.
#[tokio::test]
async fn a_role_change_applies_to_tokens_and_deleting_the_user_stops_them() {
    let server = TestServer::start().await;
    let (owner, _) = owner(&server).await;
    let (member, bob) = account(&server, &owner, "bob", Role::Member).await;
    let (phone, _) = phone(&server, &member, None).await;

    // Allowed through to the feeder, which this server does not have.
    refused(
        phone.set_lock(NOBODY, Lock::Locked, key(10)).await,
        StatusCode::NOT_FOUND,
        ErrorCode::NotFound,
    );
    let demote = UserPatch {
        role: Some(Role::Viewer),
        ..UserPatch::default()
    };
    owner.update_user(bob.id, &demote).await.unwrap();
    refused(
        phone.set_lock(NOBODY, Lock::Locked, key(11)).await,
        StatusCode::FORBIDDEN,
        ErrorCode::Forbidden,
    );
    phone.feeders().await.unwrap();

    owner.delete_user(bob.id).await.unwrap();
    refused(
        phone.feeders().await,
        StatusCode::UNAUTHORIZED,
        ErrorCode::Unauthenticated,
    );
    assert_eq!(owner.tokens().await.unwrap(), []);
    server.stop().await;
}

/// Every log line the test writes, at every level.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl Log {
    fn text(&self) -> String {
        let bytes = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

impl Write for Log {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut bytes = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The secret is in the answer that made the token, and nowhere else: not
/// in a list, raw or typed, nor in any log line while it is made and used.
#[tokio::test]
async fn the_secret_is_shown_once_and_never_listed_or_logged() {
    let log = Log::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    // The test's runtime has one thread, which the server's tasks share.
    let _logging = tracing::subscriber::set_default(subscriber);

    let server = TestServer::start().await;
    let (owner, _) = owner(&server).await;
    let (phone, created) = phone(&server, &owner, None).await;
    let digits = created
        .secret
        .strip_prefix("plt_")
        .expect("a secret starts plt_");
    assert_eq!(digits.len(), 64);

    phone.feeders().await.unwrap();
    phone.tokens().await.unwrap();
    phone.revoke_token(created.token.id).await.unwrap();
    refused(
        phone.feeders().await,
        StatusCode::UNAUTHORIZED,
        ErrorCode::Unauthenticated,
    );
    let (_, again) = self::phone(&server, &owner, None).await;

    let listed = owner
        .request(wellfed_server_client::Method::GET, "/api/v1/tokens")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(listed.contains(&again.token.id.to_string()), "{listed}");
    for secret in [digits, again.secret.as_str()] {
        assert!(!listed.contains(secret), "listed: {listed}");
        assert!(!format!("{created:?}").contains(secret));
    }

    let written = log.text();
    assert!(
        written.contains("request"),
        "the log was captured: {written}"
    );
    for secret in [digits, &again.secret["plt_".len()..]] {
        assert!(!written.contains(secret), "logged: {written}");
    }
    server.stop().await;
}

/// A phone follows the event stream with its token.
#[tokio::test]
async fn a_token_opens_the_stream_until_it_is_revoked() {
    let server = TestServer::start().await;
    let (owner, _) = owner(&server).await;
    let (phone, created) = phone(&server, &owner, None).await;
    let stream = || {
        phone
            .request(wellfed_server_client::Method::GET, "/api/v1/stream")
            .send()
    };

    let open = stream().await.unwrap();
    assert_eq!(open.status(), StatusCode::OK);
    drop(open);

    owner.revoke_token(created.token.id).await.unwrap();
    assert_eq!(stream().await.unwrap().status(), StatusCode::UNAUTHORIZED);
    server.stop().await;
}
