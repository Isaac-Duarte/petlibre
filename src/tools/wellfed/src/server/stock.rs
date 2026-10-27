//! `stock`: the feeder's own stock firmware as the server keeps it, and
//! "Restore stock" ([rollback]).
//!
//! Without a verb it shows the kept backup, what the feeder says of its
//! banks and the last restore. `read` has the server read bank 1 from the
//! replacement on bank 0 (before promotion); `upload` gives it the copy
//! the installer kept; `export` copies the server's backup to a new file,
//! byte for byte.
//!
//! `restore` signs on this computer, as `wellfed restore-stock` does: the
//! server's copy is downloaded and checked against stock 1.1.15's pinned
//! image, signed as the stock image (only the pinned container signs) with
//! a downgrade statement bound to the feeder and the floor the server just
//! read, and handed to the server, which never holds the key. The restore
//! then runs on the server like an install, followed on the stream to its
//! end. It is not a keyed write: nothing is sent again, and a feeder that
//! stopped answering after the offer leaves the result uncertain (exit 1),
//! since stock serves no device API to ask.
//!
//! [rollback]: ../../../../../docs/firmware/consumer-installation.md#rollback

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use wellfed_client::install::{STOCK_VERSION, StockBackup};
use wellfed_formats::canonical::sha256;
use wellfed_server_client::api::{
    Hex, InstallFailure, InstallState, StockBackupState, StockRestore, StreamQuery,
};
use wellfed_server_client::{Client, Kept};
use wellfed_signing::{Frozen, Key, Motion};

use super::feeders::Which;
use super::install::follow;
use super::{Output, Settled, text};

#[derive(Args)]
pub(crate) struct StockArgs {
    #[command(flatten)]
    which: Which,
    #[command(subcommand)]
    verb: Option<StockVerb>,
}

#[derive(Subcommand)]
enum StockVerb {
    /// Have the server read bank 1 from the feeder and keep it, while the
    /// replacement runs from bank 0 with stock still in bank 1.
    Read,
    /// Give the server a copy of this feeder's stock (the installer's kept
    /// copy, or an export); it is checked before it is kept.
    Upload { file: PathBuf },
    /// Copy the server's backup, unchanged, to a new file.
    Export {
        /// A new file for the copy; an existing one is never replaced.
        #[arg(long)]
        output: PathBuf,
    },
    /// Sign the server's backup as the stock image and restore it on the
    /// feeder, following the restore to its end.
    Restore {
        /// The version the stock manifest carries.
        #[arg(long)]
        version: u32,
        /// The update-signing key the feeder trusts.
        #[arg(long, default_value = wellfed_signing::KEY_PATH)]
        key: PathBuf,
    },
}

pub(super) async fn stock(client: &Client, args: StockArgs, out: Output) -> Result<Settled> {
    let feeder = args.which.id(client).await?;
    match args.verb {
        None => {
            let state = client.stock(feeder).await?;
            out.show(&state, text::stock)?;
        }
        Some(StockVerb::Read) => {
            eprintln!("Reading bank 1 from the feeder (about 1.3 MB)...");
            show_kept(client.read_stock(feeder).await?, out)?;
        }
        Some(StockVerb::Upload { file }) => {
            let bytes = std::fs::read(&file).with_context(|| format!("read {}", file.display()))?;
            show_kept(client.upload_stock(feeder, bytes).await?, out)?;
        }
        Some(StockVerb::Export { output }) => {
            let (backup, bytes) = download(client, feeder).await?;
            wellfed_formats::private::write_new_whole(&output, &bytes)
                .with_context(|| format!("write {}", output.display()))?;
            out.show(&backup, |backup| {
                format!(
                    "Exported stock {} ({}) to {}",
                    backup.version,
                    backup.sha256,
                    output.display()
                )
            })?;
        }
        Some(StockVerb::Restore { version, key }) => {
            return restore(client, feeder, version, &key, out).await;
        }
    }
    Ok(Settled::Known)
}

