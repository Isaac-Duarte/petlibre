//! `wellfed`, the owner's command line ([tools]).
//!
//! It speaks two paths, and a command takes exactly one of them, never a
//! fallback between them: a write whose outcome is unknown on one path is
//! never resent on the other ([two paths, one feed]).
//!
//! - **The device API.** `wellfed setup` pairs this computer with a
//!   feeder in BLE setup mode (`pairing`): it shows a one-time code for the
//!   owner to key in on the feeder's buttons, and once the feeder has proven
//!   the code it gives the feeder Wi-Fi, enrolls this computer's key as an
//!   owner and records the feeder's identity. The protocol is
//!   `wellfed-client`'s, the same the Android app runs. The other device
//!   commands open a session on the device API, Noise over TCP to the feeder
//!   found over mDNS, as a key it enrolled, and print the feeder's answers
//!   as JSON: `configure`, `history`, `feeding` and `access`.
//! - **The owner's server.** `wellfed server …` (the `server` module)
//!   speaks the server's HTTP API through `wellfed-server-client`, with
//!   the token `wellfed server login` keeps.
//!
//! `wellfed install` takes a feeder from stock 1.1.15 to the replacement
//! (the `install` module), running `wellfed-stock` beside it for stock's
//! side.
//!
//! [tools]: ../../../../src/tools/README.md
//! [two paths, one feed]: ../../../../docs/server/README.md#two-paths-one-feed

#![forbid(unsafe_code)]

mod access;
mod backup;
mod ble;
mod configure;
mod device;
mod feeding;
mod history;
mod install;
mod keys;
mod pairing;
mod push;
mod server;
mod trace;

use std::io::BufRead;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use wellfed_model::{Answer, Op};

use crate::device::Connection;
use crate::keys::{OwnerKey, fingerprint};

#[derive(Parser)]
#[command(version, about = "The owner's command line for a Wellfed feeder")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Pair with a feeder in BLE setup mode and give it Wi-Fi.
    Setup(pairing::SetupArgs),
    /// The feeder's status.
    Status(Target),
    /// Read the settings, or change them (against the generation just read).
    Settings(configure::SettingsArgs),
    /// Lock or unlock the panel.
    Lock(configure::LockArgs),
    /// Store or clear the network the feeder joins next.
    Wifi(configure::WifiArgs),
    /// Set the feeder's clock from this computer's, with its time zone's
    /// offsets; or, with --zone or --ntp, change the time settings; or,
    /// with --show, read them and the clock's state.
    Time(configure::TimeArgs),
    /// The feeder's history, and with --follow its live events and samples.
    Events(history::EventsArgs),
    /// The feeder's debug log (the record lines its UART prints), and with
    /// --follow each new line. Owners only.
    Logs(history::LogsArgs),
    /// Mint a single-use invite for another client.
    Invite(access::InviteArgs),
    /// Redeem an invite with this computer's key.
    Join(access::JoinArgs),
    /// The enrolled clients.
    Clients(Target),
    /// Revoke a client; its open sessions close.
    Revoke(access::RevokeArgs),
    /// Dispense portions, and wait for the feed's result.
    Feed(feeding::FeedArgs),
    /// Stop a running feed.
    CancelFeed(feeding::CancelFeedArgs),
    /// Restart the feeder.
    Reboot(Target),
    /// The feeder's statistics: counters, memory, flash, power, food, feeds
    /// and portions.
    Stats(history::StatsArgs),
    /// What one feed's drive measured: its current, marks, beams, battery
    /// and motor commands over time, from the feeder's RAM or its storage.
    Trace(history::TraceArgs),
    /// Read the portion calibration and where the rotor rests, or store
    /// what a portion weighs, from a scale's reading of counted portions
    /// (docs/firmware/calibration.md).
    Calibrate(configure::CalibrateArgs),
    /// Become an owner again through the feeder's recovery window.
    Recover(pairing::RecoverArgs),
    /// Install the replacement on a feeder running stock 1.1.15: point stock
    /// at this computer, have it install the build into bank 0, pair as its
    /// owner, and (with a verified stock backup) promote it to bank 1.
    Install(Box<install::InstallArgs>),
    /// The owner's copy of stock: read it from the replacement on bank 0
    /// (bank 1 still holds stock until the promotion), show it, or export
    /// it unchanged.
    Backup(backup::BackupArgs),
    /// Install the stock backup over the replacement: signed as stock,
    /// pushed as any update, then the feeder runs stock from bank 1.
    RestoreStock(Box<backup::RestoreArgs>),
    /// The owner's server: log in, then the feeders, feeds and accounts it
    /// holds, over its HTTP API rather than the device API.
    Server(server::ServerArgs),
}

