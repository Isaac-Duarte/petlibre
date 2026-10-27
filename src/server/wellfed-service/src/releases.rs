//! The releases the server keeps for its owner: listed, uploaded and
//! checked for on the project's GitHub releases, through
//! `wellfed-releases`, with the time from the service's clock.

use wellfed_api::{Release, ReleaseCheck, ReleasePage, ReleaseQuery};
use wellfed_model::Hex;

use crate::{ReleaseAdded, Result, Service};

impl Service {
    /// A page of the cached releases, newest version first.
    pub async fn releases(&self, query: &ReleaseQuery) -> Result<ReleasePage> {
        Ok(self.releases.list(query).await?)
    }

    /// Verify an uploaded release file and keep it.
    pub async fn upload_release(&self, file: Vec<u8>) -> Result<(Release, ReleaseAdded)> {
        Ok(self.releases.upload(file, self.shared.clock.now()).await?)
    }

    /// Fetch and verify the project's releases the cache lacks. Refused
    /// while fetching is off; the server's daily job runs it only while
    /// [`Self::fetching_releases`].
    pub async fn check_releases(&self) -> Result<ReleaseCheck> {
        self.run_release_check(self.shared.clock.now()).await
    }

    /// Whether the owner allows the server to fetch releases.
    pub fn fetching_releases(&self) -> bool {
        self.releases.fetching()
    }

    /// A cached release and its file, verified again: what an install
    /// offers.
    pub async fn release_file(&self, id: Hex<32>) -> Result<(Release, Vec<u8>)> {
        Ok(self.releases.file(id).await?)
    }
}
