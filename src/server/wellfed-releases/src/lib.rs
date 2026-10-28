//! The server's releases ([API design]): signed updates kept for the owner
//! to install, whether fetched from the project's GitHub releases or
//! uploaded (an offline home, a self-built image).
//!
//! Both paths end in the same place: [`wellfed_client::release::verify`],
//! which checks the manifest with the feeder's own offer check, against the
//! project's keys and the owner's enrolled update keys, then the image
//! against the manifest. Only a file that passes is kept, in a cache in the
//! data directory ([`cache`]): the release file under its manifest digest,
//! and beside it what the server knows of it. Reading one back for an
//! install verifies it again, so a file changed on disk, or a key the owner
//! has since removed, is caught before anything reaches a feeder.
//!
//! Fetching is off unless the owner turns it on (`releases.fetch`). A
//! [`Releases::check`] reads the project's releases over HTTPS and fetches
//! every `.plota` asset the cache lacks; the server's jobs run it daily,
//! and an owner may ask for one at any time. Nothing here installs: that
//! is an owner's request for one feeder.
//!
//! Beside the releases, [`Stock`] keeps each feeder's own stock firmware:
//! the backup "Restore stock" installs, checked against the pinned stock
//! image the same way the installer checks it.
//!
//! Time comes in as `now` from the server's clock, so tests fix it.
//!
//! [API design]: ../../../../docs/api/README.md
#![forbid(unsafe_code)]

mod cache;
mod error;
mod github;
mod stock;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use jiff::Timestamp;
use wellfed_api::{AssetName, Release, ReleaseCheck, ReleasePage, ReleaseQuery, ReleaseSource};
use wellfed_model::Hex;

pub use error::Error;
pub use stock::{Stock, StockConfig, Taken};
pub use wellfed_client::install::{
    DOWNGRADE_LEN, RestoreError, RestoreOffer, STOCK_VERSION, StockBackup, StockPins,
};
pub use wellfed_client::release::{MANIFEST_LEN, ReleaseError};

use cache::{Cache, Origin};

/// The project's releases on GitHub.
pub const GITHUB_API: &str = "https://api.github.com";
pub const REPOSITORY: &str = "Isaac-Duarte/wellfed";

/// The file name a release asset ends in: the manifest's magic, `PLOTA2`,
/// names the format.
pub const ASSET_SUFFIX: &str = ".plota";

/// The longest release file: a manifest and a full bank.
pub const MAX_FILE: usize = wellfed_client::release::MAX_LEN;

/// The keys a release may be signed with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Keys {
    /// The project's: the key the firmware builds trust.
    pub project: Vec<[u8; 32]>,
    /// The owner's update keys, as they enrolled them on their feeders.
    pub owner: Vec<[u8; 32]>,
}

/// Where releases are fetched from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// The GitHub API's base URL, without a trailing slash.
    pub api: String,
    /// `owner/name`.
    pub repository: String,
}

