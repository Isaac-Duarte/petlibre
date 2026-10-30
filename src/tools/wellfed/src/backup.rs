//! The owner's copy of stock ([rollback]): `wellfed backup read`, `show`
//! and `export`, and `wellfed restore-stock`.
//!
//! The copy comes from the feeder: while the replacement runs from bank 0
//! after stock installed it, bank 1 still holds stock, and the bank-0
//! role streams it over the device API (`wellfed_install::backup`, which
//! the install runs too). It is kept per feeder under the owner's data
//! directory (`private/stock-backups/<identity>/stock-1.1.15.bin` by
//! default), never replaced by another copy.
//!
//! "Restore stock" signs that container as the stock image (only the
//! pinned container signs) with a downgrade statement bound to the feeder
//! and its floor, and pushes it as any update goes: bank 1 gives way, bank
//! 0 writes stock into bank 1 and activates it. Within the push the shared
//! installer offers again where the update protocol needs it (after bank
//! 1 gives way, or on a new session while bank 0 is not yet installing),
//! but nothing re-offers once the push has returned: a refusal ends the
//! restore, and a feeder that stops answering after an offer went out
//! leaves the result uncertain, since stock serves no device API to ask.
//!
//! [rollback]: ../../../../docs/firmware/consumer-installation.md#rollback

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use serde::Serialize;
use wellfed_client::install::{STOCK_VERSION, StockBackup};
use wellfed_formats::canonical::sha256;
use wellfed_install::restore::{Plan, Platform, Restored, Signing, Target as RestoreTarget};
use wellfed_install::storage::{backup_path, load_backup};
use wellfed_install::{Discovery, Mdns};
use wellfed_signing::Key;

use crate::ask::Asker;
use crate::keys::OwnerKey;
use crate::report::Printer;
use crate::{Target, connect};

/// The owner's data directory for backups, one subdirectory per feeder.
pub(crate) const BACKUPS: &str = wellfed_formats::bench::STOCK_BACKUPS;

#[derive(Args)]
pub(crate) struct BackupArgs {
    #[command(subcommand)]
    command: BackupCommand,
}

#[derive(Subcommand)]
enum BackupCommand {
    /// Read bank 1 from the replacement on bank 0, check it against stock
    /// 1.1.15's pinned image and keep it.
    Read(ReadArgs),
    /// The kept backup: its stock version, digest and where it is.
    Show(Stored),
    /// Copy the kept backup, unchanged, to a new file.
    Export(ExportArgs),
}

#[derive(Args)]
struct ReadArgs {
    #[command(flatten)]
    target: Target,
    /// The owner's data directory for backups.
    #[arg(long, default_value = BACKUPS)]
    backups: PathBuf,
}

/// Which kept backup: the feeder's, as the key file knows it.
#[derive(Args)]
struct Stored {
    /// This computer's client key.
    #[arg(long, default_value = "private/wellfed-owner.json")]
    keys: PathBuf,
    /// The feeder's identity, as a hex prefix; needed when the key file
    /// knows more than one.
    #[arg(long)]
    feeder: Option<String>,
    /// The owner's data directory for backups.
    #[arg(long, default_value = BACKUPS)]
    backups: PathBuf,
}

#[derive(Args)]
struct ExportArgs {
    #[command(flatten)]
    stored: Stored,
    /// A new file for the copy; an existing one is never replaced.
    #[arg(long)]
    output: PathBuf,
}

#[derive(Args)]
pub(crate) struct RestoreArgs {
    #[command(flatten)]
    target: Target,
    /// The owner's data directory for backups.
    #[arg(long, default_value = BACKUPS)]
    backups: PathBuf,
    /// A stock 1.1.15 image to restore instead of the kept backup (an
    /// exported copy, say); it must match the pinned image exactly.
    #[arg(long)]
    backup: Option<PathBuf>,
    /// An update-signing key the feeder already trusts, such as the bench's.
    /// Otherwise a fresh key is made, enrolled with a Dispense press at the
    /// feeder, and removed when the restore ends.
    #[arg(long)]
    key: Option<PathBuf>,
    /// The version the stock manifest carries; otherwise one above the
    /// feeder's floor.
    #[arg(long)]
    version: Option<u32>,
}

/// What `show` prints.
#[derive(Serialize)]
struct Shown {
    path: PathBuf,
    stock_version: &'static str,
    verified: bool,
    sha256: String,
    bytes: usize,
}

pub(crate) async fn command(args: BackupArgs) -> Result<()> {
    match args.command {
        BackupCommand::Read(args) => {
            let (_, identity, mut connection) = connect(&args.target).await?;
            let (path, _) =
                wellfed_install::backup::fetch(&mut connection, &args.backups, &identity, &Printer)
                    .await?;
            show(&path)
        }
        BackupCommand::Show(stored) => show(&stored.path()?),
        BackupCommand::Export(args) => export(&args.stored.path()?, &args.output),
    }
}

