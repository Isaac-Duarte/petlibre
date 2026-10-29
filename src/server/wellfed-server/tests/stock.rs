//! The feeder's stock firmware over HTTP, against the simulator's banks:
//! the server reads bank 1 from the replacement on bank 0 and keeps it as
//! a verified file, exports it byte for byte, and restores it after
//! promotion through the update path, as the owner signed it. Nothing
//! unverified is kept or sent, and only an owner does any of it.
//!
//! The simulator's bank 1 starts as a synthetic stock, and the test server
//! keeps stock against its pins: the vendor's image stays out of the
//! repository.

use std::cell::RefCell;
use std::rc::Rc;

use wellfed_api::{
    AdoptFeeder, ClientRole, Connection, Credentials, ErrorCode, FeederId, FeederRefusal,
    InstallState, InstallTarget, NewUser, Release, Role, StockBackupState, StockSource,
    StreamEvent, StreamQuery,
};
use wellfed_server_client::{Client, Error, Kept, StatusCode, Uploaded};
use wellfed_server_testkit::feeder::{self, Banks, Running};
use wellfed_server_testkit::release::{
    self, SECTOR, keeping_stock, project, signed, stock_restore,
};
use wellfed_server_testkit::{TestServer, WAIT, ada_setup, config, local, refused, until};

const BUILD: [u8; 32] = [0x77; 32];
const LEN: usize = 16 * SECTOR;

/// An owner's server keeping the simulator's stock, with an updatable
/// simulator adopted as `role`, online, and a release of [`BUILD`]
/// uploaded. `stock` is bank 1 as the simulator starts: the stock image.
struct Rig {
    sim: Running,
    banks: Rc<RefCell<Banks>>,
    server: TestServer,
    http: Client,
    feeder: FeederId,
    release: Release,
    stock: Vec<u8>,
}

impl Rig {
    async fn start(role: ClientRole) -> Self {
        let banks = Banks::new();
        let stock = banks.borrow().bank1().to_vec();
        let sim = feeder::start_updatable(&banks).await;
        let config = keeping_stock(release::trusting(config()), &banks);
        let server = TestServer::start_with(config).await;
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
            stock,
        }
    }

    /// Stock installs the replacement into bank 0, and the server sees the
    /// feeder serve bank 1.
    async fn to_bank_0(&self) {
        feeder::stock_gave_way(&self.sim, &self.banks);
        until("the feeder serves bank 1 from bank 0", || async {
            let state = self.http.stock(self.feeder).await.unwrap();
            state.feeder_state.is_some_and(|feeder| feeder.readable)
        })
        .await;
    }

    /// Install the release (promotion, which waits for a kept stock backup
    /// when the feeder runs from bank 0) and wait until the feeder answers
    /// from it on bank 1.
    async fn promote(&self) {
        self.http
            .install(self.feeder, self.release.id)
            .await
            .unwrap();
        until("the feeder answers from the release", || async {
            let state = self.http.stock(self.feeder).await.unwrap();
            state.feeder_state.is_some_and(|feeder| feeder.bank == 1)
                && self.banks.borrow().bank1_build == BUILD
        })
        .await;
    }

    async fn stop(self) {
        self.server.stop().await;
        self.sim.stop().await;
    }
}

