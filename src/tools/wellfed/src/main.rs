//! `wellfed`, the owner's command line ([tools roadmap]).
//!
//! `wellfed setup` pairs this computer with a feeder in BLE setup mode
//!: it shows a one-time code for the owner to key in on the feeder's
//! buttons, and once the feeder has proven the code it gives the feeder
//! Wi-Fi, enrolls this computer's key as an owner and records the feeder's
//! identity.
//! The protocol is `wellfed-client`'s, the same the Android app runs.
//!
//! The other commands open a session on the device API: Noise over
//! TCP to the feeder found over mDNS, as a key it enrolled. They print the
//! feeder's answers as JSON.
//!
//! [tools roadmap]: ../../../../docs/development/tools-roadmap.md

#![forbid(unsafe_code)]

mod ble;
mod device;
mod keys;

use std::io::BufRead;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use wellfed_client::api::Invite;
use wellfed_client::setup::{Ask, Credentials, Session, Step, panel_position};
use wellfed_core::setup_code::Code;
use wellfed_formats::{hex, private};
use wellfed_proto::api::{
    EventBody, FeederMessage, Joined, Op, Outcome, Role, SettingsChanges, Wifi,
};
use wellfed_proto::setup::{Enrolled, Refusal, Reply, WifiSaved};

use crate::ble::Link;
use crate::device::Connection;
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
    /// The feeder's status.
    Status(Target),
    /// Read the settings, or change them (against the generation just read).
    Settings(SettingsArgs),
    /// Lock or unlock the panel.
    Lock(LockArgs),
    /// Store or clear the network the feeder joins next.
    Wifi(WifiArgs),
    /// Set the feeder's clock from this computer's, with its time zone.
    Time(Target),
    /// The feeder's history, and with --follow its live events and samples.
    Events(EventsArgs),
    /// Mint a single-use invite for another client.
    Invite(InviteArgs),
    /// Redeem an invite with this computer's key.
    Join(JoinArgs),
    /// The enrolled clients.
    Clients(Target),
    /// Revoke a client; its open sessions close.
    Revoke(RevokeArgs),
    /// Dispense portions, and wait for the feed's result.
    Feed(FeedArgs),
    /// Stop a running feed.
    CancelFeed(CancelFeedArgs),
    /// Restart the feeder.
    Reboot(Target),
    /// Become an owner again through the feeder's recovery window.
    Recover(RecoverArgs),
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
        Command::Status(target) => runtime.block_on(ask_and_print(&target, Op::Status)),
        Command::Settings(args) => runtime.block_on(settings_command(args)),
        Command::Lock(args) => runtime.block_on(ask_and_print(
            &args.target,
            Op::SetLock {
                locked: args.state.on(),
            },
        )),
        Command::Wifi(args) => runtime.block_on(wifi_command(args)),
        Command::Time(target) => runtime.block_on(time_command(target)),
        Command::Events(args) => runtime.block_on(events_command(args)),
        Command::Invite(args) => runtime.block_on(invite_command(args)),
        Command::Join(args) => runtime.block_on(join_command(args)),
        Command::Clients(target) => runtime.block_on(clients_command(target)),
        Command::Revoke(args) => runtime.block_on(revoke_command(args)),
        Command::Feed(args) => runtime.block_on(feed_command(args)),
        Command::CancelFeed(args) => runtime.block_on(ask_and_print(
            &args.target,
            Op::CancelFeed { feed: args.feed },
        )),
        Command::Reboot(target) => runtime.block_on(ask_and_print(&target, Op::Reboot)),
        Command::Recover(args) => runtime.block_on(recover_command(args)),
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
    let random = OwnerKey::system();
    let ephemeral: [u8; 32] = private::random(random).context("read the system random source")?;
    let seed: [u8; 32] = private::random(random).context("read the system random source")?;
    let bits: [u8; 2] = private::random(random).context("read the system random source")?;
    let code = Code::from_bits(u16::from_le_bytes(bits));
    let mut session = Session::new(key.keypair(), ephemeral, ask, code, seed)?;

    let mut link = Link::open(args.address).await?;
    let result = exchange(&mut session, &mut link).await;
    link.close().await;
    let enrolled = result?;

    let Some(identity) = session.feeder_key() else {
        bail!("the feeder never proved the code");
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
                SetupOutcome::EnterCode(seconds) => {
                    eprintln!("{}", instructions(session.code(), seconds));
                    wait = Duration::from_secs(seconds.into()) + CONFIRM_SLACK;
                }
                SetupOutcome::Done(text) => return Ok(text),
                SetupOutcome::Refused(text) => bail!("the feeder refused: {text}"),
            },
        }
    }
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
        // The session answers the proof's steps itself.
        Reply::Round { .. } => SetupOutcome::Refused("it sent a proof step out of turn"),
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

