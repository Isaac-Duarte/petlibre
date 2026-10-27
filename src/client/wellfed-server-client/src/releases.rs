//! The releases the server keeps for installs: the list, an upload and a
//! check of the project's releases (owners only), and installing one on a
//! feeder.

use wellfed_api::{
    FeederId, Hex, Install, InstallRequest, Release, ReleaseCheck, ReleasePage, ReleaseQuery,
};
use reqwest::header::{self, HeaderValue};
use reqwest::{Method, StatusCode};

use crate::{Client, Error, decode};

/// What an upload did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Uploaded {
    /// Verified and kept.
    New(Release),
    /// The server held it already.
    Known(Release),
}

impl Client {
    /// A page of the server's verified releases, newest version first.
    pub async fn releases(&self, query: &ReleaseQuery) -> Result<ReleasePage, Error> {
        let response = self
            .api(Method::GET, "/releases")
            .query(query)
            .send()
            .await?;
        decode(response, StatusCode::OK).await
    }

    /// Upload a release file. A file the server refuses comes back as a
    /// `release_refused` problem whose `release` says why. Uploading the
    /// same file again is harmless, so it needs no idempotency key.
    pub async fn upload_release(&self, file: Vec<u8>) -> Result<Uploaded, Error> {
        let response = self
            .api(Method::POST, "/releases")
            .header(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            )
            .body(file)
            .send()
            .await?;

        if response.status() == StatusCode::OK {
            return Ok(Uploaded::Known(decode(response, StatusCode::OK).await?));
        }
        Ok(Uploaded::New(decode(response, StatusCode::CREATED).await?))
    }

    /// Check the project's releases now, keeping each new one that
    /// verifies. Refused as `fetch_disabled` while fetching is off.
    pub async fn check_releases(&self) -> Result<ReleaseCheck, Error> {
        let response = self.api(Method::POST, "/releases/check").send().await?;
        decode(response, StatusCode::OK).await
    }

    /// Install a release on a feeder. It runs on the server in the
    /// background: this returns once it has started, and the stream carries
    /// its progress and its end. Owners only; a second install while one
    /// runs is refused `install_running`.
    pub async fn install(&self, feeder: FeederId, release: Hex<32>) -> Result<Install, Error> {
        let response = self
            .api(Method::POST, &format!("/feeders/{feeder}/install"))
            .json(&InstallRequest { release })
            .send()
            .await?;
        decode(response, StatusCode::ACCEPTED).await
    }

    /// The feeder's running install, or else its last one.
    pub async fn install_state(&self, feeder: FeederId) -> Result<Install, Error> {
        let response = self
            .api(Method::GET, &format!("/feeders/{feeder}/install"))
            .send()
            .await?;
        decode(response, StatusCode::OK).await
    }
}