impl Stored {
    /// The kept backup's path for the feeder the key file names.
    fn path(&self) -> Result<PathBuf> {
        let key = OwnerKey::open(&self.keys, None, OwnerKey::system())?;
        let identity = key.feeder(self.feeder.as_deref())?;
        Ok(backup_path(&self.backups, &identity))
    }
}

fn show(path: &Path) -> Result<()> {
    let Some(backup) = load_backup(path)? else {
        bail!("no stock backup kept at {}", path.display());
    };
    crate::print_json(&Shown {
        path: path.to_path_buf(),
        stock_version: STOCK_VERSION,
        verified: backup.verified().is_some(),
        sha256: sha256(backup.bytes()),
        bytes: backup.bytes().len(),
    })
}

/// Copy the kept backup to `output` byte for byte, checked again first.
fn export(path: &Path, output: &Path) -> Result<()> {
    let Some(backup) = load_backup(path)? else {
        bail!("no stock backup kept at {}", path.display());
    };
    wellfed_formats::private::write_new_whole(output, backup.bytes())
        .with_context(|| format!("write {}", output.display()))?;
    eprintln!(
        "Exported stock {STOCK_VERSION} ({}) to {}.",
        sha256(backup.bytes()),
        output.display()
    );
    Ok(())
}

/// `wellfed restore-stock`: install the backup over the replacement.
pub(crate) async fn restore(args: RestoreArgs) -> Result<()> {
    let key_file = OwnerKey::open(&args.target.keys, None, OwnerKey::system())?;
    let identity = key_file.feeder(args.target.feeder.as_deref())?;
    let backup = match &args.backup {
        Some(given) => {
            let bytes =
                std::fs::read(given).with_context(|| format!("read {}", given.display()))?;
            StockBackup::open(bytes).with_context(|| format!("{}", given.display()))?
        }
        None => {
            let kept = backup_path(&args.backups, &identity);
            load_backup(&kept)?.with_context(|| {
                format!(
                    "no stock backup kept at {}; pass --backup with a copy",
                    kept.display()
                )
            })?
        }
    };

    let signing = match &args.key {
        Some(file) => Signing::Given(Box::new(
            Key::load(file).with_context(|| format!("load {}", file.display()))?,
        )),
        None => {
            eprintln!(
                "This asks for a Dispense press at the feeder. If its lock light is on, hold \
                 Lock for 5 seconds first."
            );
            Signing::Fresh
        }
    };
    let address = args
        .target
        .address
        .as_deref()
        .map(wellfed_connect::parse_address)
        .transpose()?;
    let located = address.is_some();
    let plan = Plan::new(
        &backup,
        RestoreTarget {
            identity,
            address,
            backups: args.backups,
            signing,
            version: args.version,
        },
    )
    .context("sign the backup as stock")?;

    let discovery = if located { Finder::Given } else { Finder::Mdns };
    let platform = Platform {
        prompter: Asker::terminal_notices(),
        discovery,
        keypair: key_file.keypair().clone(),
        report: Printer,
    };
    match wellfed_install::restore::restore(plan, platform, crate::install::signalled()?).await? {
        Restored::Activated => {
            eprintln!(
                "Activated: the feeder restarts into stock {STOCK_VERSION} from bank 1, with its \
                 own settings. Stock serves no device API: check it in stock's app, or on the \
                 broker it was provisioned to. After a Reset install it still names this \
                 computer's broker and retries forever: hold Reset for about 5 seconds and set \
                 it up in the vendor's app. After a DNS install it returns to the vendor's cloud \
                 once the DNS override is gone."
            );
            Ok(())
        }
        Restored::Refused(error) => bail!(
            "the restore stopped: {error}; the offer is not sent again. The feeder keeps \
             running the replacement from bank 0 (bank 1 may be partly written)."
        ),
        Restored::Uncertain => bail!(
            "uncertain: the feeder stopped answering after the offer went out. It may be \
             running stock already, or still the replacement; nothing is sent again. Look at \
             the feeder (stock's app, or a session with this computer's key) before trying \
             anything else."
        ),
    }
}

/// Where a restore looks for the feeder once its address stops answering: an
/// address the owner gave is the only one.
enum Finder {
    Given,
    Mdns,
}

impl Discovery for Finder {
    async fn locate(&self, identity: [u8; 32]) -> Option<SocketAddr> {
        match self {
            Self::Given => None,
            Self::Mdns => Mdns.locate(identity).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A foreign file where the backup is kept is refused, not shown or
    /// exported.
    #[test]
    fn a_foreign_file_is_neither_shown_nor_exported() {
        let dir = tempfile::tempdir().unwrap();
        let at = backup_path(dir.path(), &[0xab; 32]);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(&at, b"not stock").unwrap();
        assert!(show(&at).is_err());
        let output = dir.path().join("exported.bin");
        assert!(export(&at, &output).is_err());
        assert!(!output.exists(), "nothing exported from a foreign file");
    }
}
