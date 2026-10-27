//! The feeders' stock backups: for each feeder, the verified copy of its
//! own stock firmware, kept as a file in the data directory and never in
//! the database, the same way the owner's CLI keeps it
//! (`stock/<identity>/stock-1.1.15.bin`, its record beside it).
//!
//! A copy is checked against the pins before anything is written, whether
//! the server read bank 1 from the feeder or an owner uploaded it, and
//! checked again on every read, so a file changed on disk is reported
//! damaged and is neither exported nor restored. A kept copy is never
//! replaced: the same bytes again are the copy already kept, and different
//! ones are refused. Writes land whole, file before record, so a crash
//! leaves at worst a file with no record, which the next write over it
//! replaces.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use wellfed_api::{StockBackup as StockView, StockBackupState, StockSource};
use wellfed_client::install::{STOCK_VERSION, StockBackup, StockPins};
use wellfed_model::Hex;
use serde::{Deserialize, Serialize};

use crate::cache::{blocking, cache, sync_dir, write_whole};
use crate::{Added, Error};

/// Where the backups are kept, and what a copy must match.
#[derive(Clone, Debug)]
pub struct StockConfig {
    /// `stock/` in the data directory.
    pub dir: PathBuf,
    /// [`StockPins::STOCK`] on every real server; tests, which cannot hold
    /// the vendor's image, check a synthetic one.
    pub pins: StockPins,
}

/// A copy of stock as it reached the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Taken {
    /// Bank 1 as the feeder's bank-0 role streamed it.
    Bank(Vec<u8>),
    /// A file an owner uploaded: the vendor's container, or its payload.
    Upload(Vec<u8>),
}

/// The record beside a kept file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    sha256: Hex<32>,
    bytes: u64,
    source: StockSource,
    kept: Timestamp,
}

/// The feeders' stock backups. One per server.
#[derive(Debug)]
pub struct Stock {
    dir: PathBuf,
    pins: StockPins,
    /// One writer at a time, so two copies for one feeder cannot both find
    /// none kept.
    writing: tokio::sync::Mutex<()>,
}

impl Stock {
    pub(crate) async fn open(config: StockConfig) -> Result<Self, Error> {
        let dir = config.dir.clone();
        blocking(move || fs::create_dir_all(&dir).map_err(cache(&dir))).await?;
        Ok(Self {
            dir: config.dir,
            pins: config.pins,
            writing: tokio::sync::Mutex::new(()),
        })
    }

    /// The copy kept for the feeder `identity`, checked again: `None`
    /// when there is none.
    pub async fn view(&self, identity: [u8; 32]) -> Result<Option<StockView>, Error> {
        let (dir, pins) = (self.dir.clone(), self.pins);
        blocking(move || {
            let Some((record, file)) = read(&Paths::new(&dir, &identity))? else {
                return Ok(None);
            };
            let verified = StockBackup::open_pinned(file, &pins)
                .is_ok_and(|backup| Hex(backup.sha256()) == record.sha256);
            let state = if verified {
                StockBackupState::Verified
            } else {
                StockBackupState::Damaged
            };
            Ok(Some(view(&record, state)))
        })
        .await
    }

    /// The copy kept for `identity`, checked again: what an export sends
    /// and a restore installs.
    pub async fn backup(&self, identity: [u8; 32]) -> Result<(StockView, StockBackup), Error> {
        let (dir, pins) = (self.dir.clone(), self.pins);
        blocking(move || {
            let (record, file) = read(&Paths::new(&dir, &identity))?.ok_or(Error::NoStock)?;
            let backup = StockBackup::open_pinned(file, &pins).map_err(|_| Error::StockDamaged)?;
            if Hex(backup.sha256()) != record.sha256 {
                return Err(Error::StockDamaged);
            }
            Ok((view(&record, StockBackupState::Verified), backup))
        })
        .await
    }

    /// Check `taken` against the pins and keep it for `identity`, unless
    /// the same copy is kept already. A different copy is refused.
    pub async fn keep(
        &self,
        identity: [u8; 32],
        taken: Taken,
        now: Timestamp,
    ) -> Result<(StockView, Added), Error> {
        let _writing = self.writing.lock().await;
        let (dir, pins) = (self.dir.clone(), self.pins);

        blocking(move || {
            let (backup, source) = match taken {
                Taken::Bank(bank) => (
                    StockBackup::from_bank_pinned(bank, &pins)?,
                    StockSource::Feeder,
                ),
                Taken::Upload(image) => {
                    (StockBackup::open_pinned(image, &pins)?, StockSource::Upload)
                }
            };
            let sha256 = Hex(backup.sha256());
            let paths = Paths::new(&dir, &identity);
            if let Some((record, file)) = read(&paths)? {
                if record.sha256 != sha256 {
                    return Err(Error::StockDiffers);
                }
                // The record's own copy again: a kept file damaged on disk
                // since is written back whole.
                if file != backup.bytes() {
                    tracing::warn!("{} was damaged; writing it back", paths.file.display());
                    write_whole(&paths.file, backup.bytes())?;
                    sync_dir(&paths.feeder)?;
                }
                return Ok((view(&record, StockBackupState::Verified), Added::Known));
            }

            let record = Record {
                sha256,
                bytes: u64::try_from(backup.bytes().len()).unwrap_or(u64::MAX),
                source,
                kept: now,
            };
            fs::create_dir_all(&paths.feeder).map_err(cache(&paths.feeder))?;
            let text = serde_json::to_vec_pretty(&record).map_err(|source| Error::Entry {
                path: paths.record.clone(),
                source,
            })?;
            write_whole(&paths.file, backup.bytes())?;
            write_whole(&paths.record, &text)?;
            sync_dir(&paths.feeder)?;
            Ok((view(&record, StockBackupState::Verified), Added::New))
        })
        .await
    }
}

fn view(record: &Record, state: StockBackupState) -> StockView {
    StockView {
        version: STOCK_VERSION.into(),
        state,
        sha256: record.sha256,
        bytes: record.bytes,
        source: record.source,
        kept: record.kept,
    }
}

/// One feeder's directory, its kept file and the record beside it.
struct Paths {
    feeder: PathBuf,
    file: PathBuf,
    record: PathBuf,
}

impl Paths {
    fn new(dir: &Path, identity: &[u8; 32]) -> Self {
        let feeder = dir.join(Hex(*identity).to_string());
        let name = format!("stock-{STOCK_VERSION}");
        Self {
            file: feeder.join(format!("{name}.bin")),
            record: feeder.join(format!("{name}.json")),
            feeder,
        }
    }
}

/// The record and file kept for one feeder; `None` when either is
/// missing. A file without its record is a write a crash cut short, and
/// counts as none.
fn read(paths: &Paths) -> Result<Option<(Record, Vec<u8>)>, Error> {
    let Paths { file, record, .. } = paths;
    let text = match fs::read(record) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(cache(record)(source)),
    };
    let parsed: Record = serde_json::from_slice(&text).map_err(|source| Error::Entry {
        path: record.clone(),
        source,
    })?;
    let bytes = match fs::read(file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            tracing::warn!("{} has no stock file beside it", record.display());
            return Ok(None);
        }
        Err(source) => return Err(cache(file)(source)),
    };
    Ok(Some((parsed, bytes)))
}
