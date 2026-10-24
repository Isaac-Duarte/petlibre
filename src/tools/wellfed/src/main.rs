//! `wellfed`, the owner's command line ([tools roadmap]).
//!
//! `wellfed setup` pairs this computer with a feeder in BLE setup mode
//! (FW45): it gives the feeder Wi-Fi, enrolls this computer's key as an
//! owner once someone presses Dispense, and records the feeder's identity.
//! The protocol is `wellfed-client`'s, the same the Android app runs. The
//! device API's commands (feed, plans, settings, history) join as FW47
//! lands.
//!
//! [tools roadmap]: ../../../../docs/development/tools-roadmap.md

#![forbid(unsafe_code)]

mod ble;
mod keys;

use std::io::BufRead;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use wellfed_client::setup::{Ask, Credentials, Session, Step};
use wellfed_formats::private;
use wellfed_proto::setup::{Enrolled, Refusal, Reply, WifiSaved};

use crate::ble::Link;
use crate::keys::{OwnerKey, fingerprint};

/// The feeder answers each handshake message at once.
const HANDSHAKE_WAIT: Duration = Duration::from_secs(10);
/// Beyond the feeder's own confirmation window, for the reply to arrive.
const CONFIRM_SLACK: Duration = Duration::from_secs(10);

#[derive(Parser)]
#[command(version, about = "The owner's command line for a Wellfed feeder")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Pair with a feeder in BLE setup mode and give it Wi-Fi.
    Setup(SetupArgs),
}

#[derive(Args)]
struct SetupArgs {
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    match cli.command {
        Command::Setup(args) => runtime.block_on(setup(args)),
    }
}

/// The passphrase, from standard input without echoing it anywhere.
fn passphrase() -> Result<String> {
    eprintln!("Passphrase for the feeder's network (standard input):");
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .context("read the passphrase")?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

async fn setup(args: SetupArgs) -> Result<()> {
    let wifi = match args.ssid {
        Some(ssid) => Some(Credentials::new(ssid.into_bytes(), passphrase()?)?),
        None => None,
    };
    let mut key = OwnerKey::open(&args.keys, args.label.as_deref(), OwnerKey::system())?;
    let ask = Ask {
        label: key.label().to_string(),
        wifi,
    };
    let ephemeral: [u8; 32] =
        private::random(OwnerKey::system()).context("read the system random source")?;
    let mut session = Session::new(key.keypair(), ephemeral, ask)?;

    let mut link = Link::open(args.address).await?;
    let result = exchange(&mut session, &mut link).await;
    link.close().await;
    let enrolled = result?;

    let Some(identity) = session.feeder_key() else {
        bail!("the feeder never sent its key");
    };
    key.remember(&identity)?;
    println!(
        "Paired with feeder {} ({}); it reboots now without BLE.",
        fingerprint(&identity),
        enrolled
    );
    Ok(())
}

/// Run the session to its end; the outcome as text.
async fn exchange(session: &mut Session, link: &mut Link) -> Result<String> {
    link.send(&session.start()?).await?;
    let mut wait = HANDSHAKE_WAIT;
    loop {
        let message = link.receive(wait).await?;
        match session.receive(&message)? {
            Step::Send(bytes) => link.send(&bytes).await?,
            Step::Reply(reply) => match outcome(reply) {
                Outcome::Confirming(seconds) => {
                    eprintln!("Press Dispense on the feeder within {seconds} s to confirm.");
                    wait = Duration::from_secs(seconds.into()) + CONFIRM_SLACK;
                }
                Outcome::Done(text) => return Ok(text),
                Outcome::Refused(text) => bail!("the feeder refused: {text}"),
            },
        }
    }
}

enum Outcome {
    Confirming(u16),
    Done(String),
    Refused(&'static str),
}

fn outcome(reply: Reply) -> Outcome {
    match reply {
        Reply::Confirming { seconds } => Outcome::Confirming(seconds),
        Reply::Done { enrolled, wifi } => Outcome::Done(format!(
            "{}, {}",
            match enrolled {
                Enrolled::New => "enrolled as an owner",
                Enrolled::Already => "already enrolled",
            },
            match wifi {
                WifiSaved::Saved => "Wi-Fi saved for its next join",
                WifiSaved::Unchanged => "Wi-Fi unchanged",
            }
        )),
        Reply::Refused(refusal) => Outcome::Refused(match refusal {
            Refusal::Busy => "another setup is waiting for its press",
            Refusal::Unconfirmed => "nobody pressed Dispense in time",
            Refusal::Full => "every client slot is taken; revoke one first",
            Refusal::NotWritable => "its store cannot be written now",
            Refusal::Invalid => "it rejected the label or the Wi-Fi credentials",
        }),
    }
}