impl Default for Source {
    fn default() -> Self {
        Self {
            api: GITHUB_API.into(),
            repository: REPOSITORY.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    /// The cache: `releases/` in the data directory.
    pub dir: PathBuf,
    pub keys: Keys,
    /// Where releases are fetched from; `None` while fetching is off.
    pub fetch: Option<Source>,
    /// The feeders' stock backups.
    pub stock: StockConfig,
}

/// Whether a release was new to the cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Added {
    New,
    /// The cache held it already, from an earlier upload or check.
    Known,
}

/// The release cache and its fetcher. One per server.
#[derive(Debug)]
pub struct Releases {
    cache: Arc<Cache>,
    stock: Stock,
    fetch: Option<Source>,
    http: reqwest::Client,
    /// One check at a time: a check the owner asks for while the daily one
    /// runs waits for it, then finds what it fetched.
    checking: tokio::sync::Mutex<()>,
    /// Assets this process fetched and did not add (refused, or already
    /// held under an upload), so a daily check does not fetch them again.
    seen: Mutex<HashSet<u64>>,
}

/// How long a connection to GitHub may take to open.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long one request, a whole asset's download included, may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

impl Releases {
    /// The cache in `config.dir`, made if missing. A write a crash cut short
    /// is cleaned up here.
    pub async fn open(config: Config) -> Result<Self, Error> {
        let cache = Cache::open(config.dir, config.keys).await?;
        let stock = Stock::open(config.stock).await?;
        let http = reqwest::Client::builder()
            .user_agent(concat!("wellfed-server/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(Error::Client)?;

        Ok(Self {
            cache: Arc::new(cache),
            stock,
            fetch: config.fetch,
            http,
            checking: tokio::sync::Mutex::new(()),
            seen: Mutex::new(HashSet::new()),
        })
    }

    /// The feeders' stock backups.
    pub fn stock(&self) -> &Stock {
        &self.stock
    }

    /// Whether [`Self::check`] may run.
    pub fn fetching(&self) -> bool {
        self.fetch.is_some()
    }

    /// A page of the cached releases, newest version first.
    pub async fn list(&self, query: &ReleaseQuery) -> Result<ReleasePage, Error> {
        let limit = match query.limit {
            None => 100,
            Some(limit @ 1..=1000) => limit,
            Some(limit) => return Err(Error::Limit { limit }),
        };
        let after = query.after.as_deref().map(cache::id).transpose()?;

        let releases = self.cache.list().await?;
        let start = match after {
            None => 0,
            Some(after) => {
                let at = releases.iter().position(|(release, _)| release.id == after);
                at.ok_or(Error::Cursor)? + 1
            }
        };
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        let items: Vec<Release> = releases
            .into_iter()
            .skip(start)
            .filter_map(|(release, listed)| listed.then_some(release))
            .take(limit + 1)
            .collect();
        Ok(page(items, limit))
    }

    /// Verify an uploaded file and keep it.
    pub async fn upload(&self, file: Vec<u8>, now: Timestamp) -> Result<(Release, Added), Error> {
        self.cache.put(file, Origin::Upload, now).await
    }

    /// A cached release and its file, verified again against today's keys:
    /// what an install offers.
    pub async fn file(&self, id: Hex<32>) -> Result<(Release, Vec<u8>), Error> {
        self.cache.file(id).await
    }

    /// Fetch the project's releases the cache lacks, verify each, and keep
    /// those that pass. A release list GitHub does not serve ends the
    /// check; an asset that fails to download is reported and tried again
    /// by the next check.
    pub async fn check(&self, now: Timestamp) -> Result<ReleaseCheck, Error> {
        let Some(source) = &self.fetch else {
            return Err(Error::FetchDisabled);
        };
        let _running = self.checking.lock().await;

        let listed = github::releases(&self.http, source).await?;
        let held = self.cache.assets().await?;
        let mut check = ReleaseCheck {
            added: Vec::new(),
            refused: Vec::new(),
            failed: Vec::new(),
        };
        for asset in listed {
            if held.contains(&asset.id) || self.seen().contains(&asset.id) {
                continue;
            }

            let origin = Origin::Github {
                tag: asset.tag.clone(),
                asset: asset.name.clone(),
                id: asset.id,
            };
            let downloaded = match github::download(&self.http, &asset).await {
                Ok(downloaded) => downloaded,
                Err(error) => {
                    tracing::warn!(tag = %asset.tag, asset = %asset.name, "release download failed: {error}");
                    check.failed.push(AssetName {
                        tag: asset.tag,
                        asset: asset.name,
                    });
                    continue;
                }
            };
            let outcome = match downloaded {
                Some(file) => self.cache.put(file, origin, now).await,
                None => Err(ReleaseError::TooLarge {
                    len: usize::try_from(asset.size).unwrap_or(usize::MAX),
                }
                .into()),
            };
            match outcome {
                Ok((release, Added::New)) => {
                    tracing::info!(tag = %asset.tag, asset = %asset.name, "release {} added", release.version);
                    check.added.push(release);
                }
                Ok((_, Added::Known)) => {
                    self.seen().insert(asset.id);
                }
                Err(error) => {
                    let Some(reason) = error.refusal() else {
                        return Err(error);
                    };
                    tracing::warn!(tag = %asset.tag, asset = %asset.name, "release refused: {error}");
                    self.seen().insert(asset.id);
                    check.refused.push(wellfed_api::RefusedAsset {
                        tag: asset.tag,
                        asset: asset.name,
                        reason,
                    });
                }
            }
        }
        Ok(check)
    }

    fn seen(&self) -> std::sync::MutexGuard<'_, HashSet<u64>> {
        // Insertions are whole, so a poisoned set is still consistent.
        self.seen.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// `items` (at most `limit + 1` of them) as a page of `limit`, whose
/// `next` names its last release when another follows.
fn page(mut items: Vec<Release>, limit: usize) -> ReleasePage {
    let more = items.len() > limit;
    items.truncate(limit);
    let next = more
        .then(|| items.last().map(|release| release.id.to_string()))
        .flatten();
    ReleasePage { items, next }
}

impl From<&Origin> for ReleaseSource {
    fn from(origin: &Origin) -> Self {
        match origin {
            Origin::Upload => Self::Upload,
            Origin::Github { tag, asset, .. } => Self::Github {
                tag: tag.clone(),
                asset: asset.clone(),
            },
        }
    }
}
