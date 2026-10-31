//! What an install keeps, under the owner's backups directory: one
//! subdirectory per feeder (named by its identity) holding its stock backup
//! (`stock-1.1.15.bin`) and the promotion manifests the run signed, and beside
//! them `installing.json`, the record a resumed run finds its feeder by. The CLI's directory is `private/stock-backups`; the
//! app's is in its own storage.
//!
//! Nothing kept is ever replaced by a different copy: a backup a run read stays
//! as it was read, and a different read refuses.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use wellfed_client::install::{STOCK_VERSION, StockBackup};
use wellfed_formats::hex;

use crate::Error;

/// The directory under `backups` that holds what is kept for the feeder
/// `identity`.
pub fn feeder_directory(backups: &Path, identity: &[u8; 32]) -> PathBuf {
    backups.join(hex::encode(identity))
}

/// Where the feeder `identity`'s backup is kept under `backups`.
pub fn backup_path(backups: &Path, identity: &[u8; 32]) -> PathBuf {
    feeder_directory(backups, identity).join(format!("stock-{STOCK_VERSION}.bin"))
}

/// Where the install keeps the update manifest it signed for the feeder
/// `identity` at `version`, beside its backup: a run resumed after the
/// activation reads the version it waits for there, and a later run signs
/// above it.
pub fn promotion_path(backups: &Path, identity: &[u8; 32], version: u32) -> PathBuf {
    feeder_directory(backups, identity).join(format!("promotion-v{version}.manifest"))
}

/// Where a restore keeps the update key it made for the feeder `identity`,
/// only until the restore ends ([`crate::restore`]).
pub fn restore_key_path(backups: &Path, identity: &[u8; 32]) -> PathBuf {
    feeder_directory(backups, identity).join("restore-key.json")
}

/// The backup kept at `path`, checked again; `None` when there is none.
pub fn load_backup(path: &Path) -> Result<Option<StockBackup>, Error> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(Error::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let backup = StockBackup::open(bytes).map_err(|source| Error::BackupDamaged {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(Some(backup))
}

/// Keep `backup` at `path`. A copy already there stays: the same bytes are
/// kept as they are, and different ones refuse.
pub fn keep_backup(path: &Path, backup: &StockBackup) -> Result<(), Error> {
    if let Some(kept) = load_backup(path)? {
        return wellfed_formats::require(kept.bytes() == backup.bytes(), || Error::BackupExists {
            path: path.to_path_buf(),
        });
    }
    // Whole or not at all: a partial file would refuse every later run.
    write_new(path, backup.bytes())
}

/// Write `bytes` as a new file at `path`, whole or not at all, creating its
/// directory.
pub(crate) fn write_new(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory).map_err(|source| Error::Create {
            path: directory.to_path_buf(),
            source,
        })?;
    }
    wellfed_formats::private::write_new_whole(path, bytes).map_err(|source| Error::Write {
        path: path.to_path_buf(),
        source,
    })
}

/// Whether an earlier run paired with a feeder and did not reach the end, so
/// a new one may pick up from bank 0.
pub fn resumable(backups: &Path) -> bool {
    matches!(Paired::load(backups), Ok(Some(_)))
}

/// Remove the paired record whoever's it is: a platform that treats the
/// install as over though the run stopped short of its end.
pub fn forget_paired(backups: &Path) -> Result<(), Error> {
    let path = backups.join(Paired::FILE);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(Error::Remove { path, source }),
    }
}

/// The checked copy of stock kept for `identity`, if there is one.
pub fn kept_backup(backups: &Path, identity: &[u8; 32]) -> Option<PathBuf> {
    let path = backup_path(backups, identity);
    path.exists().then_some(path)
}

/// The feeder an install paired with and where it answered, if it has yet,
/// kept beside the backups (`installing.json`) as soon as pairing succeeds,
/// so a resumed run finds it with no image lease left. It names the stock device this install enrolled (trust on
/// first use), and only a run of that same install takes it: the file is
/// shared by every feeder the backups hold. Its identity is the one the
/// feeder proved. The run that reaches the end removes it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Paired {
    /// Stock's device ID, as the installer's broker enrolled it.
    pub(crate) device: String,
    pub(crate) identity: wellfed_model::Hex<32>,
    /// Where it last answered; `None` while it restarts onto the owner's
    /// Wi-Fi after Bluetooth setup.
    pub(crate) address: Option<SocketAddr>,
}

