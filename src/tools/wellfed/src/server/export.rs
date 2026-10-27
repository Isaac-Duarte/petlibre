//! The server's archive, saved to a file as it arrives. Owners only.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use futures_util::TryStreamExt;
use wellfed_server_client::Client;
use wellfed_server_client::api::ExportFormat;
use serde::Serialize;

use super::{Output, Settled};

#[derive(Args)]
pub(crate) struct ExportArgs {
    /// Where to save the archive (a tar). It appears only once whole.
    file: PathBuf,
    /// Write the tables as CSV for a spreadsheet; only the JSON archive
    /// imports.
    #[arg(long)]
    csv: bool,
}

/// What `--json` prints once the archive is saved.
#[derive(Serialize)]
struct Saved<'a> {
    file: &'a Path,
    bytes: u64,
}

pub(super) async fn export(client: &Client, args: ExportArgs, out: Output) -> Result<Settled> {
    let format = if args.csv {
        ExportFormat::Csv
    } else {
        ExportFormat::Json
    };
    let mut chunks = client.export(format).await?;

    // Written beside the file and renamed once whole, so a cut export never
    // passes for a whole one.
    let mut partial = args.file.clone().into_os_string();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    let mut file =
        std::fs::File::create(&partial).with_context(|| format!("create {}", partial.display()))?;
    let mut bytes = 0;
    while let Some(chunk) = chunks
        .try_next()
        .await
        .context("the export stopped before its end")?
    {
        file.write_all(&chunk)
            .with_context(|| format!("write {}", partial.display()))?;
        bytes += chunk.len() as u64;
    }
    file.sync_all()
        .with_context(|| format!("sync {}", partial.display()))?;
    std::fs::rename(&partial, &args.file)
        .with_context(|| format!("rename {} to {}", partial.display(), args.file.display()))?;

    let saved = Saved {
        file: &args.file,
        bytes,
    };
    out.show(&saved, |saved| {
        format!("Saved {} ({} bytes)", saved.file.display(), saved.bytes)
    })?;
    Ok(Settled::Known)
}
