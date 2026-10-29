//! Accounts and the household over HTTP: who may do what, the refusals'
//! codes, and the household's settings.

use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;
use wellfed_api::{
    AdoptFeeder, Credentials, ErrorCode, FeederId, Generation, HouseholdPatch, NewUser, PLANS,
    PasswordChange, PlansChange, Role, SettingsPatch, Units, User, UserPatch,
};
use wellfed_server_client::{Auth, Client, Lock, StatusCode};
use wellfed_server_testkit::fixed::ManualClock;
use wellfed_server_testkit::{TestServer, ZONE, ada_setup, refused, refused_for};

/// A feeder no server has adopted: a permission is checked before the id.
const NOBODY: FeederId = FeederId(Uuid::from_u128(0x0192c5d0_0000_7000_8000_000000000000));

const PASSWORD: &str = "temporary pass";

/// The owner's client, after setup.
async fn owner(server: &TestServer) -> (Client, User) {
    let mut client = server.client();
    let user = client.setup(&ada_setup()).await.unwrap();
    (client, user)
}

/// A new account's client, logged in.
async fn account(
    server: &TestServer,
    owner: &Client,
    name: &str,
    role: Role,
    expires: Option<jiff::Timestamp>,
) -> (Client, User) {
    let new = NewUser {
        name: name.into(),
        password: PASSWORD.into(),
        role,
        expires,
    };
    let user = owner.create_user(&new).await.unwrap();
    let mut client = server.client();
    client.login(&credentials(name)).await.unwrap();
    (client, user)
}

fn credentials(name: &str) -> Credentials {
    Credentials {
        name: name.into(),
        password: PASSWORD.into(),
    }
}

fn key() -> Uuid {
    Uuid::from_u128(0x0192c5d0_1111_7000_8000_000000000003)
}

#[tokio::test]
async fn a_viewer_reads_but_every_write_is_refused() {
    let server = TestServer::start().await;
    let (owner, _) = owner(&server).await;
    let (viewer, _) = account(&server, &owner, "vic", Role::Viewer, None).await;

    viewer.feeders().await.unwrap();
    assert_eq!(viewer.household().await.unwrap().zone, ZONE);
    assert_eq!(viewer.session().await.unwrap().role, Role::Viewer);

    let forbidden = StatusCode::FORBIDDEN;
    let code = ErrorCode::Forbidden;
    let patch = SettingsPatch::default();
    refused_for(
        "settings",
        viewer
            .put_settings(NOBODY, Generation(1), &patch, key())
            .await,
        forbidden,
        code,
    );
    refused_for(
        "lock",
        viewer.set_lock(NOBODY, Lock::Locked, key()).await,
        forbidden,
        code,
    );
    let empty = PlansChange {
        plans: [None; PLANS],
    };
    refused_for(
        "plans",
        viewer.put_plans(NOBODY, Generation(1), &empty, key()).await,
        forbidden,
        code,
    );
    // Reading is allowed through to the feeder, which this server does not
    // have.
    refused_for(
        "read plans",
        viewer.plans(NOBODY).await,
        StatusCode::NOT_FOUND,
        ErrorCode::NotFound,
    );
    let adopt = AdoptFeeder {
        invite: "wellfed-invite:1:x".into(),
        name: "kitchen".into(),
        address: None,
    };
    refused_for("adopt", viewer.adopt(&adopt).await, forbidden, code);
    refused_for("users", viewer.users().await, forbidden, code);
    let new = NewUser {
        name: "eve".into(),
        password: PASSWORD.into(),
        role: Role::Owner,
        expires: None,
    };
    refused_for("create", viewer.create_user(&new).await, forbidden, code);
    let household = HouseholdPatch {
        units: Some(Units::Ounces),
        ..HouseholdPatch::default()
    };
    refused_for(
        "household",
        viewer.update_household(&household).await,
        forbidden,
        code,
    );
    server.stop().await;
}

#[tokio::test]
async fn a_member_controls_feeders_but_not_accounts_or_the_household() {
    let server = TestServer::start().await;
    let (owner, ada) = owner(&server).await;
    let (member, _) = account(&server, &owner, "bob", Role::Member, None).await;

    // Allowed through to the feeder, which this server does not have.
    let patch = SettingsPatch::default();
    refused(
        member
            .put_settings(NOBODY, Generation(1), &patch, key())
            .await,
        StatusCode::NOT_FOUND,
        ErrorCode::NotFound,
    );

    let forbidden = (StatusCode::FORBIDDEN, ErrorCode::Forbidden);
    refused_for("users", member.users().await, forbidden.0, forbidden.1);
    let promote = UserPatch {
        role: Some(Role::Owner),
        ..UserPatch::default()
    };
    refused_for(
        "promote",
        member.update_user(ada.id, &promote).await,
        forbidden.0,
        forbidden.1,
    );
    refused_for(
        "household",
        member.update_household(&HouseholdPatch::default()).await,
        forbidden.0,
        forbidden.1,
    );
    server.stop().await;
}

