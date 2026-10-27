//! `install`: an owner's install of a kept release on one feeder, followed
//! on the stream to its end; without a release, where the feeder's running
//! or last install stands.
//!
//! An install is not a keyed write: the server runs one at a time and
//! refuses another while it runs, and the feeder reports what it already
//! runs, so nothing installs twice. A failed install exits 1 with the
//! server's reason and is not started again.

use anyhow::{Result, bail};
use clap::Args;
use wellfed_server_client::api::{Hex, Install, InstallState, StreamEvent, StreamQuery};
use wellfed_server_client::{Client, Updates};

use super::feeders::Which;
use super::{Output, Settled, text};

#[derive(Args)]
pub(crate) struct InstallArgs {
    #[command(flatten)]
    which: Which,
    /// The release's id, as `releases list` shows it. Without one, the
    /// feeder's running or last install.
    #[arg(value_parser = release_id)]
    release: Option<Hex<32>>,
}

pub(super) async fn install(client: &Client, args: InstallArgs, out: Output) -> Result<Settled> {
    let feeder = args.which.id(client).await?;
    let Some(release) = args.release else {
        let install = client.install_state(feeder).await?;
        out.show(&install, text::install)?;
        return Ok(Settled::Known);
    };

    // Opened first, so the end cannot slip past.
    let mut updates = client.stream(&StreamQuery::default(), None).await?;
    let started = client.install(feeder, release).await?;
    out.show(&started, text::install)?;

    let ended = match follow(&mut updates, &started, out).await? {
        Some(ended) => ended,
        // The stream ended first: read where it stands.
        None => client.install_state(feeder).await?,
    };
    if ended.id != started.id || ended.state == InstallState::Running {
        out.show(&ended, text::install)?;
        bail!("the stream ended while the install ran; `install` without a release reads it");
    }
    match ended.failure {
        None => Ok(Settled::Known),
        Some(failure) => bail!("the install failed: {}", text::word(&failure)),
    }
}

/// Follow `started` on the stream, showing each change, until it ends;
/// `None` if the stream ends first.
pub(super) async fn follow(
    updates: &mut Updates,
    started: &Install,
    out: Output,
) -> Result<Option<Install>> {
    while let Some(update) = updates.next().await {
        let Ok(update) = update else {
            return Ok(None);
        };
        let StreamEvent::Install { install, .. } = update.event else {
            continue;
        };
        if install.id != started.id || install == *started {
            continue;
        }
        out.show(&install, text::install)?;
        if install.state != InstallState::Running {
            return Ok(Some(install));
        }
    }
    Ok(None)
}

/// A release id as the API spells it: 64 lowercase hex digits.
fn release_id(text: &str) -> Result<Hex<32>, NotReleaseId> {
    wellfed_session::hex::decode_array(text.as_bytes())
        .map(Hex)
        .ok_or(NotReleaseId)
}

#[derive(Debug, thiserror::Error)]
#[error("a release id is 64 lowercase hex digits, as `releases list` shows it")]
struct NotReleaseId;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_id_is_its_lowercase_hex() {
        let id = "ab".repeat(32);
        assert_eq!(release_id(&id).ok(), Some(Hex([0xab; 32])));
        assert!(release_id(&"AB".repeat(32)).is_err());
        assert!(release_id("ab").is_err());
    }
}
