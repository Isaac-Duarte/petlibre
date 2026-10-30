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

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use serde::Serialize;
use wellfed_client::install::{STOCK_VERSION, StockBackup};
use wellfed_client::update::Image;
use wellfed_formats::canonical::sha256;
use wellfed_install::push::{Pushed, push};
use wellfed_install::storage::{backup_path, load_backup};
use wellfed_model::{Answer, Op};
use wellfed_signing::{Frozen, Key, Motion};

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
    /// The update-signing key the feeder trusts.
    #[arg(long, default_value = wellfed_signing::KEY_PATH)]
    key: PathBuf,
    /// The version the stock manifest carries.
    #[arg(long)]
    version: u32,
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

    // Signed as the stock image: only the pinned container passes.
    let frozen = Frozen::stock(backup.bytes().to_vec()).context("sign the backup as stock")?;
    let key = Key::load(&args.key).with_context(|| format!("load {}", args.key.display()))?;
    let manifest = frozen.offer(&key, args.version, Motion::Refused)?;

    let (_, _, mut connection) = connect(&args.target).await?;
    if !connection.capabilities().update {
        bail!("this build takes no updates over the device API");
    }
    let floor = match connection.ask(&Op::UpdateState).await? {
        Answer::Update(view) => view.min_version,
        other => bail!("the feeder answered {other:?} to an update state request"),
    };
    let downgrade = key.downgrade(&manifest, &identity, floor)?;
    drop(connection);

    eprintln!(
        "Restoring stock {STOCK_VERSION} as version {} (a downgrade at floor {floor}).",
        args.version
    );
    let address = crate::device::locate(&identity, args.target.address.as_deref())?;
    let image = Image {
        manifest: &manifest,
        downgrade: Some(&downgrade),
        payload: backup.payload(),
    };
    let keypair = key_file.keypair();
    let pushed = push(
        image,
        async || {
            Ok(wellfed_connect::Connection::open(
                keypair,
                identity,
                address,
                wellfed_install::session::random()?,
            )
            .await?)
        },
        // Stock serves no device API: a session that opens is never it.
        |_, _| false,
        &Printer,
    )
    .await?;

    match pushed {
        Pushed::Activated | Pushed::AlreadyRunning => {
            eprintln!(
                "Activated: the feeder restarts into stock {STOCK_VERSION} from bank 1, with \
                 its own settings. Stock serves no device API: check it in stock's app, or \
                 on the broker it was provisioned to."
            );
            Ok(())
        }
        Pushed::Failed(error) => bail!(
            "the restore stopped: {error}; the offer is not sent again. The feeder keeps \
             running the replacement from bank 0 (bank 1 may be partly written)."
        ),
        Pushed::Gone { offered: true } => bail!(
            "uncertain: the feeder stopped answering after the offer went out. It may be \
             running stock already, or still the replacement; nothing is sent again. Look at \
             the feeder (stock's app, or a session with this computer's key) before trying \
             anything else."
        ),
        Pushed::Gone { offered: false } => {
            bail!("the feeder did not answer; nothing was offered")
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
