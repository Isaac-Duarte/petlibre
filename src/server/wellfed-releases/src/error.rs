//! Why a release operation failed: a file that is not a release (a typed
//! [`ReleaseRefusal`] the API carries), fetching that is off or cannot
//! reach GitHub, a bad page cursor, or the cache's own disk.

use std::io;
use std::path::PathBuf;

use wellfed_api::ReleaseRefusal;
use wellfed_client::release::ReleaseError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a release: {0}")]
    Refused(#[from] ReleaseError),
    #[error("fetching releases is off (releases.fetch)")]
    FetchDisabled,
    #[error("build the HTTP client")]
    Client(#[source] reqwest::Error),
    #[error("could not read the project's releases")]
    Unreachable(#[source] reqwest::Error),
    #[error("the project's release list does not parse")]
    Listing(#[source] serde_json::Error),
    #[error("the project's release list is larger than any the server reads")]
    ListingTooLarge,
    #[error("no such release")]
    NotFound,
    #[error("the cached file is no longer the release its name says")]
    Changed,
    #[error("`after` names no release the server holds")]
    Cursor,
    #[error("a page holds 1-1000 releases, not {limit}")]
    Limit { limit: u32 },
    #[error("release cache {}", path.display())]
    Cache {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("release record {} does not parse", path.display())]
    Entry {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("the release cache's task stopped before it finished")]
    Task,
}

impl Error {
    /// Why the file was refused, if it was.
    pub fn refusal(&self) -> Option<ReleaseRefusal> {
        let Self::Refused(error) = self else {
            return None;
        };
        Some(match error {
            ReleaseError::Short { .. } | ReleaseError::TooLarge { .. } => ReleaseRefusal::Size,
            ReleaseError::Manifest(_) => ReleaseRefusal::Manifest,
            ReleaseError::Stock => ReleaseRefusal::NeedsStatement,
            ReleaseError::UnknownKey => ReleaseRefusal::UnknownKey,
            ReleaseError::Signature => ReleaseRefusal::Signature,
            ReleaseError::Refused(wellfed_client::release::OfferError::Downgrade) => {
                ReleaseRefusal::NeedsStatement
            }
            ReleaseError::Refused(_) => ReleaseRefusal::Refused,
            ReleaseError::ImageLength { .. } | ReleaseError::ImageDigest | ReleaseError::Marker => {
                ReleaseRefusal::Image
            }
        })
    }
}
