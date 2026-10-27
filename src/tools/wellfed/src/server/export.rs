//! The server's archive, saved to a file as it arrives. Owners only.

use std::io::{Read, Seek, SeekFrom, Write};
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
    drop(file);
    whole(&partial)?;
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

/// Refuse an archive that does not end the way a whole tar does: its
/// blocks complete and the two empty ones last. A body that stopped short
/// without an error is caught here, and the partial file stays for a look.
fn whole(path: &Path) -> Result<()> {
    const BLOCK: u64 = 512;
    const END: usize = 1024;
    /// The same two blocks, as a seek back from the end.
    const BACK: i64 = -1024;

    let mut file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let size = file.metadata()?.len();
    let mut end = [0u8; END];
    let ended = size >= BLOCK * 2 && size % BLOCK == 0 && {
        file.seek(SeekFrom::End(BACK))?;
        file.read_exact(&mut end)?;
        end.iter().all(|&byte| byte == 0)
    };
    anyhow::ensure!(
        ended,
        "the export stopped before its end; {} is incomplete",
        path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_tar_ending_in_its_two_empty_blocks_is_whole() {
        let dir = tempfile::tempdir().unwrap();
        let file = |name: &str, bytes: &[u8]| {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        };
        let mut data = vec![7; 512];
        data.extend_from_slice(&[0; 1024]);

        assert!(whole(&file("whole.tar", &data)).is_ok());
        assert!(whole(&file("one-block.tar", &data[..1024])).is_err());
        assert!(whole(&file("ragged.tar", &data[..1500])).is_err());
        assert!(whole(&file("empty.tar", &[])).is_err());
    }
}
