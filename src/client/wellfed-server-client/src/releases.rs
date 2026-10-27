//! The releases the server keeps for installs: the list, an upload and a
//! check of the project's releases. Owners only.

use wellfed_api::{Release, ReleaseCheck, ReleasePage, ReleaseQuery};
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
}
