//! The project's GitHub releases: the list (the REST API's
//! `/repos/{owner}/{name}/releases`, newest first) and each release asset's
//! download. Drafts and pre-releases are left out; an owner who wants one
//! uploads it. Nothing read here is trusted: every file is verified before
//! the cache keeps it, so this only bounds what it reads.

use reqwest::header::ACCEPT;
use serde::Deserialize;
use wellfed_client::release::MAX_LEN;

use crate::{ASSET_SUFFIX, Error, Source};

/// The most a release list may take: thirty releases' JSON is a few
/// hundred kilobytes.
const LIST_MAX: usize = 4 << 20;

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    id: u64,
    name: String,
    size: u64,
    browser_download_url: String,
}

/// A release file one of the project's releases offers.
#[derive(Clone, Debug)]
pub(crate) struct Asset {
    pub(crate) id: u64,
    pub(crate) tag: String,
    pub(crate) name: String,
    /// The size GitHub states, checked before downloading.
    pub(crate) size: u64,
    url: String,
}

/// The release files of the project's latest published releases.
pub(crate) async fn releases(http: &reqwest::Client, source: &Source) -> Result<Vec<Asset>, Error> {
    let url = format!(
        "{}/repos/{}/releases?per_page=30",
        source.api, source.repository
    );
    let response = http
        .get(url)
        .header(ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(Error::Unreachable)?;
    let body = limited(response, LIST_MAX)
        .await?
        .ok_or(Error::ListingTooLarge)?;
    let releases: Vec<GithubRelease> = serde_json::from_slice(&body).map_err(Error::Listing)?;

    Ok(releases
        .into_iter()
        .filter(|release| !release.draft && !release.prerelease)
        .flat_map(|release| {
            let tag = release.tag_name;
            release
                .assets
                .into_iter()
                .filter(|asset| asset.name.ends_with(ASSET_SUFFIX))
                .map(move |asset| Asset {
                    id: asset.id,
                    tag: tag.clone(),
                    name: asset.name,
                    size: asset.size,
                    url: asset.browser_download_url,
                })
        })
        .collect())
}

/// The asset's bytes, or `None` when it is larger than any release.
pub(crate) async fn download(
    http: &reqwest::Client,
    asset: &Asset,
) -> Result<Option<Vec<u8>>, Error> {
    if usize::try_from(asset.size).map_or(true, |size| size > MAX_LEN) {
        return Ok(None);
    }

    let response = http
        .get(&asset.url)
        .header(ACCEPT, "application/octet-stream")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(Error::Unreachable)?;
    limited(response, MAX_LEN).await
}

/// The response's body, or `None` once it passes `max` bytes, whatever
/// its headers said.
async fn limited(mut response: reqwest::Response, max: usize) -> Result<Option<Vec<u8>>, Error> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(Error::Unreachable)? {
        if body.len() + chunk.len() > max {
            return Ok(None);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Some(body))
}
