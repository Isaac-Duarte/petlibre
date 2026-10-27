//! The release cache: a directory in the server's data directory holding,
//! for each release, its file (`<id>.plota`) and what the server knows of
//! it (`<id>.json`), the id being the manifest's digest in hex.
//!
//! A file is verified before anything is written, and each write lands
//! whole: it goes to a `.part` file, is synced, then renamed into place,
//! the release file before its record. A record therefore always has its
//! file; a crash between the two leaves a file with no record, which the
//! next start removes along with any `.part`. The directory itself is the
//! index: a household holds a handful of releases, so listing reads every
//! record.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use jiff::Timestamp;
use wellfed_api::{Release, ReleaseSigner};
use wellfed_client::release::{self, KeyId, Signer};
use wellfed_model::Hex;
use serde::de::IntoDeserializer as _;
use serde::de::value::StrDeserializer;
use serde::{Deserialize, Serialize};

use crate::{Added, Error, Keys};

const FILE: &str = "plota";
const RECORD: &str = "json";
const PART: &str = "part";

/// Where a file came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    Upload,
    Github { tag: String, asset: String, id: u64 },
}

/// A release's record beside its file.
#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    release: Release,
    /// The GitHub asset it was fetched as, so a check does not fetch it
    /// again.
    asset: Option<u64>,
}

#[derive(Debug)]
pub(crate) struct Cache {
    dir: PathBuf,
    keys: Arc<Keys>,
    /// One writer at a time, so two uploads of one release cannot both
    /// find it missing and race on its files.
    writing: tokio::sync::Mutex<()>,
}

impl Cache {
    pub(crate) async fn open(dir: PathBuf, keys: Keys) -> Result<Self, Error> {
        let swept = dir.clone();
        blocking(move || {
            fs::create_dir_all(&swept).map_err(cache(&swept))?;
            sweep(&swept)
        })
        .await?;

        Ok(Self {
            dir,
            keys: Arc::new(keys),
            writing: tokio::sync::Mutex::new(()),
        })
    }

    /// Every release one of today's keys signed, newest version first
    /// (then by id, so pages are stable), its signer as those keys say. A
    /// release whose key the owner has since removed stays on disk but is
    /// not listed, as an install would refuse it.
    ///
    /// Each comes with whether it is listed, so a page cursor that names a
    /// release hidden since still marks its place.
    pub(crate) async fn list(&self) -> Result<Vec<(Release, bool)>, Error> {
        let dir = self.dir.clone();
        let project: Vec<KeyId> = self.keys.project.iter().map(release::key_id).collect();
        let owner: Vec<KeyId> = self.keys.owner.iter().map(release::key_id).collect();
        let mut releases: Vec<(Release, bool)> = blocking(move || entries(&dir))
            .await?
            .into_iter()
            .map(|entry| {
                let mut release = entry.release;
                let signer = if project.contains(&release.key_id.0) {
                    Some(ReleaseSigner::Project)
                } else if owner.contains(&release.key_id.0) {
                    Some(ReleaseSigner::Owner)
                } else {
                    None
                };
                if let Some(signer) = signer {
                    release.signer = signer;
                }
                (release, signer.is_some())
            })
            .collect();
        releases.sort_by(|(a, _), (b, _)| b.version.cmp(&a.version).then(a.id.0.cmp(&b.id.0)));
        Ok(releases)
    }

    /// The GitHub assets the cache holds.
    pub(crate) async fn assets(&self) -> Result<HashSet<u64>, Error> {
        let dir = self.dir.clone();
        let entries = blocking(move || entries(&dir)).await?;
        Ok(entries
            .into_iter()
            .filter_map(|entry| entry.asset)
            .collect())
    }

    /// Verify `file` and keep it, unless the cache holds it already.
    pub(crate) async fn put(
        &self,
        file: Vec<u8>,
        origin: Origin,
        now: Timestamp,
    ) -> Result<(Release, Added), Error> {
        let _writing = self.writing.lock().await;
        let dir = self.dir.clone();
        let keys = Arc::clone(&self.keys);

        blocking(move || {
            let verified = verify(&file, &keys)?;
            let id = Hex(verified.digest);
            let record = path(&dir, id, RECORD);
            if let Some(entry) = readable(&record, &path(&dir, id, FILE)) {
                return Ok((entry.release, Added::Known));
            }

            let release = Release {
                id,
                version: verified.version,
                build_id: Hex(verified.build_id),
                image_len: verified.image_len,
                image_sha256: Hex(verified.image_sha256),
                key_id: Hex(verified.key_id),
                signer: match verified.signer {
                    Signer::Project => ReleaseSigner::Project,
                    Signer::Owner => ReleaseSigner::Owner,
                },
                source: (&origin).into(),
                added: now,
            };
            let entry = Entry {
                release,
                asset: match origin {
                    Origin::Github { id, .. } => Some(id),
                    Origin::Upload => None,
                },
            };
            let text = serde_json::to_vec_pretty(&entry).map_err(|source| Error::Entry {
                path: record.clone(),
                source,
            })?;
            write_whole(&path(&dir, id, FILE), &file)?;
            write_whole(&record, &text)?;
            sync_dir(&dir)?;
            Ok((entry.release, Added::New))
        })
        .await
    }

