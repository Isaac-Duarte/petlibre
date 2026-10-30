//! Becoming a feeder's owner: `setup` over BLE, and `recover` through the
//! feeder's recovery window on the LAN. Both run `wellfed-client`'s setup
//! session, the one the Android app runs, and show the one-time code the
//! owner keys in on the feeder's buttons.

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Args;
use wellfed_client::setup::{Ask, Credentials, Session, Step, panel_position};
use wellfed_formats::bench::prompt::{CodePresses, Prompt};
use wellfed_formats::private;
use wellfed_model::setup::{Enrolled, Refusal, Reply, WifiSaved};
use wellfed_session::setup_code::Code;

use crate::ask::Asker;
use crate::ble::Link;
use crate::keys::{OwnerKey, fingerprint};
use crate::passphrase;

/// The feeder answers each handshake message at once.
const HANDSHAKE_WAIT: Duration = Duration::from_secs(10);
/// Beyond the feeder's own confirmation window, for the reply to arrive.
const CONFIRM_SLACK: Duration = Duration::from_secs(10);

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
    let (identity, enrolled) = over_ble(&mut key, wifi, link, &mut asker).await?;
    println!(
        "Paired with feeder {} ({}); it reboots now without BLE.",
        fingerprint(&identity),
        enrolled
    );
    Ok(())
}

/// Enroll `key` as an owner over `link` to a feeder in setup mode, giving
/// it `wifi` when set, the owner keying in the code `asker` announces; the
/// feeder's identity, remembered in the key file, and what the feeder said.
/// The link is closed either way.
pub(crate) async fn over_ble<W: Write>(
    key: &mut OwnerKey,
    wifi: Option<Credentials>,
    mut link: Link,
    asker: &mut Asker<W>,
) -> Result<([u8; 32], String)> {
    let ask = Ask {
        label: key.label().to_string(),
        wifi,
    };
    let random = OwnerKey::system();
    let ephemeral: [u8; 32] = private::random(random).context("read the system random source")?;
    let seed: [u8; 32] = private::random(random).context("read the system random source")?;
    let bits: [u8; 2] = private::random(random).context("read the system random source")?;
    let code = Code::from_bits(u16::from_le_bytes(bits));
    let mut session = Session::new(key.keypair(), ephemeral, ask, code, seed)?;

    let result = exchange(&mut session, &mut link, asker).await;
    link.close().await;
    let enrolled = result?;

    let Some(identity) = session.feeder_key() else {
        bail!("the feeder never proved the code");
    };
    key.remember(&identity)?;
    Ok((identity, enrolled))
}

/// Run the session to its end; the outcome as text.
async fn exchange<W: Write>(
    session: &mut Session,
    link: &mut Link,
    asker: &mut Asker<W>,
) -> Result<String> {
    link.send(&session.start()?).await?;
    let mut wait = HANDSHAKE_WAIT;
    loop {
        let message = link.receive(wait).await?;
        match session.receive(&message)? {
            Step::Send(bytes) => link.send(&bytes).await?,
            Step::Reply(reply) => match outcome(reply) {
                SetupOutcome::EnterCode(seconds) => {
                    show_code(asker, session.code(), seconds).await?;
                    wait = Duration::from_secs(seconds.into()) + CONFIRM_SLACK;
                }
                SetupOutcome::Done(text) => return Ok(text),
                SetupOutcome::Refused(text) => bail!("the feeder refused: {text}"),
            },
        }
    }
}

/// Announce the code to key in. The feeder already waits for it when it
/// asks, so the announcement goes at once and the run waits only for the
/// feeder's outcome, within its window.
async fn show_code<W: Write>(asker: &mut Asker<W>, code: Code, seconds: u16) -> Result<()> {
    let prompt = Prompt::KeySetupCode {
        presses: CodePresses::new(*code.presses()),
    };
    asker
        .announce(prompt, &instructions(code, seconds))
        .await
        .context("show the setup code")
}

/// The code as the owner keys it in: button positions, then their names.
fn instructions(code: Code, seconds: u16) -> String {
    let presses = code.presses();
    let positions: Vec<String> = presses
        .iter()
        .map(|&b| panel_position(b).to_string())
        .collect();
    let names: Vec<&str> = presses.iter().map(|b| b.as_str()).collect();
    format!(
        "Within {seconds} s, press these buttons on the feeder one at a time \
         (numbered from the left: 1 Lock, 2 Dispense, 3 Mic, 4 Reset):\n\n    \
         {}\n    ({})\n\nThe code proves this computer is talking to that feeder; \
         it is used once.",
        positions.join(" "),
        names.join(", ")
    )
}

enum SetupOutcome {
    EnterCode(u16),
    Done(String),
    Refused(&'static str),
}

fn outcome(reply: Reply) -> SetupOutcome {
    match reply {
        Reply::EnterCode { seconds } => SetupOutcome::EnterCode(seconds),
        Reply::Done { enrolled, wifi } => SetupOutcome::Done(format!(
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
        Reply::Refused(refusal) => SetupOutcome::Refused(match refusal {
            Refusal::Busy => "another setup is waiting for its code",
            Refusal::Unconfirmed => "the code was not keyed in in time",
            Refusal::WrongCode => {
                "the code keyed in does not match: mistyped, or another device is relaying"
            }
            Refusal::Full => "every client slot is taken; revoke one first",
            Refusal::NotWritable => "its store cannot be written now",
            Refusal::Invalid => "it rejected the label or the Wi-Fi credentials",
        }),
    }
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
    let (identity, text) = recover(&mut key, address, &mut Asker::terminal_notices()).await?;
    println!("Recovered feeder {} ({text}).", fingerprint(&identity));
    Ok(())
}

/// Enroll `key` as an owner through the feeder's recovery window at
/// `address`, the owner keying in the code `asker` announces; the feeder's
/// identity, remembered in the key file, and what the feeder said.
pub(crate) async fn recover<W: Write>(
    key: &mut OwnerKey,
    address: std::net::SocketAddr,
    asker: &mut Asker<W>,
) -> Result<([u8; 32], String)> {
    let random = OwnerKey::system();
    let ephemeral: [u8; 32] = private::random(random).context("read the system random source")?;
    let seed: [u8; 32] = private::random(random).context("read the system random source")?;
    let bits: [u8; 2] = private::random(random).context("read the system random source")?;
    let ask = Ask {
        label: key.label().to_string(),
        wifi: None,
    };
    let session = Session::new(
        key.keypair(),
        ephemeral,
        ask,
        Code::from_bits(u16::from_le_bytes(bits)),
        seed,
    )?;

    let mut recovery = wellfed_connect::Recovery::open(address, session)
        .await
        .with_context(|| format!("connect to {address}"))?;
    let mut wait = HANDSHAKE_WAIT;
    let text = loop {
        let reply = match recovery.reply(wait).await {
            Err(wellfed_connect::Error::Closed) => {
                bail!("the feeder closed the connection: is its recovery window open?")
            }
            Err(wellfed_connect::Error::Timeout) => bail!("the feeder did not answer in time"),
            other => other?,
        };
        match outcome(reply) {
            SetupOutcome::EnterCode(seconds) => {
                show_code(asker, recovery.session().code(), seconds).await?;
                wait = Duration::from_secs(seconds.into()) + CONFIRM_SLACK;
            }
            SetupOutcome::Done(text) => break text,
            SetupOutcome::Refused(text) => bail!("the feeder refused: {text}"),
        }
    };

    let Some(identity) = recovery.session().feeder_key() else {
        bail!("the feeder never proved the code");
    };
    key.remember(&identity)?;
    Ok((identity, text))
}
