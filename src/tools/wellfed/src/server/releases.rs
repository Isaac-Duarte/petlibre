//! The releases the server keeps for installs: the list, an upload and a
//! check of the project's releases. Owners only.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Subcommand;
use wellfed_server_client::api::ReleaseQuery;
use wellfed_server_client::{Client, Uploaded};

use super::{Output, Settled, text};

#[derive(Subcommand)]
pub(crate) enum Releases {
    /// The verified releases, newest version first.
    List {
        /// The previous page's `next`, as it came.
        #[arg(long)]
        after: Option<String>,
        /// At most this many (1-1000, default 100).
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Upload a release file; the server verifies it before keeping it.
    Upload { file: PathBuf },
    /// Check the project's releases now (when fetching is on), keeping
    /// each new one that verifies.
    Check,
}

pub(super) async fn releases(client: &Client, releases: Releases, out: Output) -> Result<Settled> {
    match releases {
        Releases::List { after, limit } => {
            let page = client.releases(&ReleaseQuery { after, limit }).await?;
            out.show(&page, |page| text::lines(&page.items, text::release))?;
            if let Some(next) = &page.next {
                eprintln!("More after {next}: --after {next}");
            }
        }
        Releases::Upload { file } => {
            let bytes = std::fs::read(&file).with_context(|| format!("read {}", file.display()))?;
            // Uploading the same file twice is harmless, so it needs no key.
            match client.upload_release(bytes).await? {
                Uploaded::New(release) => {
                    out.show(&release, |r| format!("Kept {}", text::release(r)))?;
                }
                Uploaded::Known(release) => {
                    out.show(&release, |r| format!("Already kept: {}", text::release(r)))?;
                }
            }
        }
        Releases::Check => {
            let check = client.check_releases().await?;
            out.show(&check, |check| {
                let mut lines: Vec<String> = check
                    .added
                    .iter()
                    .map(|r| format!("Added {}", text::release(r)))
                    .collect();
                lines.extend(check.refused.iter().map(|asset| {
                    format!(
                        "Refused {} {}: {}",
                        asset.tag,
                        asset.asset,
                        text::word(&asset.reason)
                    )
                }));
                lines.extend(
                    check
                        .failed
                        .iter()
                        .map(|asset| format!("Not fetched: {} {}", asset.tag, asset.asset)),
                );
                if lines.is_empty() {
                    lines.push("Nothing new.".to_owned());
                }
                lines.join("\n")
            })?;
        }
    }
    Ok(Settled::Known)
}
