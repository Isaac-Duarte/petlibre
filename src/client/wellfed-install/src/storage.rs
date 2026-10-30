//! What an install keeps, under the owner's backups directory: one
//! subdirectory per feeder (named by its identity) holding its stock backup
//! (`stock-1.1.15.bin`), stock's plan sectors, the generation their put went
//! against and the promotion manifests the run signed, and beside them `installing.json`, the record a resumed run
//! finds its feeder by. The CLI's directory is `private/stock-backups`; the
//! app's is in its own storage.
//!
//! Nothing kept is ever replaced by a different copy: a backup or plans a
//! run read stay as they were read, and a different read refuses.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use wellfed_client::install::{STOCK_VERSION, StockBackup};
use wellfed_formats::hex;
use wellfed_model::Generation;

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

/// Where the install keeps stock's plan sectors, beside the feeder's
/// backup: the count byte and entries as the feeder served them
/// (`wellfed_client::install::plans::kept`), which `wellfed-stock plans`
/// reads again.
pub fn plans_path(backups: &Path, identity: &[u8; 32]) -> PathBuf {
    feeder_directory(backups, identity).join("stock-plans.bin")
}

/// Where the install keeps the generation its put of stock's plans goes against,
/// beside the plans it puts ([`keep_put_base`]).
pub fn put_base_path(backups: &Path, identity: &[u8; 32]) -> PathBuf {
    feeder_directory(backups, identity).join("stock-plans.base.json")
}

/// Keep the generation a put goes against, on disk before that put leaves: the
/// file and its directory are synced before this returns, so with no file here
/// no put ever left. A later run compares it with the table's generation, the
/// only proof that neither that put nor the owner has changed the table. A base
/// already here stays: the steps only go again from the same one, and a
/// different one refuses, so nothing is sent on a wrong record.
pub fn keep_put_base(path: &Path, base: Generation) -> Result<(), Error> {
    if let Some(kept) = kept_put_base(path)? {
        return wellfed_formats::require(kept == base, || Error::PutBaseExists {
            path: path.to_path_buf(),
            kept,
            base,
        });
    }
    let bytes = serde_json::to_vec(&base).map_err(|source| Error::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    write_new(path, &bytes)
}

/// The generation an earlier put went against, if one may have left.
pub(crate) fn kept_put_base(path: &Path) -> Result<Option<Generation>, Error> {
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
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|source| Error::Parse {
            path: path.to_path_buf(),
            source,
        })
}

/// The install reached its end: no put is left to judge.
pub(crate) fn forget_put_base(path: &Path) -> Result<(), Error> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(Error::Remove {
            path: path.to_path_buf(),
            source,
        }),
    }
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

/// Keep stock's plan bytes at `path`. As with the backup, a copy already
/// there stays: the same bytes are kept as they are, and different ones
/// refuse rather than replace what an earlier run read.
pub fn keep_plans(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    match std::fs::read(path) {
        Ok(kept) => {
            return wellfed_formats::require(kept == bytes, || Error::PlansExist {
                path: path.to_path_buf(),
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(Error::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    }
    write_new(path, bytes)
}

/// The plans an earlier run kept at `path`, if any.
pub(crate) fn kept_plans(path: &Path) -> Result<Option<Vec<u8>>, Error> {
    match std::fs::read(path) {
        Ok(kept) => Ok(Some(kept)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(Error::Read {
            path: path.to_path_buf(),
            source,
        }),
    }
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

/// Rename the kept plans at `kept` to `stock-plans.applied.bin` beside it,
/// or to a name with `now_ms` when an earlier applied copy holds that name:
/// nothing kept is overwritten. `None` when nothing is kept there.
pub(crate) fn set_aside(kept: &Path, now_ms: u64) -> Result<Option<PathBuf>, Error> {
    if !kept.exists() {
        return Ok(None);
    }
    let mut applied = kept.with_file_name("stock-plans.applied.bin");
    if applied.exists() {
        applied = kept.with_file_name(format!("stock-plans.applied-{now_ms}.bin"));
    }
    std::fs::rename(kept, &applied).map_err(|source| Error::SetAside {
        path: kept.to_path_buf(),
        source,
    })?;
    Ok(Some(applied))
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

    /// Stock's plans are kept beside the backup, and never replaced by a
    /// different read.
    #[test]
    fn kept_plans_sit_beside_the_backup_and_are_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let at = plans_path(dir.path(), &[0xab; 32]);
        assert_eq!(at.parent(), backup_path(dir.path(), &[0xab; 32]).parent());

        keep_plans(&at, &[1, 2, 3]).unwrap();
        keep_plans(&at, &[1, 2, 3]).unwrap();
        assert!(matches!(
            keep_plans(&at, &[1, 2, 4]),
            Err(Error::PlansExist { .. })
        ));
        assert_eq!(std::fs::read(&at).unwrap(), [1, 2, 3]);
    }

    /// A put's base stays as it was kept until the install's end. No file is the
    /// only answer that says no put left: a damaged one is an error, which a run
    /// treats as one that may have.
    #[test]
    fn a_puts_base_is_kept_once_and_forgotten_at_the_end() {
        let dir = tempfile::tempdir().unwrap();
        let kept = put_base_path(dir.path(), &[0xab; 32]);
        assert_eq!(kept_put_base(&kept).unwrap(), None);

        keep_put_base(&kept, Generation(4)).unwrap();
        assert_eq!(kept_put_base(&kept).unwrap(), Some(Generation(4)));
        keep_put_base(&kept, Generation(4)).unwrap();
        assert!(matches!(
            keep_put_base(&kept, Generation(5)),
            Err(Error::PutBaseExists { .. })
        ));
        assert_eq!(kept_put_base(&kept).unwrap(), Some(Generation(4)));

        std::fs::write(&kept, b"not a generation").unwrap();
        assert!(kept_put_base(&kept).is_err());

        forget_put_base(&kept).unwrap();
        forget_put_base(&kept).unwrap();
        assert_eq!(kept_put_base(&kept).unwrap(), None);
    }

    /// An applied copy is set aside, never over an earlier one, so a
    /// resumed run finds no kept copy to apply again.
    #[test]
    fn an_applied_copy_is_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let kept = plans_path(dir.path(), &[0xab; 32]);
        assert_eq!(set_aside(&kept, 7).unwrap(), None);

        keep_plans(&kept, &[1]).unwrap();
        let first = set_aside(&kept, 7).unwrap().unwrap();
        assert!(first.ends_with("stock-plans.applied.bin"));
        assert!(!kept.exists());

        keep_plans(&kept, &[2]).unwrap();
        let second = set_aside(&kept, 7).unwrap().unwrap();
        assert!(second.ends_with("stock-plans.applied-7.bin"));
        assert_eq!(std::fs::read(first).unwrap(), [1]);
        assert_eq!(std::fs::read(second).unwrap(), [2]);
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