    /// Release `id` and its file, verified again.
    pub(crate) async fn file(&self, id: Hex<32>) -> Result<(Release, Vec<u8>), Error> {
        let dir = self.dir.clone();
        let keys = Arc::clone(&self.keys);

        blocking(move || {
            let file_path = path(&dir, id, FILE);
            let entry = readable(&path(&dir, id, RECORD), &file_path).ok_or(Error::NotFound)?;
            let file = fs::read(&file_path).map_err(cache(&file_path))?;

            let verified = verify(&file, &keys)?;
            if verified.digest != id.0 {
                return Err(Error::Changed);
            }
            Ok((entry.release, file))
        })
        .await
    }
}

/// A page cursor: a release id in hex.
pub(crate) fn id(text: &str) -> Result<Hex<32>, Error> {
    let text: StrDeserializer<'_, serde::de::value::Error> = text.into_deserializer();
    Hex::deserialize(text).map_err(|_| Error::Cursor)
}

fn verify(file: &[u8], keys: &Keys) -> Result<release::Verified, Error> {
    let keys = release::Keys {
        project: &keys.project,
        owner: &keys.owner,
    };
    Ok(release::verify(file, &keys)?)
}

/// Run `work` on the blocking pool: reading, hashing and syncing a
/// megabyte is not for the async workers.
pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| Error::Task)?
}

fn path(dir: &Path, id: Hex<32>, extension: &str) -> PathBuf {
    dir.join(format!("{id}.{extension}"))
}

pub(crate) fn cache(path: &Path) -> impl FnOnce(io::Error) -> Error + '_ {
    move |source| Error::Cache {
        path: path.to_owned(),
        source,
    }
}

/// The record at `path`, or `None` when there is none.
fn read_entry(path: &Path) -> Result<Option<Entry>, Error> {
    let text = match fs::read(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(cache(path)(source)),
    };
    serde_json::from_slice(&text)
        .map(Some)
        .map_err(|source| Error::Entry {
            path: path.to_owned(),
            source,
        })
}

/// The record at `record`, if it parses and its release file is there.
/// Anything else is logged and treated as no record, so one damaged file
/// neither hides the others nor stops the release being kept again.
fn readable(record: &Path, file: &Path) -> Option<Entry> {
    let entry = match read_entry(record) {
        Ok(entry) => entry?,
        Err(error) => {
            tracing::warn!("skipping a release record: {error}");
            return None;
        }
    };
    if !file.exists() {
        tracing::warn!("skipping {}: its release file is missing", record.display());
        return None;
    }
    Some(entry)
}

/// Every readable record in `dir`.
fn entries(dir: &Path) -> Result<Vec<Entry>, Error> {
    let mut out = Vec::new();
    for item in fs::read_dir(dir).map_err(cache(dir))? {
        let path = item.map_err(cache(dir))?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == RECORD)
            && let Some(entry) = readable(&path, &path.with_extension(FILE))
        {
            out.push(entry);
        }
    }
    Ok(out)
}

/// Remove what a cut-short write left: `.part` files, and a release file
/// whose record never landed.
fn sweep(dir: &Path) -> Result<(), Error> {
    for item in fs::read_dir(dir).map_err(cache(dir))? {
        let path = item.map_err(cache(dir))?.path();
        let extension = path.extension().and_then(|extension| extension.to_str());
        let orphan = extension == Some(FILE) && !path.with_extension(RECORD).exists();
        if extension == Some(PART) || orphan {
            tracing::warn!("removing {}, left by an interrupted write", path.display());
            fs::remove_file(&path).map_err(cache(&path))?;
        }
    }
    Ok(())
}

/// Write `bytes` to `path` so that it is either absent or whole.
pub(crate) fn write_whole(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let mut part = path.as_os_str().to_owned();
    part.push(".");
    part.push(PART);
    let part = PathBuf::from(part);

    let mut file = File::create(&part).map_err(cache(&part))?;
    file.write_all(bytes).map_err(cache(&part))?;
    file.sync_all().map_err(cache(&part))?;
    fs::rename(&part, path).map_err(cache(path))
}

pub(crate) fn sync_dir(dir: &Path) -> Result<(), Error> {
    File::open(dir)
        .and_then(|dir| dir.sync_all())
        .map_err(cache(dir))
}
