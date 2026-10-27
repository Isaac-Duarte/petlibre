//! Releases over HTTP: an owner's upload verified and kept, a refused
//! file's typed reason, and a check's refusals. The fetch itself is tested
//! in `wellfed-releases` against a stand-in for GitHub.

use ed25519_dalek::SigningKey;
use wellfed_api::{
    Credentials, ErrorCode, NewUser, ReleaseQuery, ReleaseRefusal, ReleaseSigner, ReleaseSource,
    Role,
};
use wellfed_server_client::{Client, Error, StatusCode, Uploaded};
use wellfed_server_testkit::release::{self, MANIFEST_LEN, SECTOR, project, signed};
use wellfed_server_testkit::{TestServer, ada_setup, config, refused};
use wellfed_service::{Config, RELEASE_MAX, ReleaseFeed};

/// The kit's config, trusting [`project`] and fetching from `fetch`.
fn trusting(fetch: Option<ReleaseFeed>) -> Config {
    let mut config = release::trusting(config());
    config.releases.fetch = fetch;
    config
}

/// A release at `version`, signed by `key`.
fn release(key: &SigningKey, version: u32) -> Vec<u8> {
    sized(key, version, 2 * SECTOR)
}

/// A release at `version` whose image is `len` bytes.
fn sized(key: &SigningKey, version: u32, len: usize) -> Vec<u8> {
    signed(key, version, [0x3c; 32], len)
}

async fn owner(server: &TestServer) -> Client {
    let mut client = server.client();
    client.setup(&ada_setup()).await.unwrap();
    client
}

/// The refusal a `release_refused` problem names.
#[track_caller]
fn refused_as(result: Result<Uploaded, Error>, want: ReleaseRefusal) {
    match result {
        Err(Error::Problem {
            status, problem, ..
        }) => {
            assert_eq!(
                (status, problem.code, problem.release),
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    ErrorCode::ReleaseRefused,
                    Some(want)
                ),
                "{problem:?}"
            );
        }
        other => panic!("expected a refused release, got {other:?}"),
    }
}

#[tokio::test]
async fn an_owner_uploads_a_release_and_lists_it() {
    let server = TestServer::start_with(trusting(None)).await;
    let client = owner(&server).await;
    let file = release(&project(), 7);

    let Uploaded::New(release) = client.upload_release(file.clone()).await.unwrap() else {
        panic!("a first upload is new");
    };
    assert_eq!(release.version, 7);
    assert_eq!(release.signer, ReleaseSigner::Project);
    assert_eq!(release.source, ReleaseSource::Upload);

    let again = client.upload_release(file).await.unwrap();
    assert_eq!(again, Uploaded::Known(release.clone()));

    let page = client.releases(&ReleaseQuery::default()).await.unwrap();
    assert_eq!((page.items, page.next), (vec![release], None));

    server.stop().await;
}

/// Every other route takes 64 KiB at most; a release is far larger.
#[tokio::test]
async fn an_upload_may_be_a_full_bank() {
    let server = TestServer::start_with(trusting(None)).await;
    let client = owner(&server).await;
    let file = sized(&project(), 7, RELEASE_MAX - MANIFEST_LEN);
    assert_eq!(file.len(), RELEASE_MAX);

    let uploaded = client.upload_release(file).await.unwrap();

    assert!(matches!(uploaded, Uploaded::New(_)), "{uploaded:?}");
    server.stop().await;
}

#[tokio::test]
async fn releases_need_a_login() {
    let server = TestServer::start_with(trusting(None)).await;
    let anonymous = server.client();

    refused(
        anonymous.releases(&ReleaseQuery::default()).await,
        StatusCode::UNAUTHORIZED,
        ErrorCode::Unauthenticated,
    );
    refused(
        anonymous.upload_release(release(&project(), 7)).await,
        StatusCode::UNAUTHORIZED,
        ErrorCode::Unauthenticated,
    );
    refused(
        anonymous.check_releases().await,
        StatusCode::UNAUTHORIZED,
        ErrorCode::Unauthenticated,
    );

    server.stop().await;
}

#[tokio::test]
async fn a_refused_upload_says_why_and_is_not_kept() {
    let server = TestServer::start_with(trusting(None)).await;
    let client = owner(&server).await;

    let mut forged = release(&project(), 7);
    forged[MANIFEST_LEN - 1] ^= 0x01;
    refused_as(
        client.upload_release(forged).await,
        ReleaseRefusal::Signature,
    );

    let stranger = SigningKey::from_bytes(&[0x07; 32]);
    refused_as(
        client.upload_release(release(&stranger, 7)).await,
        ReleaseRefusal::UnknownKey,
    );

    let mut corrupted = release(&project(), 7);
    *corrupted.last_mut().unwrap() ^= 0x01;
    refused_as(
        client.upload_release(corrupted).await,
        ReleaseRefusal::Image,
    );

    refused(
        client.upload_release(vec![0; RELEASE_MAX + 1]).await,
        StatusCode::PAYLOAD_TOO_LARGE,
        ErrorCode::TooLarge,
    );

    let page = client.releases(&ReleaseQuery::default()).await.unwrap();
    assert!(page.items.is_empty());

    server.stop().await;
}

#[tokio::test]
async fn a_check_is_refused_while_fetching_is_off() {
    let server = TestServer::start_with(trusting(None)).await;
    let client = owner(&server).await;

    refused(
        client.check_releases().await,
        StatusCode::CONFLICT,
        ErrorCode::FetchDisabled,
    );

    server.stop().await;
}

#[tokio::test]
async fn a_check_that_cannot_reach_github_says_so() {
    // A port nothing listens on: the connection is refused at once.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = closed.local_addr().unwrap();
    drop(closed);
    let feed = ReleaseFeed {
        api: format!("http://{address}"),
        repository: "owner/wellfed".into(),
    };
    let server = TestServer::start_with(trusting(Some(feed))).await;
    let client = owner(&server).await;

    refused(
        client.check_releases().await,
        StatusCode::BAD_GATEWAY,
        ErrorCode::ReleasesUnreachable,
    );

    server.stop().await;
}

#[tokio::test]
async fn releases_are_for_owners_only() {
    let server = TestServer::start_with(trusting(None)).await;
    let owner = owner(&server).await;
    let bob = Credentials {
        name: "bob".into(),
        password: "battery staple".into(),
    };
    owner
        .create_user(&NewUser {
            name: bob.name.clone(),
            password: bob.password.clone(),
            role: Role::Member,
            expires: None,
        })
        .await
        .unwrap();
    let mut member = server.client();
    member.login(&bob).await.unwrap();

    refused(
        member.releases(&ReleaseQuery::default()).await,
        StatusCode::FORBIDDEN,
        ErrorCode::Forbidden,
    );
    refused(
        member.upload_release(release(&project(), 7)).await,
        StatusCode::FORBIDDEN,
        ErrorCode::Forbidden,
    );
    refused(
        member.check_releases().await,
        StatusCode::FORBIDDEN,
        ErrorCode::Forbidden,
    );

    server.stop().await;
}