/// Which feeder, and with which key.
#[derive(Args)]
struct Target {
    /// This computer's client key.
    #[arg(long, default_value = "private/wellfed-owner.json")]
    keys: PathBuf,
    /// The feeder's identity, as a hex prefix; needed when the key file
    /// knows more than one.
    #[arg(long)]
    feeder: Option<String>,
    /// The feeder's address (ip or ip:port); otherwise found over mDNS.
    #[arg(long)]
    address: Option<String>,
}

#[derive(Clone, Copy, ValueEnum)]
enum Toggle {
    On,
    Off,
}

impl Toggle {
    const fn on(self) -> bool {
        matches!(self, Self::On)
    }
}

#[derive(Args)]
struct SettingsArgs {
    #[command(flatten)]
    target: Target,
    #[arg(long)]
    lights: Option<Toggle>,
    #[arg(long)]
    sound: Option<Toggle>,
    /// 0-100.
    #[arg(long)]
    volume: Option<u8>,
}

#[derive(Args)]
struct LockArgs {
    #[command(flatten)]
    target: Target,
    state: Toggle,
}

#[derive(Args)]
struct WifiArgs {
    #[command(flatten)]
    target: Target,
    /// The network to store; the passphrase is read from standard input.
    #[arg(long, conflicts_with = "clear")]
    ssid: Option<String>,
    /// Forget the stored network; the build's applies again.
    #[arg(long)]
    clear: bool,
}

#[derive(Args)]
struct EventsArgs {
    #[command(flatten)]
    target: Target,
    /// Start after this sequence number, in the feeder's current epoch
    /// (default: the oldest kept).
    #[arg(long)]
    after: Option<u64>,
    /// Keep the session open and print new events (and samples) as they
    /// happen.
    #[arg(long)]
    follow: bool,
    #[arg(long)]
    samples: bool,
}

#[derive(Args)]
struct FeedArgs {
    #[command(flatten)]
    target: Target,
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..))]
    portions: u8,
}

#[derive(Args)]
struct CancelFeedArgs {
    #[command(flatten)]
    target: Target,
    /// The feed's number, as `wellfed feed` printed it.
    feed: u32,
}

#[derive(Clone, Copy, ValueEnum)]
enum InviteRole {
    Owner,
    Member,
    Server,
}

#[derive(Args)]
struct InviteArgs {
    #[command(flatten)]
    target: Target,
    #[arg(long, value_enum, default_value = "member")]
    role: InviteRole,
}

#[derive(Args)]
struct JoinArgs {
    /// The invite as `wellfed invite` printed it.
    invite: String,
    /// The key that joins, created on first use.
    #[arg(long, default_value = "private/wellfed-owner.json")]
    keys: PathBuf,
    #[arg(long)]
    label: Option<String>,
    #[arg(long)]
    address: Option<String>,
}

#[derive(Args)]
struct RevokeArgs {
    #[command(flatten)]
    target: Target,
    /// The client's key, as a hex prefix of at least 8 digits.
    client: String,
}

async fn connect(target: &Target) -> Result<(OwnerKey, [u8; 32], Connection)> {
    let key = OwnerKey::open(&target.keys, None, OwnerKey::system())?;
    let identity = key.feeder(target.feeder.as_deref())?;
    let address = device::locate(&identity, target.address.as_deref())?;
    let connection = Connection::open(&key, identity, address).await?;
    eprintln!(
        "Session with feeder {} at {address} ({:?}, build {}).",
        fingerprint(&identity),
        connection.role(),
        connection.build()
    );
    Ok((key, identity, connection))
}

fn print_json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

