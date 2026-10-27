//! An imported feeder against the simulator: the server it was imported
//! onto restarts with the feeder archived and not mirrored, and adopting
//! the same feeder again brings it back under its id, its history
//! continuing from where the archive left it.

use std::pin::pin;

use futures_util::TryStreamExt;
use wellfed_api::{Connection, ExportFormat};
use wellfed_server_testkit::{TestService, ada_setup, feeder, local, until};

#[test]
fn an_imported_feeder_restarts_archived_and_is_adopted_again_with_its_history() {
    local(async {
        let sim = feeder::start().await;
        let first = TestService::start().await;
        first.service().setup(ada_setup()).await.unwrap();
        let id = first.adopt(&sim).await;
        first.link(id, Connection::Online).await;
        until("the first server holds some history", || async {
            !first.bodies(id).await.is_empty()
        })
        .await;
        let held = first.bodies(id).await;
        let synced = first.service().feeder(id).await.unwrap().feeder.synced;

        let chunks = first.service().export(ExportFormat::Json, "test").await;
        let mut chunks = pin!(chunks.unwrap());
        let mut archive = Vec::new();
        while let Some(chunk) = chunks.try_next().await.unwrap() {
            archive.extend_from_slice(&chunk);
        }
        first.shutdown().await;

        let second = TestService::start().await;
        let upload = futures_util::stream::iter([Ok::<_, std::io::Error>(archive)]);
        second.service().import(upload).await.unwrap();

        // A restart skips the archived feeder: no key came with it.
        let store = second.stop().await;
        let second = TestService::reopen(store).await;
        let feeders = second.service().feeders().await.unwrap();
        assert_eq!(feeders.len(), 1);
        assert_eq!(feeders[0].id, id);
        assert!(feeders[0].archived.is_some(), "{feeders:?}");
        assert_eq!(feeders[0].connection, Connection::Offline);
        assert_eq!(second.bodies(id).await, held, "the history came along");

        // Adopted again: the same id, mirrored, and the history goes on
        // from the archive's cursor rather than starting over.
        assert_eq!(second.adopt(&sim).await, id);
        second.link(id, Connection::Online).await;
        let detail = second.service().feeder(id).await.unwrap();
        assert_eq!(detail.feeder.archived, None);
        assert!(detail.feeder.synced >= synced);
        until("the feeder's new events follow the old ones", || async {
            let bodies = second.bodies(id).await;
            bodies.len() > held.len() && bodies.starts_with(&held)
        })
        .await;

        second.shutdown().await;
        sim.stop().await;
    });
}
