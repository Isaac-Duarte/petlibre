//! Readiness from the service: a database that stops answering is reported
//! as failing, not refused.

use std::sync::Arc;

use wellfed_api::StoreState;
use wellfed_server_testkit::fixed::{FixedClock, SeededRandom};
use wellfed_server_testkit::{TestService, config, feeder, local};
use wellfed_service::Service;

#[test]
fn a_store_that_fails_is_reported_and_its_feeders_left_out() {
    local(async {
        let sim = feeder::start().await;
        let running = TestService::start().await;
        running.adopt(&sim).await;
        let temp = running.stop().await;

        // The test keeps a handle on the service's database, to close it
        // under the running service.
        let store = temp.open().await;
        let service = Service::start(
            store.clone(),
            config(),
            Arc::new(FixedClock),
            Arc::new(SeededRandom::fresh()),
        )
        .await
        .unwrap();
        let ready = service.status("test").await;
        assert_eq!(ready.store, StoreState::Ok);
        assert_eq!(ready.feeders.len(), 1);

        store.close().await;
        let failing = service.status("test").await;
        assert_eq!(failing.store, StoreState::Failing);
        assert!(failing.feeders.is_empty(), "the list comes from the store");
        assert_eq!(failing.version, "test");

        service.shutdown().await;
        sim.stop().await;
        drop(temp);
    });
}

#[tokio::test]
async fn a_fresh_store_is_ready_with_no_feeders_and_no_jobs() {
    let running = TestService::start().await;

    let ready = running.service().status("test").await;

    assert_eq!(ready.store, StoreState::Ok);
    assert_eq!(ready.started, FixedClock.now());
    assert!(ready.feeders.is_empty());
    assert!(ready.jobs.is_empty());
    running.shutdown().await;
}
