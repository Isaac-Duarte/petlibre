//! The sealing key: `keys/server.key` in the data directory, the key's 32
//! bytes as they are, readable by the server's user only (0600, in a 0700
//! directory). It seals every feeder key the database holds, so the
//! database alone cannot open them, and backups leave it out.
//!
//! Servers before the settings file kept it in `config.json`. The first
//! start moves it: the new file is written and synced, then `config.json`
//! (which held nothing else) is deleted, so the secret is not left in a
//! second file. A start interrupted between the two finds both holding one
//! key and finishes the move; two different keys are refused, since only
//! one of them opens the stored feeder keys.
//!
//! A database with no key file is refused rather than given a new key,
//! which could not open its feeder keys: that is a backup restored without
//! the key, or a key file lost.

use std::io::Write;
use std::path::{Path, PathBuf};

use wellfed_api::Hex;
use wellfed_service::Random;
use serde::Deserialize;

use crate::config::DATABASE;

/// The key file, under the data directory.
pub const KEY_DIR: &str = "keys";
pub const KEY_FILE: &str = "server.key";

/// Where servers before the settings file kept the key.
const OLD_FILE: &str = "config.json";

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("write {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{} holds {len} bytes, not a 32-byte key", path.display())]
    Length { path: PathBuf, len: usize },
    #[error("{} is not the old key file", path.display())]
    OldFile {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error(
        "{} and {} hold different keys; keep the one that opens the database's feeder keys and remove the other",
        key.display(),
        old.display()
    )]
    Conflict { key: PathBuf, old: PathBuf },
    #[error(
        "{} exists but its key {} does not; restore the key file beside it (a new key could not open its feeder keys)",
        database.display(),
        key.display()
    )]
    Missing { database: PathBuf, key: PathBuf },
    #[error("make a new key")]
    Random(#[source] wellfed_service::Error),
}

/// `config.json` as servers before the settings file wrote it.
#[derive(Deserialize)]
struct OldFile {
    key: Hex<32>,
}

/// What [`sealing_key`] did to find the key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeySource {
    /// Read from `keys/server.key`.
    Read,
    /// Moved from `config.json`, which is now gone.
    Moved,
    /// Made, on a server's first start.
    Made,
}

/// The sealing key for the data directory `data`: read, moved from
/// `config.json`, or made with `random` on the first start.
pub fn sealing_key(data: &Path, random: &dyn Random) -> Result<([u8; 32], KeySource), KeyError> {
    let dir = data.join(KEY_DIR);
    let path = dir.join(KEY_FILE);
    let old_path = data.join(OLD_FILE);
    let old = read_old(&old_path)?;

    if let Some(key) = read_key(&path)? {
        if let Some(old) = old {
            if old != key {
                return Err(KeyError::Conflict {
                    key: path,
                    old: old_path,
                });
            }
            remove_old(&old_path, data)?;
            tracing::info!(
                "removed {}, whose key {} already holds",
                old_path.display(),
                path.display()
            );
        }
        return Ok((key, KeySource::Read));
    }

    let (key, source) = match old {
        Some(key) => (key, KeySource::Moved),
        None => {
            let database = data.join(DATABASE);
            let exists = std::fs::exists(&database).map_err(|source| KeyError::Read {
                path: database.clone(),
                source,
            })?;
            if exists {
                return Err(KeyError::Missing {
                    database,
                    key: path,
                });
            }
            let mut key = [0; 32];
            random.fill(&mut key).map_err(KeyError::Random)?;
            (key, KeySource::Made)
        }
    };

    write_key(&dir, &path, &key)?;
    if source == KeySource::Moved {
        remove_old(&old_path, data)?;
        tracing::info!(
            "moved the sealing key from {} to {} and deleted {0}",
            old_path.display(),
            path.display()
        );
    } else {
        tracing::info!("made a new sealing key in {}", path.display());
    }
    Ok((key, source))
}

fn read_key(path: &Path) -> Result<Option<[u8; 32]>, KeyError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(KeyError::Read {
                path: path.to_owned(),
                source,
            });
        }
    };

    let len = bytes.len();
    let key = <[u8; 32]>::try_from(bytes).map_err(|_| KeyError::Length {
        path: path.to_owned(),
        len,
    })?;
    Ok(Some(key))
}

fn read_old(path: &Path) -> Result<Option<[u8; 32]>, KeyError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(KeyError::Read {
                path: path.to_owned(),
                source,
            });
        }
    };

    let file: OldFile = serde_json::from_slice(&bytes).map_err(|source| KeyError::OldFile {
        path: path.to_owned(),
        source,
    })?;
    Ok(Some(file.key.0))
}

/// Write the key beside its final name and rename it into place, so a
/// crash leaves either no key file or a whole one, never a short one.
fn write_key(dir: &Path, path: &Path, key: &[u8; 32]) -> Result<(), KeyError> {
    let failed = |path: &Path| {
        let path = path.to_owned();
        move |source| KeyError::Write { path, source }
    };
    let staged = path.with_extension("key.new");

    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir).map_err(failed(dir))?;

    // A staged file from an interrupted start may have been made with other
    // permissions; `mode` applies only to a file this call creates.
    match std::fs::remove_file(&staged) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => return Err(failed(&staged)(source)),
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&staged).map_err(failed(&staged))?;
    file.write_all(key)
        .and_then(|()| file.sync_all())
        .map_err(failed(&staged))?;

    std::fs::rename(&staged, path).map_err(failed(path))?;
    sync_dir(dir).map_err(failed(dir))
}

fn remove_old(old: &Path, data: &Path) -> Result<(), KeyError> {
    std::fs::remove_file(old).map_err(|source| KeyError::Write {
        path: old.to_owned(),
        source,
    })?;
    sync_dir(data).map_err(|source| KeyError::Write {
        path: data.to_owned(),
        source,
    })
}

/// Make a rename or removal in `dir` durable.
fn sync_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    // Elsewhere a directory cannot be opened to sync; the rename is as
    // durable as the platform makes it.
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}
