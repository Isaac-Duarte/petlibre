//! Becoming a feeder's owner: `setup` over BLE, and `recover` through the
//! feeder's recovery window on the LAN. Both run `wellfed_install`'s
//! pairing, the one the install and the Android app run, which shows the
//! one-time code the owner keys in on the feeder's buttons.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use wellfed_client::setup::Credentials;

use crate::ask::Asker;
use crate::ble::Link;
use crate::keys::{OwnerKey, fingerprint};
use crate::passphrase;

#[derive(Args)]
pub(crate) struct SetupArgs {
    /// The network the feeder should join; the passphrase is read from
    /// standard input. Without it the feeder keeps its Wi-Fi.
    #[arg(long)]
    ssid: Option<String>,
    /// How the feeder lists this computer (1-16 bytes); kept in the key file.
    #[arg(long)]
    label: Option<String>,
    /// This computer's client key, created on first use.
    #[arg(long, default_value = "private/wellfed-owner.json")]
    keys: PathBuf,
    /// The feeder's Bluetooth address; otherwise the first in setup mode.
    #[arg(long)]
    address: Option<bluer::Address>,
}

pub(crate) async fn setup(args: SetupArgs) -> Result<()> {
    let wifi = match args.ssid {
        Some(ssid) => Some(Credentials::new(ssid.into_bytes(), passphrase()?)?),
        None => None,
    };
    let mut key = OwnerKey::open(&args.keys, args.label.as_deref(), OwnerKey::system())?;
    let link = Link::open(args.address).await?;
    let mut asker = Asker::terminal_notices();
    let (identity, outcome) =
        wellfed_install::pairing::over_link(&mut key, wifi, link, &mut asker).await?;
    println!(
        "Paired with feeder {} ({outcome}); it reboots now without BLE.",
        fingerprint(&identity)
    );
    Ok(())
}

#[derive(Args)]
pub(crate) struct RecoverArgs {
    /// The feeder's address (ip or ip:port): recovery is for a client that
    /// knows no feeder, so there is no identity to look up over mDNS.
    #[arg(long)]
    address: String,
    /// This computer's client key, created on first use.
    #[arg(long, default_value = "private/wellfed-owner.json")]
    keys: PathBuf,
    #[arg(long)]
    label: Option<String>,
}

/// LAN recovery: with the feeder's recovery window open (Lock and Dispense
/// held on an unlocked panel), run the setup exchange over the device API
/// and become an owner once the code is keyed in.
pub(crate) async fn recover_command(args: RecoverArgs) -> Result<()> {
    let mut key = OwnerKey::open(&args.keys, args.label.as_deref(), OwnerKey::system())?;
    let address = wellfed_connect::parse_address(&args.address).context("--address")?;
    let (identity, outcome) =
        wellfed_install::pairing::recover(&mut key, address, &mut Asker::terminal_notices())
            .await?;
    println!("Recovered feeder {} ({outcome}).", fingerprint(&identity));
    Ok(())
}