#[test]
fn bank_1_read_from_bank_0_is_kept_exported_and_restored_after_promotion() {
    local(async {
        let rig = Rig::start(ClientRole::Owner).await;
        let state = rig.http.stock(rig.feeder).await.unwrap();
        assert_eq!((state.backup, state.restore), (None, None));
        let on_bank_1 = state.feeder_state.expect("an owner's key reads the banks");
        assert_eq!((on_bank_1.bank, on_bank_1.readable), (1, false));

        // Bank 1 serves no stock of its own.
        match rig.http.read_stock(rig.feeder).await {
            Err(Error::Problem {
                status, problem, ..
            }) => {
                assert_eq!(status, StatusCode::CONFLICT);
                assert_eq!(problem.refusal, Some(FeederRefusal::Unavailable));
            }
            other => panic!("expected a refusal, got {other:?}"),
        }

        rig.to_bank_0().await;
        let Kept::New(kept) = rig.http.read_stock(rig.feeder).await.unwrap() else {
            panic!("a first read is kept");
        };
        assert_eq!(kept.version, "1.1.15");
        assert_eq!(kept.state, StockBackupState::Verified);
        assert_eq!(kept.source, StockSource::Feeder);
        assert_eq!(
            rig.http.read_stock(rig.feeder).await.unwrap(),
            Kept::Known(kept.clone()),
            "a kept copy is not read again"
        );

        // The export is the vendor's container around bank 1 as stock had
        // it, its marker byte restored.
        let exported = rig.http.stock_backup(rig.feeder).await.unwrap();
        assert_eq!(kept.bytes, exported.len() as u64);
        assert_eq!(&exported[32..], &rig.stock[..]);

        rig.promote().await;
        let floor = rig
            .http
            .stock(rig.feeder)
            .await
            .unwrap()
            .feeder_state
            .unwrap()
            .floor;
        let mut updates = rig
            .http
            .stream(&StreamQuery::default(), None)
            .await
            .unwrap();
        let restore = stock_restore(&project(), 1, &rig.stock, rig.sim.identity, floor);
        let started = rig.http.restore_stock(rig.feeder, &restore).await.unwrap();
        assert_eq!(
            (started.target, started.version, started.state),
            (
                InstallTarget::Stock {
                    sha256: kept.sha256
                },
                1,
                InstallState::Running
            )
        );
        assert_eq!(started.image_len as usize, rig.stock.len());

        // Its progress and its end reach the stream as an install's do.
        let ended = loop {
            let update = tokio::time::timeout(8 * WAIT, updates.next())
                .await
                .expect("an update in time")
                .expect("the stream is open")
                .unwrap();
            let StreamEvent::Install { install, .. } = update.event else {
                continue;
            };
            if install.id == started.id && install.state != InstallState::Running {
                break install;
            }
        };
        assert_eq!(ended.state, InstallState::Activated, "{ended:?}");
        assert_eq!(ended.written, ended.image_len);
        until("bank 1 holds stock again", || async {
            rig.banks.borrow().bank1() == &rig.stock[..]
        })
        .await;
        let state = rig.http.stock(rig.feeder).await.unwrap();
        assert_eq!(state.restore.map(|restore| restore.id), Some(started.id));
        rig.stop().await;
    });
}

/// Promotion overwrites bank 1, the feeder's only copy of stock, so an
/// install on a feeder running from bank 0 with stock in bank 1 waits for
/// a kept backup, as the installer's promotion does; nothing is sent or
/// recorded before it.
#[test]
fn an_install_over_stock_waits_for_a_kept_backup() {
    local(async {
        let rig = Rig::start(ClientRole::Owner).await;
        rig.to_bank_0().await;

        refused(
            rig.http.install(rig.feeder, rig.release.id).await,
            StatusCode::CONFLICT,
            ErrorCode::StockBackupNeeded,
        );
        refused(
            rig.http.install_state(rig.feeder).await,
            StatusCode::NOT_FOUND,
            ErrorCode::NotFound,
        );
        // Stock's activation programmed only the marker's first word.
        assert_eq!(rig.banks.borrow().bank1()[4..], rig.stock[4..]);

        rig.http.read_stock(rig.feeder).await.unwrap();
        rig.promote().await;
        rig.stop().await;
    });
}