#[tokio::test]
async fn owners_manage_accounts_with_typed_refusals() {
    let server = TestServer::start().await;
    let (owner, ada) = owner(&server).await;
    let (member, bob) = account(&server, &owner, "bob", Role::Member, None).await;

    let taken = NewUser {
        name: "bob".into(),
        password: PASSWORD.into(),
        role: Role::Viewer,
        expires: None,
    };
    refused(
        owner.create_user(&taken).await,
        StatusCode::CONFLICT,
        ErrorCode::NameTaken,
    );
    let short = NewUser {
        name: "cy".into(),
        password: "nine char".into(),
        ..taken
    };
    refused(
        owner.create_user(&short).await,
        StatusCode::BAD_REQUEST,
        ErrorCode::Invalid,
    );

    let demote = UserPatch {
        role: Some(Role::Member),
        ..UserPatch::default()
    };
    refused(
        owner.update_user(ada.id, &demote).await,
        StatusCode::CONFLICT,
        ErrorCode::LastOwner,
    );
    refused(
        owner.delete_user(ada.id).await,
        StatusCode::CONFLICT,
        ErrorCode::LastOwner,
    );

    let viewer = UserPatch {
        role: Some(Role::Viewer),
        ..UserPatch::default()
    };
    let bob = owner.update_user(bob.id, &viewer).await.unwrap();
    assert_eq!(bob.role, Role::Viewer);
    assert_eq!(member.session().await.unwrap().role, Role::Viewer);
    assert_eq!(owner.users().await.unwrap(), [ada, bob.clone()]);

    owner.delete_user(bob.id).await.unwrap();
    refused(
        member.session().await,
        StatusCode::UNAUTHORIZED,
        ErrorCode::Unauthenticated,
    );
    refused(
        owner.delete_user(bob.id).await,
        StatusCode::NOT_FOUND,
        ErrorCode::NotFound,
    );
    server.stop().await;
}

#[tokio::test]
async fn an_expired_account_is_refused_as_disabled() {
    let clock = ManualClock::new();
    let server = TestServer::start_with_clock(Arc::new(clock.clone())).await;
    let (owner, _) = owner(&server).await;
    let now = wellfed_service::Clock::now(&clock);
    let expires = now + Duration::from_secs(600);
    let (sitter, _) = account(&server, &owner, "sitter", Role::Member, Some(expires)).await;
    sitter.feeders().await.unwrap();

    clock.advance(Duration::from_secs(900));
    refused(
        sitter.feeders().await,
        StatusCode::FORBIDDEN,
        ErrorCode::AccountDisabled,
    );
    refused(
        server.client().login(&credentials("sitter")).await,
        StatusCode::FORBIDDEN,
        ErrorCode::AccountDisabled,
    );
    server.stop().await;
}

#[tokio::test]
async fn setup_takes_the_zone_and_an_owner_changes_the_household() {
    let server = TestServer::start().await;
    let state = server.client().setup_state().await.unwrap();
    assert_eq!(state.zone.as_deref(), Some("UTC"), "the host's, to offer");

    let mut nowhere = ada_setup();
    nowhere.zone = "Mars/Olympus".into();
    refused(
        server.client().setup(&nowhere).await,
        StatusCode::BAD_REQUEST,
        ErrorCode::Invalid,
    );
    let (owner, _) = owner(&server).await;
    assert_eq!(owner.household().await.unwrap().zone, ZONE);

    let patch = HouseholdPatch {
        units: Some(Units::Ounces),
        zone: Some("Asia/Kolkata".into()),
        ..HouseholdPatch::default()
    };
    let household = owner.update_household(&patch).await.unwrap();
    assert_eq!(
        (household.units, household.zone.as_str()),
        (Units::Ounces, "Asia/Kolkata")
    );
    assert_eq!(owner.household().await.unwrap(), household);
    server.stop().await;
}

#[tokio::test]
async fn an_anonymous_client_is_unauthenticated_everywhere() {
    let server = TestServer::start().await;
    owner(&server).await;
    let anonymous = server.client().with_auth(None::<Auth>);

    let unauthenticated = (StatusCode::UNAUTHORIZED, ErrorCode::Unauthenticated);
    refused_for(
        "household",
        anonymous.household().await,
        unauthenticated.0,
        unauthenticated.1,
    );
    refused_for(
        "users",
        anonymous.users().await,
        unauthenticated.0,
        unauthenticated.1,
    );
    server.stop().await;
}

#[tokio::test]
async fn a_viewer_changes_their_own_password_and_stays_logged_in() {
    let server = TestServer::start().await;
    let (owner, _) = owner(&server).await;
    let (viewer, _) = account(&server, &owner, "vic", Role::Viewer, None).await;
    let mut elsewhere = server.client();
    elsewhere.login(&credentials("vic")).await.unwrap();

    let wrong = PasswordChange {
        current: "not the password".into(),
        password: "my own secret".into(),
    };
    refused(
        viewer.change_password(&wrong).await,
        StatusCode::UNAUTHORIZED,
        ErrorCode::BadCredentials,
    );
    let change = PasswordChange {
        current: PASSWORD.into(),
        password: "my own secret".into(),
    };
    viewer.change_password(&change).await.unwrap();

    assert_eq!(viewer.session().await.unwrap().name, "vic");
    refused(
        elsewhere.session().await,
        StatusCode::UNAUTHORIZED,
        ErrorCode::Unauthenticated,
    );
    refused(
        server.client().login(&credentials("vic")).await,
        StatusCode::UNAUTHORIZED,
        ErrorCode::BadCredentials,
    );
    server.stop().await;
}
