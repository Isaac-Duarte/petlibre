//! Installs over HTTP, against the simulator's update path: an owner's
//! install runs to activation with its progress on the stream while the
//! feeder keeps answering, a second install waits its turn as a refusal,
//! and only an owner installs, only where the server's key is an owner's.

use std::cell::RefCell;
use std::rc::Rc;

use uuid::Uuid;
use wellfed_api::{
    AdoptFeeder, ClientRole, Connection, Credentials, ErrorCode, FeederId, FeederRefusal, Hex,
    Install, InstallState, InstallTarget, NewUser, Release, Role, StreamEvent, StreamQuery,
};
use wellfed_server_client::{Client, Error, Lock, StatusCode, Uploaded};
use wellfed_server_testkit::feeder::{self, Banks, Running};
use wellfed_server_testkit::release::{self, SECTOR, project, signed};
use wellfed_server_testkit::{TestServer, WAIT, ada_setup, config, local, refused, until};

const BUILD: [u8; 32] = [0x77; 32];
const LEN: usize = 16 * SECTOR;

/// An owner's server with an updatable simulator adopted as `role`, online,
/// and a release of [`BUILD`] uploaded.
struct Rig {
    sim: Running,
    banks: Rc<RefCell<Banks>>,
    server: TestServer,
    http: Client,
    feeder: FeederId,
    release: Release,
}

