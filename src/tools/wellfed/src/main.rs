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
//!   found over mDNS, as a key it enrolled: `configure`, `plans`,
//!   `history`, `feeding`, `access` and `update_key`. What a person reads prints as text
//!   (`text`), with `--json` for scripts; the rest prints the feeder's
//!   answers as JSON.
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
mod ask;
mod backup;
mod bench;
mod ble;
mod call;
mod configure;
mod device;
mod feeding;
mod history;
mod install;
mod keys;
mod pairing;
mod plans;
mod push;
mod server;
mod text;
mod trace;
mod update_key;

use std::io::BufRead;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use wellfed_formats::bench::rendezvous;
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
    /// The feeder's status: its clock, whether it can feed, the food
    /// level, the last failed feed, the supply, the desiccant reminder,
    /// the panel lock and its link.
    Status(history::StatusArgs),
    /// Read the settings, or change them (against the generation just read).
    Settings(configure::SettingsArgs),
    /// The feeding plans: list them, or add, edit, enable, disable or
    /// delete one (docs/firmware/schedules.md).
    Plans(plans::PlansArgs),
    /// The meal call a plan plays before its meal: show the stored clip,
    /// upload one from a WAV file, or delete it
    /// (docs/firmware/schedules.md#meal-call).
    Call(call::CallArgs),
    /// The desiccant reminder: days left until the pack is due, and with
    /// --reset record that it was replaced today
    /// (docs/firmware/desiccant.md). Its period is a setting.
    Desiccant(configure::DesiccantArgs),
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
    /// The feeds the feeder's history keeps: when, who asked, portions asked
    /// for and delivered, and how each ended (an uncertain feed as a range).
    Feeds(history::FeedsArgs),
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
    /// An owner's update-signing key: create one, enroll it on the feeder
    /// (confirmed by a Dispense press there), or list the enrolled ones.
    UpdateKey(update_key::UpdateKeyArgs),
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

    // These open sessions of their own, beside the one the bench server
    // holds: setup and recovery over the pairing window, a join with
    // another key, and the pushes of an install or a restore.
    let own_sessions = match &cli.command {
        Command::Setup(_) => Some("setup"),
        Command::Install(_) => Some("install"),
        Command::Recover(_) => Some("recover"),
        Command::Join(_) => Some("join"),
        Command::RestoreStock(_) => Some("restore-stock"),
        _ => None,
    };
    if let Some(command) = own_sessions {
        runtime.block_on(refuse_beside_bench(command, &rendezvous::directories()))?;
    }

    let device = match cli.command {
        Command::Server(args) => return runtime.block_on(server::run(args)),
        Command::Setup(args) => runtime.block_on(pairing::setup(args)),
        Command::Status(args) => runtime.block_on(history::status_command(args)),
        Command::Settings(args) => runtime.block_on(configure::settings_command(args)),
        Command::Plans(args) => runtime.block_on(plans::command(args)),
        Command::Call(args) => runtime.block_on(call::command(args)),
        Command::Desiccant(args) => runtime.block_on(configure::desiccant_command(args)),
        Command::Lock(args) => runtime.block_on(configure::lock_command(args)),
        Command::Wifi(args) => runtime.block_on(configure::wifi_command(args)),
        Command::Time(args) => runtime.block_on(configure::time_command(args)),
        Command::Events(args) => runtime.block_on(history::events_command(args)),
        Command::Feeds(args) => runtime.block_on(history::feeds_command(args)),
        Command::Logs(args) => runtime.block_on(history::logs_command(args)),
        Command::Invite(args) => runtime.block_on(access::invite_command(args)),
        Command::Join(args) => runtime.block_on(access::join_command(args)),
        Command::Clients(target) => runtime.block_on(access::clients_command(target)),
        Command::Revoke(args) => runtime.block_on(access::revoke_command(args)),
        Command::UpdateKey(args) => runtime.block_on(update_key::command(args)),
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

/// Refuse `command`, which opens device sessions of its own, while a bench
/// server holds the feeder's (its pointer in `rendezvous`), unless this
/// process is the server's job and the server paused its session for it
/// (`bench::Bench::admit`).
async fn refuse_beside_bench(command: &'static str, rendezvous: &[PathBuf]) -> Result<()> {
    match bench::Bench::find(rendezvous).await? {
        Some(bench) => bench.admit(command).await,
        None => Ok(()),
    }
}

/// A session with the feeder: the bench server's whenever one answers for
/// this user, and otherwise this computer's own.
async fn connect(target: &Target) -> Result<(OwnerKey, [u8; 32], Connection)> {
    connect_via(target, &rendezvous::directories()).await
}

/// [`connect`], finding the bench server by its pointer in `rendezvous`.
async fn connect_via(
    target: &Target,
    rendezvous: &[PathBuf],
) -> Result<(OwnerKey, [u8; 32], Connection)> {
    if let Some(bench) = bench::Bench::find(rendezvous).await? {
        let lent = bench::lend(
            bench,
            &target.keys,
            target.feeder.as_deref(),
            target.address.as_deref(),
        )
        .await?;
        let key = OwnerKey::open(&target.keys, None, OwnerKey::system())?;
        let identity = key.feeder(target.feeder.as_deref())?;
        if wellfed_formats::hex::encode(&identity) != lent.identity() {
            bail!(
                "the bench server's session is with feeder {}, not {}",
                &lent.identity()[..16],
                fingerprint(&identity)
            );
        }
        eprintln!(
            "Session with feeder {} through the bench server ({:?}, build {}).",
            fingerprint(&identity),
            lent.role(),
            lent.build()
        );
        return Ok((key, identity, Connection::Bench(Box::new(lent))));
    }

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
            "--stock-config",
            "private/installer.json",
            "--ssid",
            "Home",
        ];
        assert!(matches!(
            Cli::try_parse_from(install).map(|cli| cli.command),
            Ok(Command::Install(_))
        ));
        // Resuming on bank 0 sends no frame.
        let resumed = [&install[..8], &["--from-bank0", "--ssid", "Home"]].concat();
        assert!(Cli::try_parse_from(resumed).is_err());
        // The stock config is always named: the bench's captured one is no
        // default.
        assert!(Cli::try_parse_from(&install[..6]).is_err());
        // DNS first, or the frame, or a resumed run: one at a time.
        let dns = [&install[..8], &["--broker-name", "broker.example.net"]].concat();
        assert!(matches!(
            Cli::try_parse_from(&dns).map(|cli| cli.command),
            Ok(Command::Install(_))
        ));
        for other in [&["--ssid", "Home"][..], &["--from-bank0"][..]] {
            assert!(Cli::try_parse_from([&dns[..], other].concat()).is_err());
        }
        // Prompts go to a terminal or, for the bench server, as lines.
        for prompts in ["terminal", "lines"] {
            let asked = [&dns[..], &["--prompts", prompts]].concat();
            assert!(Cli::try_parse_from(asked).is_ok(), "{prompts}");
        }
        assert!(Cli::try_parse_from([&dns[..], &["--prompts", "json"]].concat()).is_err());
        // The frame's broker means nothing without the frame.
        let broker = [&install[..8], &["--broker", "10.42.103.1:1883"]].concat();
        assert!(Cli::try_parse_from(broker).is_err());

        // A plan's flags follow its verb, the slot first.
        let edit = [
            "wellfed", "plans", "edit", "3", "--at", "07:30", "--days", "mon,thu",
        ];
        assert!(matches!(
            Cli::try_parse_from(edit).map(|cli| cli.command),
            Ok(Command::Plans(_))
        ));
        let out_of_range = ["wellfed", "plans", "delete", "10"];
        assert!(Cli::try_parse_from(out_of_range).is_err());
        // A plan's meal call plays 0 to 5 times.
        let calling = ["wellfed", "plans", "edit", "3", "--call", "5"];
        assert!(Cli::try_parse_from(calling).is_ok());
        let too_loud = ["wellfed", "plans", "edit", "3", "--call", "6"];
        assert!(Cli::try_parse_from(too_loud).is_err());
        let upload = ["wellfed", "call", "upload", "dinner.wav"];
        assert!(matches!(
            Cli::try_parse_from(upload).map(|cli| cli.command),
            Ok(Command::Call(_))
        ));
        let too_many = [
            "wellfed",
            "plans",
            "add",
            "--at",
            "07:30",
            "--portions",
            "49",
        ];
        assert!(Cli::try_parse_from(too_many).is_err());

        // The period's limits are checked before anything is sent.
        let period = ["wellfed", "settings", "--desiccant-days", "91"];
        assert!(Cli::try_parse_from(period).is_err());
        let reset = ["wellfed", "desiccant", "--reset"];
        assert!(matches!(
            Cli::try_parse_from(reset).map(|cli| cli.command),
            Ok(Command::Desiccant(_))
        ));
        // The meal reminder takes `off` or 1-60 minutes, here and through
        // the server.
        for lead in ["off", "1", "60"] {
            let set = ["wellfed", "settings", "--meal-reminder", lead];
            assert!(matches!(
                Cli::try_parse_from(set).map(|cli| cli.command),
                Ok(Command::Settings(_))
            ));
        }
        for lead in ["0", "61", "soon", ""] {
            let set = ["wellfed", "settings", "--meal-reminder", lead];
            assert!(Cli::try_parse_from(set).is_err(), "{lead:?}");
        }
        let through = ["wellfed", "server", "settings", "--meal-reminder", "5"];
        assert!(Cli::try_parse_from(through).is_ok());

        // What a person reads is text, and --json gives scripts the wire's
        // spelling.
        for read in [
            &["wellfed", "status", "--json"][..],
            &["wellfed", "settings", "--json"],
            &["wellfed", "plans", "list", "--json"],
            &["wellfed", "events", "--follow", "--samples", "--json"],
            &["wellfed", "desiccant", "--json"],
            &["wellfed", "feeds", "--json"],
        ] {
            assert!(Cli::try_parse_from(read).is_ok(), "{read:?}");
        }

        // An update key's label fits the feeder's 16 bytes.
        let enroll = ["wellfed", "update-key", "enroll", "--key", "k.json"];
        assert!(Cli::try_parse_from(enroll).is_ok());
        let long = [&enroll[..], &["--label", "seventeen-letters"]].concat();
        assert!(Cli::try_parse_from(long).is_err());
        let new = ["wellfed", "update-key", "new", "--output", "k.json"];
        assert!(Cli::try_parse_from(new).is_ok());

        // A server command parses whole, its flags after the verb.
        let feed = ["wellfed", "server", "feed", "--portions", "2", "--json"];
        assert!(matches!(
            Cli::try_parse_from(feed).map(|cli| cli.command),
            Ok(Command::Server(_))
        ));
    }
}