async fn ask_and_print(target: &Target, op: Op<'_>) -> Result<()> {
    let (_, _, mut connection) = connect(target).await?;
    let bytes = connection.ask(op).await?;
    let outcome = bytes.outcome();
    print_json(&outcome)?;
    if let Outcome::Refused(refusal) = outcome {
        bail!("refused: {refusal:?}");
    }
    Ok(())
}

async fn settings_command(args: SettingsArgs) -> Result<()> {
    let changes = SettingsChanges {
        lights: args.lights.map(Toggle::on),
        sound: args.sound.map(Toggle::on),
        volume: args.volume,
    };
    if changes == SettingsChanges::default() {
        return ask_and_print(&args.target, Op::GetSettings).await;
    }
    let (_, _, mut connection) = connect(&args.target).await?;
    let current = connection.ask(Op::GetSettings).await?;
    let Outcome::Settings(settings) = current.outcome() else {
        bail!("the feeder did not return its settings");
    };
    let bytes = connection
        .ask(Op::PutSettings {
            base: settings.generation,
            changes,
        })
        .await?;
    let outcome = bytes.outcome();
    print_json(&outcome)?;
    if let Outcome::Refused(refusal) = outcome {
        bail!("refused: {refusal:?}");
    }
    Ok(())
}

async fn wifi_command(args: WifiArgs) -> Result<()> {
    let passphrase = match &args.ssid {
        Some(_) => Some(passphrase()?),
        None if args.clear => None,
        None => bail!("pass --ssid or --clear"),
    };
    let (_, _, mut connection) = connect(&args.target).await?;
    let status = connection.ask(Op::Status).await?;
    let Outcome::Status(status) = status.outcome() else {
        bail!("the feeder did not return its status");
    };
    let wifi = match (&args.ssid, &passphrase) {
        (Some(ssid), Some(psk)) => Some(Wifi {
            ssid: ssid.as_bytes(),
            psk,
        }),
        _ => None,
    };
    let bytes = connection
        .ask(Op::PutWifi {
            base: status.wifi_generation,
            wifi,
        })
        .await?;
    let outcome = bytes.outcome();
    print_json(&outcome)?;
    if let Outcome::Refused(refusal) = outcome {
        bail!("refused: {refusal:?}");
    }
    Ok(())
}

async fn time_command(target: Target) -> Result<()> {
    let zone = jiff::tz::TimeZone::system();
    let sync = wellfed_client::time::sync(jiff::Timestamp::now(), &zone);
    ask_and_print(&target, Op::TimeSync(sync)).await
}

async fn events_command(args: EventsArgs) -> Result<()> {
    let (_, _, mut connection) = connect(&args.target).await?;
    connection
        .subscribe(args.after, args.samples && args.follow)
        .await?;
    // Without --follow, stop once the backlog has been quiet for a moment.
    let wait = if args.follow {
        Duration::from_secs(3600)
    } else {
        Duration::from_secs(2)
    };
    loop {
        // The session keeps itself alive, so a quiet hour is no failure.
        let message = match connection.next(wait).await {
            Ok(Some(message)) => message,
            Ok(None) if args.follow => continue,
            Ok(None) => return Ok(()),
            Err(error) => return Err(error),
        };
        let message = message.get();
        match message {
            FeederMessage::Event(_) | FeederMessage::Gap { .. } | FeederMessage::Sample(_) => {
                println!("{}", serde_json::to_string(&message)?);
            }
            FeederMessage::Response { .. } => {}
        }
    }
}

/// Longer than any feed the owner's limits allow.
const FEED_WAIT: Duration = Duration::from_secs(120);

/// One feed under a fresh id, never retried: a feed whose answer is lost
/// may have run. Prints the answer, then the feed's `Fed` event.
async fn feed_command(args: FeedArgs) -> Result<()> {
    let (_, _, mut connection) = connect(&args.target).await?;
    let status = connection.ask(Op::Status).await?;
    let Outcome::Status(status) = status.outcome() else {
        bail!("the feeder did not answer with its status");
    };
    // Only events after this one can be the feed's.
    connection.subscribe(status.last_event, false).await?;

    let answer = connection
        .ask(Op::Feed {
            portions: args.portions,
        })
        .await?;
    let outcome = answer.outcome();
    print_json(&outcome)?;
    let Outcome::Feeding { feed } = outcome else {
        bail!("the feeder did not start the feed: {outcome:?}");
    };

    loop {
        let Some(message) = connection.next(FEED_WAIT).await? else {
            bail!(
                "no result for feed {feed} after {} s; `wellfed events` shows it once recorded",
                FEED_WAIT.as_secs()
            );
        };
        let message = message.get();
        if let FeederMessage::Event(event) = message
            && matches!(event.body, EventBody::Fed { feed: ended, .. } if ended == feed)
        {
            return print_json(&event);
        }
    }
}

