//! Readiness: what an owner checks when something looks wrong.

use tracing::warn;
use wellfed_api::{FeederLink, ServerStatus, StoreState};

use crate::Service;

impl Service {
    /// The database, each feeder's link and the jobs' last runs. A failing
    /// database is reported here, not refused, so the view still answers;
    /// its error goes to the log. `version` is the server binary's.
    pub async fn status(&self, version: &str) -> ServerStatus {
        let store = &self.shared.store;
        let checked = store.ping().await;
        let rows = match checked {
            Ok(()) => store.feeders().await,
            Err(error) => Err(error),
        };
        let rows = match rows {
            Ok(feeders) => store.job_runs().await.map(|jobs| (feeders, jobs)),
            Err(error) => Err(error),
        };
        let (store, (feeders, jobs)) = match rows {
            Ok(rows) => (StoreState::Ok, rows),
            Err(error) => {
                warn!("readiness: the database failed: {error}");
                (StoreState::Failing, (Vec::new(), Vec::new()))
            }
        };

        let feeders = feeders
            .into_iter()
            .map(|row| {
                let feeder = self.view(row);
                FeederLink {
                    id: feeder.id,
                    name: feeder.name,
                    connection: feeder.connection,
                }
            })
            .collect();
        ServerStatus {
            version: version.into(),
            started: self.shared.started,
            store,
            feeders,
            jobs,
        }
    }
}