fn show_kept(kept: Kept, out: Output) -> Result<()> {
    match kept {
        Kept::New(backup) => out.show(&backup, |b| format!("Kept {}", text::stock_backup(b))),
        Kept::Known(backup) => out.show(&backup, |b| {
            format!("Already kept: {}", text::stock_backup(b))
        }),
    }
}

/// The server's verified backup and its bytes, which must be the file the
/// server describes.
async fn download(
    client: &Client,
    feeder: wellfed_server_client::api::FeederId,
) -> Result<(wellfed_server_client::api::StockBackup, Vec<u8>)> {
    let state = client.stock(feeder).await?;
    let Some(backup) = state.backup else {
        bail!("the server keeps no stock backup for this feeder; `stock read` or `stock upload`");
    };
    if backup.state != StockBackupState::Verified {
        bail!("the server's stock backup no longer verifies (damaged on its disk)");
    }
    let bytes = client.stock_backup(feeder).await?;
    if sha256(&bytes) != backup.sha256.to_string() {
        bail!("the downloaded backup is not the file the server describes; nothing was written");
    }
    Ok((backup, bytes))
}

async fn restore(
    client: &Client,
    feeder: wellfed_server_client::api::FeederId,
    version: u32,
    key: &std::path::Path,
    out: Output,
) -> Result<Settled> {
    let identity = client.feeder(feeder).await?.feeder.identity;
    let state = client.stock(feeder).await?;
    let Some(floor) = state.feeder_state.map(|feeder| feeder.floor) else {
        bail!(
            "the server could not read the feeder's update floor: it is offline, or the \
             server's key is not an owner's on it"
        );
    };
    let (_, bytes) = download(client, feeder).await?;

    // Signed as the stock image: only the pinned container passes.
    let backup = StockBackup::open(bytes).with_context(|| {
        format!(
            "the server's copy is not stock {STOCK_VERSION}'s pinned image; nothing was signed \
             or sent"
        )
    })?;
    let frozen = Frozen::stock(backup.bytes().to_vec()).context("sign the backup as stock")?;
    let key = Key::load(key).with_context(|| format!("load {}", key.display()))?;
    let manifest = frozen.offer(&key, version, Motion::Refused)?;
    let downgrade = key.downgrade(&manifest, &identity.0, floor)?;
    let request = StockRestore {
        manifest: Hex(manifest),
        downgrade: Hex(downgrade
            .try_into()
            .ok()
            .context("a downgrade statement's length")?),
    };

    // Opened first, so the end cannot slip past.
    let mut updates = client.stream(&StreamQuery::default(), None).await?;
    eprintln!(
        "Restoring stock {STOCK_VERSION} as version {version} (a downgrade at floor {floor})."
    );
    let started = client.restore_stock(feeder, &request).await?;
    out.show(&started, text::install)?;

    let ended = match follow(&mut updates, &started, out).await? {
        Some(ended) => ended,
        None => client.stock(feeder).await?.restore.unwrap_or(started),
    };
    match (ended.state, ended.failure) {
        (InstallState::AlreadyRunning, _) => {
            eprintln!("The feeder already runs this stock image from bank 1: nothing was sent.");
            Ok(Settled::Known)
        }
        (InstallState::Activated, _) => {
            eprintln!(
                "Activated: the feeder restarts into stock {STOCK_VERSION} from bank 1, with its \
                 own settings. Stock serves no device API, so the server no longer reaches it."
            );
            Ok(Settled::Known)
        }
        (_, Some(InstallFailure::Unreachable { offered: true })) => bail!(
            "uncertain: the feeder stopped answering after the offer went out. It may run \
             stock already, or still the replacement; nothing is sent again. Look at the \
             feeder before trying anything else."
        ),
        (_, Some(failure)) => bail!(
            "the restore failed: {}; it is not sent again, and the feeder keeps running the \
             replacement",
            text::word(&failure)
        ),
        (_, None) => bail!("the stream ended while the restore ran; `stock` reads where it stands"),
    }
}
