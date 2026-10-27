//! An install across a server restart: the one running when the server
//! stopped is failed (`interrupted`) and readable after it starts again,
//! the slot is free, and the next install takes up the transfer the feeder
//! kept receiving.

use std::time::Duration;

use wellfed_api::{ClientRole, Connection as Link, InstallFailure, InstallRequest, InstallState};
use wellfed_server_testkit::feeder::{self, Banks};
use wellfed_server_testkit::release::{self, SECTOR, project, signed};
use wellfed_server_testkit::{TestService, WAIT, ada, config, local, until};
use wellfed_service::{ReleaseAdded, Service};

const BUILD: [u8; 32] = [0x77; 32];
const LEN: usize = 16 * SECTOR;

async fn ended(service: &Service, id: wellfed_api::FeederId) -> wellfed_api::Install {
    let ended = tokio::time::timeout(4 * WAIT, async {
        loop {
            let install = service.install_state(id).await.unwrap();
            if install.state != InstallState::Running {
                return install;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    ended.await.expect("the install ends in time")
}

#[test]
fn a_restart_mid_install_fails_it_frees_the_slot_and_the_next_activates() {
    local(async {
        let banks = Banks::new();
        let sim = feeder::start_updatable(&banks).await;
        let server = TestService::start_with(release::trusting(config())).await;
        let address = sim.address.to_string();
        let id = server.adopt_as(&sim, &address, ClientRole::Owner).await;
        server.link(id, Link::Online).await;
        let file = signed(&project(), 7, BUILD, LEN);
        let (release, added) = server.service().upload_release(file).await.unwrap();
        assert_eq!(added, ReleaseAdded::New);
        let request = InstallRequest {
            release: release.id,
        };

        let first = server
            .service()
            .install(&ada().into(), id, request)
            .await
            .unwrap();
        until("the transfer is under way", || async {
            server.service().install_state(id).await.unwrap().written > 0
        })
        .await;
        let store = server.stop().await;

        let server = TestService::open(store, release::trusting(config())).await;
        let interrupted = server.service().install_state(id).await.unwrap();
        assert_eq!(interrupted.id, first.id);
        assert_eq!(interrupted.state, InstallState::Failed);
        assert_eq!(interrupted.failure, Some(InstallFailure::Interrupted));
        assert!(interrupted.finished.is_some());

        // The feeder keeps a quiet transfer for a minute: offered again, the
        // release is taken up from where bank 0 stands and activated.
        server.link(id, Link::Online).await;
        let second = server
            .service()
            .install(&ada().into(), id, request)
            .await
            .unwrap();
        assert_ne!(second.id, first.id);
        let ended = ended(server.service(), id).await;
        assert_eq!(
            (ended.id, ended.state),
            (second.id, InstallState::Activated)
        );
        until("the feeder restarts into the release", || async {
            banks.borrow().bank1_build == BUILD
        })
        .await;

        server.shutdown().await;
        sim.stop().await;
    });
}