impl Paired {
    const FILE: &str = "installing.json";

    pub(crate) fn load(backups: &Path) -> Result<Option<Self>, Error> {
        let path = backups.join(Self::FILE);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(Error::Read { path, source }),
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|source| Error::Parse { path, source })
    }

    pub(crate) fn keep(&self, backups: &Path) -> Result<(), Error> {
        std::fs::create_dir_all(backups).map_err(|source| Error::Create {
            path: backups.to_path_buf(),
            source,
        })?;
        let path = backups.join(Self::FILE);
        wellfed_formats::private::write_json(&path, self)
            .map_err(|source| Error::Write { path, source })
    }

    /// Remove the record when it is `device`'s: that install is done.
    pub(crate) fn clear(backups: &Path, device: &str) -> Result<(), Error> {
        if Self::load(backups)?.is_some_and(|record| record.device == device) {
            let path = backups.join(Self::FILE);
            std::fs::remove_file(&path).map_err(|source| Error::Remove { path, source })?;
        }
        Ok(())
    }
}

/// A feeder's proven session answered with this build: the evidence that closes
/// the stock trial that installed it, kept with the feeder because a trial can
/// outlive the run that saw the answer. It names the trial it is about, so a
/// later trial never closes on it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Answered {
    /// Stock's device ID, as the installer's broker enrolled it.
    pub(crate) device: String,
    pub(crate) build: String,
    /// The device's latest trial when the answer came; `None` with none yet.
    pub(crate) sequence: Option<usize>,
}

impl Answered {
    const FILE: &str = "answered.json";

    pub(crate) fn load(backups: &Path, identity: &[u8; 32]) -> Result<Option<Self>, Error> {
        let path = feeder_directory(backups, identity).join(Self::FILE);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(Error::Read { path, source }),
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|source| Error::Parse { path, source })
    }

    pub(crate) fn keep(&self, backups: &Path, identity: &[u8; 32]) -> Result<(), Error> {
        let directory = feeder_directory(backups, identity);
        std::fs::create_dir_all(&directory).map_err(|source| Error::Create {
            path: directory.clone(),
            source,
        })?;
        let path = directory.join(Self::FILE);
        wellfed_formats::private::write_json(&path, self)
            .map_err(|source| Error::Write { path, source })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A backup is kept per feeder and never replaced by a different copy;
    /// an unreadable or foreign file there is refused, not trusted.
    #[test]
    fn a_kept_backup_is_never_replaced_and_a_foreign_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let at = backup_path(dir.path(), &[0xab; 32]);
        assert!(at.ends_with(format!("{}/stock-1.1.15.bin", "ab".repeat(32))));
        assert!(load_backup(&at).unwrap().is_none());

        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(&at, b"not stock").unwrap();
        assert!(matches!(load_backup(&at), Err(Error::BackupDamaged { .. })));
    }

    /// The record round-trips, and only its own install's end clears it.
    #[test]
    fn the_record_is_cleared_only_by_its_own_install() {
        let dir = tempfile::tempdir().unwrap();
        let backups = dir.path().join("stock-backups");
        assert_eq!(Paired::load(&backups).unwrap(), None);

        let recorded = Paired {
            device: "AF_A".into(),
            identity: wellfed_model::Hex([0xab; 32]),
            address: Some("192.168.4.20:7100".parse().unwrap()),
        };
        recorded.keep(&backups).unwrap();
        assert_eq!(Paired::load(&backups).unwrap(), Some(recorded.clone()));

        Paired::clear(&backups, "AF_B").unwrap();
        assert_eq!(Paired::load(&backups).unwrap(), Some(recorded));
        Paired::clear(&backups, "AF_A").unwrap();
        assert_eq!(Paired::load(&backups).unwrap(), None);
    }
}