async fn invite_command(args: InviteArgs) -> Result<()> {
    let role = match args.role {
        InviteRole::Owner => Role::Owner,
        InviteRole::Member => Role::Member,
        InviteRole::Server => Role::Server,
    };
    let (_, identity, mut connection) = connect(&args.target).await?;
    let bytes = connection.ask(Op::CreateInvite { role }).await?;
    match bytes.outcome() {
        Outcome::Invite(invite) => {
            let text = Invite {
                feeder: identity,
                id: invite.id,
                secret: invite.secret,
            }
            .text();
            println!("{text}");
            eprintln!(
                "Single use, for the next {} s; anyone holding it joins as {role:?}.",
                invite.seconds
            );
            Ok(())
        }
        Outcome::Refused(refusal) => bail!("refused: {refusal:?}"),
        other => bail!("unexpected answer {other:?}"),
    }
}

async fn join_command(args: JoinArgs) -> Result<()> {
    let invite = Invite::parse(&args.invite)?;
    let mut key = OwnerKey::open(&args.keys, args.label.as_deref(), OwnerKey::system())?;
    let address = device::locate(&invite.feeder, args.address.as_deref())?;
    let joined = wellfed_connect::join(
        key.keypair(),
        invite,
        key.label(),
        address,
        device::random()?,
    )
    .await
    .map_err(|error| match error {
        wellfed_connect::Error::Closed => {
            anyhow::anyhow!("the feeder closed the connection (an unknown or expired invite)")
        }
        other => other.into(),
    })?;
    match joined {
        Joined::Enrolled { role } => {
            key.remember(&invite.feeder)?;
            println!("Joined feeder {} as {role:?}.", fingerprint(&invite.feeder));
            Ok(())
        }
        Joined::Refused(refusal) => bail!("refused: {refusal:?}"),
    }
}

async fn clients_command(target: Target) -> Result<()> {
    let (_, _, mut connection) = connect(&target).await?;
    let bytes = connection.ask(Op::ListClients).await?;
    let Outcome::Clients(clients) = bytes.outcome() else {
        bail!("the feeder did not list its clients");
    };
    for client in clients.clients.iter().flatten() {
        println!(
            "{}  {:<6?}  {}{}",
            hex::encode(&client.key),
            client.role,
            client.label,
            if client.you { "  (this key)" } else { "" }
        );
    }
    Ok(())
}

async fn revoke_command(args: RevokeArgs) -> Result<()> {
    if args.client.len() < 8 {
        bail!("give at least 8 hex digits of the client's key");
    }
    let (_, _, mut connection) = connect(&args.target).await?;
    let bytes = connection.ask(Op::ListClients).await?;
    let Outcome::Clients(clients) = bytes.outcome() else {
        bail!("the feeder did not list its clients");
    };
    let matches: Vec<[u8; 32]> = clients
        .clients
        .iter()
        .flatten()
        .filter(|c| hex::encode(&c.key).starts_with(&args.client))
        .map(|c| c.key)
        .collect();
    let [key] = matches.as_slice() else {
        bail!("{} clients match {}", matches.len(), args.client);
    };
    let bytes = connection.ask(Op::RevokeClient { key: *key }).await?;
    let outcome = bytes.outcome();
    print_json(&outcome)?;
    if let Outcome::Refused(refusal) = outcome {
        bail!("refused: {refusal:?}");
    }
    Ok(())
}

#[derive(Args)]
struct RecoverArgs {
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
async fn recover_command(args: RecoverArgs) -> Result<()> {
    let mut key = OwnerKey::open(&args.keys, args.label.as_deref(), OwnerKey::system())?;
    let address = wellfed_connect::parse_address(&args.address).context("--address")?;
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
                eprintln!("{}", instructions(recovery.session().code(), seconds));
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
    println!("Recovered feeder {} ({text}).", fingerprint(&identity));
    Ok(())
}