impl Rig {
    async fn start(role: ClientRole) -> Self {
        let banks = Banks::new();
        let sim = feeder::start_updatable(&banks).await;
        let server = TestServer::start_with(release::trusting(config())).await;
        let mut http = server.client();
        http.setup(&ada_setup()).await.unwrap();
        let feeder = http
            .adopt(&AdoptFeeder {
                invite: feeder::invite(&sim, role).await,
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
        let file = signed(&project(), 7, BUILD, LEN);
        let Uploaded::New(release) = http.upload_release(file).await.unwrap() else {
            panic!("a first upload is new");
        };

        Self {
            sim,
            banks,
            server,
            http,
            feeder,
            release,
        }
    }

    /// Wait until the install is no longer running, and return its end.
    async fn ended(&self) -> Install {
        let ended = tokio::time::timeout(4 * WAIT, async {
            loop {
                let install = self.http.install_state(self.feeder).await.unwrap();
                if install.state != InstallState::Running {
                    return install;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        });
        ended.await.expect("the install ends in time")
    }

    async fn stop(self) {
        self.server.stop().await;
        self.sim.stop().await;
    }
}

#[test]
fn an_owner_installs_a_release_with_its_progress_on_the_stream() {
    local(async {
        let rig = Rig::start(ClientRole::Owner).await;
        let mut updates = rig
            .http
            .stream(&StreamQuery::default(), None)
            .await
            .unwrap();

        let started = rig.http.install(rig.feeder, rig.release.id).await.unwrap();
        assert_eq!(started.state, InstallState::Running);
        assert_eq!(
            (started.feeder, started.target, started.version),
            (
                rig.feeder,
                InstallTarget::Release {
                    release: rig.release.id
                },
                7
            )
        );
        assert_eq!(
            (started.written, started.image_len),
            (0, rig.release.image_len)
        );

        // The stream carries the start, the progress and the end, in order.
        let mut seen = Vec::new();
        loop {
            let update = tokio::time::timeout(4 * WAIT, updates.next())
                .await
                .expect("an update in time")
                .expect("the stream is open")
                .unwrap();
            let StreamEvent::Install { feeder, install } = update.event else {
                continue;
            };
            assert_eq!((feeder, install.id), (rig.feeder, started.id));
            seen.push((install.state, install.written));
            if install.state != InstallState::Running {
                break;
            }
        }
        assert_eq!(seen.first(), Some(&(InstallState::Running, 0)));
        assert_eq!(
            seen.last(),
            Some(&(InstallState::Activated, rig.release.image_len))
        );
        assert!(
            seen.iter()
                .any(|&(state, written)| state == InstallState::Running && written > 0),
            "progress between: {seen:?}"
        );

        let ended = rig.http.install_state(rig.feeder).await.unwrap();
        assert_eq!(ended.state, InstallState::Activated);
        assert_eq!(ended.failure, None);
        assert!(ended.finished.is_some());
        until("the feeder restarts into the release", || async {
            rig.banks.borrow().bank1_build == BUILD
        })
        .await;
        rig.stop().await;
    });
}

/// Installing the build the feeder already runs sends nothing and says so,
/// rather than reading as an activation or a refusal.
#[test]
fn installing_the_running_build_ends_already_running() {
    local(async {
        let rig = Rig::start(ClientRole::Owner).await;
        rig.http.install(rig.feeder, rig.release.id).await.unwrap();
        assert_eq!(rig.ended().await.state, InstallState::Activated);
        until("the feeder answers from the release", || async {
            let detail = rig.http.feeder(rig.feeder).await;
            let status = detail.ok().and_then(|detail| detail.status);
            status.is_some_and(|status| status.live && status.status.bank == 1)
        })
        .await;

        let again = rig.http.install(rig.feeder, rig.release.id).await.unwrap();
        let ended = rig.ended().await;

        assert_eq!(ended.id, again.id);
        assert_eq!(
            (ended.state, ended.written, ended.failure),
            (InstallState::AlreadyRunning, 0, None)
        );
        assert!(ended.finished.is_some());
        rig.stop().await;
    });
}

/// A status read and a lock write go through while the transfer runs, each
/// waiting at most one chunk, and the install still activates.
#[test]
fn status_and_writes_are_answered_during_the_transfer() {
    local(async {
        let rig = Rig::start(ClientRole::Owner).await;
        rig.http.install(rig.feeder, rig.release.id).await.unwrap();
        until("the transfer is under way", || async {
            rig.http.install_state(rig.feeder).await.unwrap().written > 0
        })
        .await;

        let detail = rig.http.feeder(rig.feeder).await.unwrap();
        assert!(
            detail.status.as_ref().is_some_and(|status| status.live),
            "{detail:?}"
        );
        rig.http
            .set_lock(rig.feeder, Lock::Locked, Uuid::from_u128(1))
            .await
            .unwrap();
        let during = rig.http.install_state(rig.feeder).await.unwrap();
        assert_eq!(during.state, InstallState::Running);
        assert!(during.written < during.image_len, "{during:?}");

        assert_eq!(rig.ended().await.state, InstallState::Activated);
        rig.stop().await;
    });
}

#[test]
fn a_second_install_is_refused_while_one_runs() {
    local(async {
        let rig = Rig::start(ClientRole::Owner).await;
        let first = rig.http.install(rig.feeder, rig.release.id).await.unwrap();

        refused(
            rig.http.install(rig.feeder, rig.release.id).await,
            StatusCode::CONFLICT,
            ErrorCode::InstallRunning,
        );

        let ended = rig.ended().await;
        assert_eq!((ended.id, ended.state), (first.id, InstallState::Activated));
        rig.stop().await;
    });
}

#[test]
fn only_an_owner_installs_and_anyone_reads_it() {
    local(async {
        let rig = Rig::start(ClientRole::Owner).await;
        let bob = Credentials {
            name: "bob".into(),
            password: "battery staple".into(),
        };
        rig.http
            .create_user(&NewUser {
                name: bob.name.clone(),
                password: bob.password.clone(),
                role: Role::Member,
                expires: None,
            })
            .await
            .unwrap();
        let mut member = rig.server.client();
        member.login(&bob).await.unwrap();

        refused(
            member.install(rig.feeder, rig.release.id).await,
            StatusCode::FORBIDDEN,
            ErrorCode::Forbidden,
        );
        refused(
            member.install_state(rig.feeder).await,
            StatusCode::NOT_FOUND,
            ErrorCode::NotFound,
        );
        refused(
            rig.server
                .client()
                .install(rig.feeder, rig.release.id)
                .await,
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthenticated,
        );

        let started = rig.http.install(rig.feeder, rig.release.id).await.unwrap();
        let read = member.install_state(rig.feeder).await.unwrap();
        assert_eq!(read.id, started.id);
        rig.ended().await;
        rig.stop().await;
    });
}

/// A feeder refuses updates from the `server` role, so an install there is
/// refused before anything is sent, and none is recorded.
#[test]
fn a_feeder_adopted_for_the_server_role_is_refused() {
    local(async {
        let rig = Rig::start(ClientRole::Server).await;

        match rig.http.install(rig.feeder, rig.release.id).await {
            Err(Error::Problem {
                status, problem, ..
            }) => {
                assert_eq!(status, StatusCode::CONFLICT);
                assert_eq!(problem.code, ErrorCode::FeederRefused);
                assert_eq!(problem.refusal, Some(FeederRefusal::Forbidden));
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        refused(
            rig.http.install_state(rig.feeder).await,
            StatusCode::NOT_FOUND,
            ErrorCode::NotFound,
        );
        rig.stop().await;
    });
}

#[test]
fn an_unknown_release_or_feeder_is_not_found() {
    local(async {
        let rig = Rig::start(ClientRole::Owner).await;
        let unknown = Hex([0xab; 32]);

        refused(
            rig.http.install(rig.feeder, unknown).await,
            StatusCode::NOT_FOUND,
            ErrorCode::NotFound,
        );
        refused(
            rig.http
                .install(FeederId(Uuid::from_u128(9)), rig.release.id)
                .await,
            StatusCode::NOT_FOUND,
            ErrorCode::NotFound,
        );
        rig.stop().await;
    });
}