/// Exits 0 on success and 1 on an error; `wellfed server` also exits 3
/// when a write's outcome is unknown.
fn main() -> Result<ExitCode> {
    let cli = Cli::parse();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    let device = match cli.command {
        Command::Server(args) => return runtime.block_on(server::run(args)),
        Command::Setup(args) => runtime.block_on(pairing::setup(args)),
        Command::Status(target) => runtime.block_on(ask_and_print(&target, &Op::Status)),
        Command::Settings(args) => runtime.block_on(configure::settings_command(args)),
        Command::Lock(args) => runtime.block_on(configure::lock_command(args)),
        Command::Wifi(args) => runtime.block_on(configure::wifi_command(args)),
        Command::Time(args) => runtime.block_on(configure::time_command(args)),
        Command::Events(args) => runtime.block_on(history::events_command(args)),
        Command::Logs(args) => runtime.block_on(history::logs_command(args)),
        Command::Invite(args) => runtime.block_on(access::invite_command(args)),
        Command::Join(args) => runtime.block_on(access::join_command(args)),
        Command::Clients(target) => runtime.block_on(access::clients_command(target)),
        Command::Revoke(args) => runtime.block_on(access::revoke_command(args)),
        Command::Feed(args) => runtime.block_on(feeding::feed_command(args)),
        Command::CancelFeed(args) => runtime.block_on(feeding::cancel_feed_command(args)),
        Command::Reboot(target) => runtime.block_on(ask_and_print(&target, &Op::Reboot)),
        Command::Calibrate(args) => runtime.block_on(configure::calibrate_command(args)),
        Command::Stats(args) => runtime.block_on(history::stats_command(args)),
        Command::Trace(args) => runtime.block_on(history::trace_command(args)),
        Command::Recover(args) => runtime.block_on(pairing::recover_command(args)),
        Command::Install(args) => runtime.block_on(install::run(*args)),
        Command::Backup(args) => runtime.block_on(backup::command(args)),
        Command::RestoreStock(args) => runtime.block_on(backup::restore(*args)),
    };
    device.map(|()| ExitCode::SUCCESS)
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

async fn ask_and_print(target: &Target, op: &Op) -> Result<()> {
    let (_, _, mut connection) = connect(target).await?;
    let answer = connection.ask(op).await?;
    print_answer(&answer)
}

/// The answer as JSON; a refusal is also the command's failure.
fn print_answer(answer: &Answer) -> Result<()> {
    print_json(answer)?;
    if let Answer::Refused(refusal) = answer {
        bail!("refused: {refusal:?}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();

        let install = [
            "wellfed",
            "install",
            "--review",
            "private/review",
            "--manifest",
            "private/manifest.bin",
            "--ssid",
            "Home",
        ];
        assert!(matches!(
            Cli::try_parse_from(install).map(|cli| cli.command),
            Ok(Command::Install(_))
        ));
        // Resuming on bank 0 sends no frame.
        let resumed = [&install[..6], &["--from-bank0", "--ssid", "Home"]].concat();
        assert!(Cli::try_parse_from(resumed).is_err());

        // A server command parses whole, its flags after the verb.
        let feed = ["wellfed", "server", "feed", "--portions", "2", "--json"];
        assert!(matches!(
            Cli::try_parse_from(feed).map(|cli| cli.command),
            Ok(Command::Server(_))
        ));
    }
}
