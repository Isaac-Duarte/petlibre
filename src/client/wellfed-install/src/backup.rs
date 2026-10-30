//! The owner's copy of stock, read from the feeder ([rollback]): while the
//! replacement runs from bank 0 after stock installed it, bank 1 still holds
//! stock, and the bank-0 role streams it over the device API.
//! `wellfed_client::install` checks it against the pinned 1.1.15 payload and
//! rebuilds the vendor's container, which is kept per feeder
//! ([`crate::storage`]), never replaced by another copy.
//!
//! [rollback]: ../../../../docs/firmware/consumer-installation.md#rollback

use std::path::{Path, PathBuf};

use wellfed_client::install::{Bank1Reader, StockBackup};

use crate::event::Event;
use crate::platform::Report;
use crate::session::Session;
use crate::{Error, storage};

/// Report the read's progress every this many bytes.
const PROGRESS: usize = 256 * 1024;

/// The kept backup for `identity` under `backups`, or bank 1 read now from
/// the bank-0 role over `session` and kept: the copy the promotion waits
/// for.
pub async fn fetch(
    session: &mut impl Session,
    backups: &Path,
    identity: &[u8; 32],
    report: &impl Report,
) -> Result<(PathBuf, StockBackup), Error> {
    let path = storage::backup_path(backups, identity);
    if let Some(kept) = storage::load_backup(&path)? {
        report.event(Event::BackupFound { path: path.clone() });
        return Ok((path, kept));
    }
    // A feeder that predates the op would end the session on it.
    wellfed_formats::require(session.capabilities().backup, || Error::NoBackupServed)?;

    report.event(Event::ReadingBank1 {
        identity: *identity,
    });
    let mut reader = Bank1Reader::new();
    let mut shown = 0;
    let bank = loop {
        let answer = session
            .ask(&reader.next_op())
            .await
            .map_err(Error::Platform)?;
        if let Some(bank) = reader.take(answer).map_err(Error::Bank1)? {
            break bank;
        }
        if reader.read() / PROGRESS != shown {
            shown = reader.read() / PROGRESS;
            report.event(Event::Bank1Read(reader.read()));
        }
    };
    let backup = StockBackup::from_bank(bank).map_err(Error::NotStock)?;
    storage::keep_backup(&path, &backup)?;
    report.event(Event::BackupKept { path: path.clone() });
    Ok((path, backup))
}