/// Nothing unverified is kept, and nothing is sent unless the manifest
/// offers exactly the kept backup as stock.
#[test]
fn only_a_verified_copy_is_kept_and_only_its_own_manifest_restores_it() {
    local(async {
        let rig = Rig::start(ClientRole::Owner).await;
        let floor = 0;
        let restore = stock_restore(&project(), 1, &rig.stock, rig.sim.identity, floor);

        refused(
            rig.http.restore_stock(rig.feeder, &restore).await,
            StatusCode::NOT_FOUND,
            ErrorCode::NoStockBackup,
        );
        refused(
            rig.http.stock_backup(rig.feeder).await,
            StatusCode::NOT_FOUND,
            ErrorCode::NoStockBackup,
        );

        let mut foreign = rig.stock.clone();
        foreign[0x1_0000] ^= 1;
        refused(
            rig.http.upload_stock(rig.feeder, foreign).await,
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::StockRefused,
        );
        assert_eq!(rig.http.stock(rig.feeder).await.unwrap().backup, None);

        // The installer's kept copy, as a bare payload.
        let Kept::New(kept) = rig
            .http
            .upload_stock(rig.feeder, rig.stock.clone())
            .await
            .unwrap()
        else {
            panic!("a first upload is kept");
        };
        assert_eq!(kept.source, StockSource::Upload);
        let exported = rig.http.stock_backup(rig.feeder).await.unwrap();
        assert!(matches!(
            rig.http.upload_stock(rig.feeder, exported).await.unwrap(),
            Kept::Known(_)
        ));

        let other = stock_restore(&project(), 1, &rig.stock[1..], rig.sim.identity, floor);
        refused(
            rig.http.restore_stock(rig.feeder, &other).await,
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::RestoreMismatch,
        );
        assert_eq!(rig.http.stock(rig.feeder).await.unwrap().restore, None);

        // One install or restore at a time.
        rig.http.install(rig.feeder, rig.release.id).await.unwrap();
        refused(
            rig.http.restore_stock(rig.feeder, &restore).await,
            StatusCode::CONFLICT,
            ErrorCode::InstallRunning,
        );
        rig.stop().await;
    });
}

#[test]
fn only_an_owner_keeps_exports_or_restores_stock_and_anyone_reads_its_state() {
    local(async {
        let rig = Rig::start(ClientRole::Owner).await;
        rig.http
            .upload_stock(rig.feeder, rig.stock.clone())
            .await
            .unwrap();
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

        let state = member.stock(rig.feeder).await.unwrap();
        assert!(state.backup.is_some());
        assert_eq!(
            state.feeder_state, None,
            "only an owner's read asks the feeder"
        );
        let restore = stock_restore(&project(), 1, &rig.stock, rig.sim.identity, 0);
        let forbidden = StatusCode::FORBIDDEN;
        refused(
            member.stock_backup(rig.feeder).await,
            forbidden,
            ErrorCode::Forbidden,
        );
        refused(
            member.read_stock(rig.feeder).await,
            forbidden,
            ErrorCode::Forbidden,
        );
        refused(
            member.upload_stock(rig.feeder, rig.stock.clone()).await,
            forbidden,
            ErrorCode::Forbidden,
        );
        refused(
            member.restore_stock(rig.feeder, &restore).await,
            forbidden,
            ErrorCode::Forbidden,
        );
        refused(
            rig.server.client().stock(rig.feeder).await,
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthenticated,
        );
        rig.stop().await;
    });
}

/// The feeder serves bank 1 and takes updates from an owner's key only, so
/// on a feeder adopted for the `server` role the read and the restore are
/// refused before anything is sent, and its banks are not read.
#[test]
fn a_feeder_adopted_for_the_server_role_is_refused_the_read_and_the_restore() {
    local(async {
        let rig = Rig::start(ClientRole::Server).await;
        let state = rig.http.stock(rig.feeder).await.unwrap();
        assert_eq!(state.feeder_state, None);

        let forbidden = |result: Result<_, Error>, request: &str| match result {
            Err(Error::Problem {
                status, problem, ..
            }) => {
                assert_eq!(status, StatusCode::CONFLICT, "{request}");
                assert_eq!(problem.code, ErrorCode::FeederRefused, "{request}");
                assert_eq!(problem.refusal, Some(FeederRefusal::Forbidden), "{request}");
            }
            other => panic!("{request}: expected a refusal, got {other:?}"),
        };
        forbidden(rig.http.read_stock(rig.feeder).await.map(|_| ()), "read");

        // An owner's own copy is still kept for the feeder.
        rig.http
            .upload_stock(rig.feeder, rig.stock.clone())
            .await
            .unwrap();
        let restore = stock_restore(&project(), 1, &rig.stock, rig.sim.identity, 0);
        forbidden(
            rig.http
                .restore_stock(rig.feeder, &restore)
                .await
                .map(|_| ()),
            "restore",
        );
        assert_eq!(rig.http.stock(rig.feeder).await.unwrap().restore, None);
        rig.stop().await;
    });
}
