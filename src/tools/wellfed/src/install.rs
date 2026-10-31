//! `wellfed install`: one run from a feeder on stock 1.1.15 to the
//! replacement confirmed on bank 1 ([consumer installation]).
//!
//! The run is `wellfed_install`'s, the same the Android app runs: stock's
//! server in this process, the request, pairing, the backup and the promotion.
//! This module is its shell for a computer: it reads
//! and checks the review, the keys, the manifest and the networks before
//! the feeder is touched, holds the bench lease beside the stock config for
//! the whole run, and gives the run this computer's platform:
//! - the person at the feeder is one [`Asker`], at a terminal or, with
//!   `--prompts lines`, as JSON lines for the bench server that runs it as
//!   a job. The setup code and the enrollment's Dispense press are only
//!   announced: the feeder times their windows and judges the presses. A
//!   job needs nothing read at a terminal, so its home network comes from a
//!   file (`--home-network`), and the owner may answer the feeder check
//!   ahead (`--expect-device`);
//! - Bluetooth is BlueZ ([`crate::ble`]), discovery is mDNS, and the client
//!   key is the key file;
//! - progress prints on standard error ([`crate::report`]), with the stock
//!   server's own log, so standard output carries prompts alone.
//!
//! SIGTERM (a job's cancel) and SIGINT end the run through the same stop as
//! its end. The report names the step it stopped in and any write whose
//! outcome had not settled; that write is uncertain, and never sent again.
//!
//! [consumer installation]: ../../../../docs/firmware/consumer-installation.md

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Args;
use tokio::signal::unix::{SignalKind, signal};
use wellfed_client::install::{InstallError, Provision, SetupFrame, StockBackup, manifest_version};
use wellfed_client::setup::Credentials;
use wellfed_formats::bench::lease::Lease;
use wellfed_formats::credentials::{KeyScope, WifiCredential};
use wellfed_formats::manifest::Provisioning;
use wellfed_formats::stock::ServerConfig;
use wellfed_install::StockPlans;
use wellfed_install::{
    Api, Mdns, Plan, Platform, Review, Signer, Start, StockOptions, Stop, build_info,
};
use wellfed_signing::{Frozen, Key, signed_so_far};

use crate::ask::{Asker, Prompts};
use crate::ble::Bluez;
use crate::keys::{OwnerKey, fingerprint};
use crate::report::{Printer, short};

#[derive(Args)]
pub(crate) struct InstallArgs {
    /// The replacement's frozen review (its `review.json`, `image.bin` and
    /// `build-manifest.json`): what stock installs into bank 0.
    #[arg(long)]
    review: PathBuf,
    /// A signed update manifest for the same build, which bank 0 installs
    /// into bank 1 at promotion; its version must be above the feeder's
    /// floor. Without it the run signs the build itself with --update-key,
    /// at a version above the floor and above any it signed before, and
    /// keeps the manifest under --backups.
    #[arg(long)]
    manifest: Option<PathBuf>,
    /// The owner's update-signing key file, created here if missing (as
    /// `wellfed update-key new` does). When the image does not build it in,
    /// the run enrolls it after pairing, confirmed by a Dispense press.
    /// Without it, a bench review signs with the bench key
    /// (private/ota-key.json), and a consumer review needs --manifest.
    /// Refused with --manifest, which is promoted as signed.
    #[arg(long)]
    update_key: Option<PathBuf>,
    /// The network a consumer image is given over Bluetooth setup
    /// (passphrase from standard input); with --ssid, that network.
    #[arg(long, conflicts_with = "ssid")]
    home_ssid: Option<String>,
    /// The network a consumer image is given, from a private JSON file of
    /// its `ssid` and `password` (as `private/hotspot.json` holds them),
    /// so nothing is read at a terminal and the run works with --prompts
    /// lines. The passphrase is never printed.
    #[arg(long, conflicts_with_all = ["home_ssid", "ssid"])]
    home_network: Option<PathBuf>,
    /// Stock's MQTT client ID as the owner already knows it (the feeder's
    /// label, the vendor app): when the broker enrolls exactly this
    /// device, the feeder check passes without asking, and when it enrolls
    /// another, the run stops before anything is sent to stock. The
    /// owner's answer to that check, given ahead from evidence they hold.
    #[arg(long)]
    expect_device: Option<String>,
    /// The feeder's Bluetooth address for setup; otherwise the first in
    /// setup mode.
    #[arg(long)]
    ble_address: Option<bluer::Address>,
    /// A copy of stock 1.1.15's OTA image that matches the pinned digest.
    /// Without one, the run reads bank 1 from the replacement on bank 0
    /// and keeps it under --backups; promotion overwrites the feeder's
    /// only copy of stock, so the run stops before it unless one copy or
    /// the other checks out, with the replacement on bank 0 and stock
    /// intact in bank 1.
    #[arg(long)]
    stock_backup: Option<PathBuf>,
    /// The owner's data directory for stock backups, one per feeder.
    #[arg(long, default_value = crate::backup::BACKUPS)]
    backups: PathBuf,
    /// The owner holds Reset, and the setup frame points stock at this
    /// network (passphrase from standard input) and the installer's broker.
    /// Without it, stock already connects to the installer's broker.
    #[arg(long)]
    ssid: Option<String>,
    /// The numeric account ID the frame carries; stock requires nonzero
    /// digits and keeps it beside the broker.
    #[arg(long, default_value = "1")]
    account: String,
    /// The broker the frame names; the stock config's MQTT address otherwise.
    #[arg(long, requires = "ssid")]
    broker: Option<SocketAddr>,
    /// The replacement is already on bank 0 from an earlier run: start at
    /// pairing, and ask stock nothing.
    #[arg(long, conflicts_with = "ssid")]
    from_bank0: bool,
    /// `wellfed-stock`'s config: an installer's (`init-installer`). One
    /// with a plan snapshot, as the bench's captured config has, is
    /// refused. The run serves stock from this process with it.
    #[arg(long)]
    stock_config: PathBuf,
    /// How long the broker trusts the first stock feeder that connects.
    #[arg(long, default_value_t = 15)]
    enroll_minutes: u64,
    /// This computer's client key, created on first use.
    #[arg(long, default_value = "private/wellfed-owner.json")]
    keys: PathBuf,
    /// The feeder's identity, as a hex prefix, when the key file knows more
    /// than one.
    #[arg(long)]
    feeder: Option<String>,
    /// The replacement's address; where stock connected from otherwise.
    #[arg(long)]
    address: Option<String>,
    /// How the run asks the person at the feeder: at this terminal, or as
    /// JSON lines on standard output and input for the bench server, which
    /// runs it as a job. The frame route (--ssid) asks at a terminal only,
    /// since it reads the passphrase there.
    #[arg(long, value_enum, default_value_t)]
    prompts: Prompts,
}

/// A run that cannot ask the way it was told to.
#[derive(Debug, thiserror::Error)]
enum PromptsRefused {
    #[error(
        "--ssid reads the network's passphrase at a terminal, so it does not run with \
         --prompts lines; nothing was started. Leave it out on the bench, whose stock already reaches the broker"
    )]
    Frame,
    #[error(
        "--home-ssid reads the network's passphrase at a terminal, so it does not run with \
         --prompts lines; nothing was started. Pass --home-network <file> instead"
    )]
    HomeNetwork,
}

/// A stock config the install will not run on.
#[derive(Debug, thiserror::Error)]
enum ConfigRefused {
    /// The server would answer stock's plan request with this snapshot,
    /// and stock replaces its stored table with whatever a reply carries
    /// (an empty list erases it), before the run can import the owner's
    /// plans.
    #[error(
        "{path} holds a plan snapshot, which the stock server would send stock in place of its own plans; nothing was started. Use an installer's config (wellfed-stock init-installer), or remove the `plans` key"
    )]
    PlanSnapshot { path: PathBuf },
}

/// What a run refuses before anything starts, beyond a file it cannot read,
/// and the run's own refusals in this command's words.
#[derive(Debug, thiserror::Error)]
enum Refused {
    #[error(
        "a consumer image trusts the release key, which this tool never holds: pass --update-key (created if missing) for the run to sign with and enroll, or --manifest"
    )]
    NoSigner,
    #[error(
        "a consumer image joins no network until Bluetooth setup gives it one: pass --home-network <file> or --home-ssid <network> (or --ssid)"
    )]
    NoHomeNetwork,
    #[error(
        "a bench image joins its build network; --home-ssid and --home-network are for a consumer image"
    )]
    HomeForBench,
    /// The broker enrolled another device than the owner pinned: whatever
    /// reached it first is not the feeder the owner means to install.
    #[error(
        "the broker enrolled device {enrolled}, connecting from {address}, but --expect-device is {expected}; nothing was sent to stock. Check which device reached the broker before running again"
    )]
    OtherDevice {
        expected: String,
        enrolled: String,
        address: String,
    },
    #[error(
        "the build moves the motor at startup ({policy}): sign it attended (wellfed-build sign --attended-motion) and pass --manifest"
    )]
    Motion { policy: String },
    #[error("the review's image is build {signed}, not {served}")]
    OtherBuild { signed: String, served: String },
    #[error("{} is not under a checkout's private/ directory, where reviews are signed", review.display())]
    NotPrivate { review: PathBuf },
    /// `--manifest` names its own signer, which the run cannot see: an
    /// update key beside it would be enrolled for nothing, or the manifest
    /// refused for want of its own.
    #[error(
        "--manifest and --update-key are exclusive: a given manifest is promoted as signed, and the run enrolls no key for it; pass one or the other"
    )]
    KeyWithManifest,
    #[error(
        "feeder {feeder}, which this install paired with, did not answer where it last did, where stock connected from, or over mDNS: pass --address <ip> to resume, or remove installing.json under --backups if its store was reset"
    )]
    NotAnswering { feeder: String },
    #[error("pass --address: no image lease names where the feeder connected from")]
    NoAddress,
}

pub(crate) async fn run(args: InstallArgs) -> Result<()> {
    if args.prompts == Prompts::Lines && args.ssid.is_some() {
        return Err(PromptsRefused::Frame.into());
    }
    if args.prompts == Prompts::Lines && args.home_ssid.is_some() {
        return Err(PromptsRefused::HomeNetwork.into());
    }
    let config_path = std::path::absolute(&args.stock_config).context("--stock-config")?;
    let config: ServerConfig = wellfed_formats::load(&config_path)
        .with_context(|| format!("read {}", config_path.display()))?;
    if config.has_plan_snapshot() {
        return Err(ConfigRefused::PlanSnapshot { path: config_path }.into());
    }
    let directory = config_path.parent().context("config directory missing")?;
    let _lease = Lease::acquire_in(directory).context("bench lease")?;

    // Everything the run needs is read and checked before the feeder is
    // touched: a bad backup, a missing manifest or key, or a consumer run
    // with no network to give stops it here.
    let review = Review::open(&args.review)?;
    let backup = match &args.stock_backup {
        Some(path) => {
            let image = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
            let backup = StockBackup::open(image).context("--stock-backup")?;
            backup
                .verified()
                .context("--stock-backup is not stock's pinned image")?;
            Some(backup)
        }
        None => {
            eprintln!(
                "No --stock-backup: bank 1 is read from the replacement before promotion, \
                 which would overwrite the feeder's only copy of stock."
            );
            None
        }
    };

    let provisioning = review.provisioning();
    let route = match provisioning {
        Provisioning::Consumer => Route::Setup,
        Provisioning::Bench => Route::Recovery,
    };
    let signer = signer(&args, &review, update_key(&args, provisioning)?)?;
    let passphrase = match (&args.ssid, &args.home_ssid) {
        (None, None) => None,
        _ => Some(crate::passphrase()?),
    };
    let home = home_wifi(&args, route, passphrase.as_deref())?;
    let start = if args.from_bank0 {
        Start::Bank0
    } else {
        Start::Stock(provision(&args, &config, passphrase)?)
    };
    let address = args
        .address
        .as_deref()
        .map(wellfed_connect::parse_address)
        .transpose()
        .context("--address")?;
    // The stock server refuses the trial on such a build too; checked here
    // as well, so a refused run stops before the feeder is touched.
    if !args.from_bank0 {
        build_info::BUILD.require_reviewed()?;
    }

    let platform = Platform {
        prompter: Asker::standard(args.prompts),
        bluetooth: Bluez {
            address: args.ble_address,
        },
        discovery: Mdns,
        keys: OwnerKey::open(&args.keys, None, OwnerKey::system())?,
        report: Printer,
    };
    // After the Reset hold no plan is left to come due in the jump.
    let plan = Plan {
        review,
        start,
        signer,
        home,
        backup,
        expect_device: args.expect_device.clone(),
        feeder: args.feeder.clone(),
        address,
        unjoined: wellfed_install::Unjoined::Wait,
        backups: args.backups.clone(),
        stock: StockOptions {
            config: config_path,
            enroll: Duration::from_secs(args.enroll_minutes * 60),
            // The bench's page reads the stock server's status.
            api: Api::Loopback,
            bind: wellfed_install::Bind::Configured,
            stock_plans: stock_plans(&args),
        },
    };

    // The stock server's log, as `wellfed-stock serve` writes it: what it
    // heard from stock, and why it held stock's clock.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "wellfed_stock=info,mqtt5=warn".into()),
        )
        .with_writer(std::io::stderr)
        .try_init();

    // From here on a signal ends the run through its stop, so the stock
    // server's broker is never left on the MQTT port.
    wellfed_install::run(plan, platform, signalled()?)
        .await
        .map_err(explain)
}

/// Resolves on SIGTERM or SIGINT. Once seen, the handlers stay installed but
/// nothing listens, so a second Ctrl-C does not cut the stop short; the stop
/// is itself bounded.
pub(crate) fn signalled() -> Result<impl Future<Output = Stop>> {
    let mut terminate = signal(SignalKind::terminate()).context("handle SIGTERM")?;
    let mut interrupt = signal(SignalKind::interrupt()).context("handle SIGINT")?;
    Ok(async move {
        tokio::select! {
            _ = terminate.recv() => Stop::Terminate,
            _ = interrupt.recv() => Stop::Interrupt,
        }
    })
}

/// How the replacement is paired, by the review's provisioning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    /// A consumer image starts Bluetooth setup by itself.
    Setup,
    /// A bench image joins its build network and pairs through the
    /// recovery window.
    Recovery,
}

/// The run's error in this command's words: the flags that resume it, or
/// that answer a refusal, where the run names none.
/// Only the setup frame follows the Reset hold, which erases stock's plans.
/// Without it stock reached the broker some other way and may still hold them,
/// so its clock stays where it reads.
fn stock_plans(args: &InstallArgs) -> StockPlans {
    if args.ssid.is_some() {
        StockPlans::Erased
    } else {
        StockPlans::Unknown
    }
}

fn explain(error: wellfed_install::Error) -> anyhow::Error {
    use wellfed_install::Error;

    match error {
        Error::Steps(
            InstallError::UpdateKey(_)
            | InstallError::UpdateKeyUncertain
            | InstallError::NotAboveFloor { .. }
            | InstallError::NoVersionAbove { .. },
        ) => anyhow::Error::from(error).context(
            "Promotion refused: the feeder runs the replacement from bank 0 and still holds \
             stock in bank 1. Run again with --from-bank0 once the cause is fixed",
        ),
        Error::Backup(_) => anyhow::Error::from(error).context(
            "Promotion refused: no verified stock backup. The feeder runs the replacement from \
             bank 0 and still holds stock in bank 1; run again with --from-bank0 once a backup \
             can be read, or with --stock-backup <stock 1.1.15 OTA image>",
        ),
        Error::KeepGiven(_) => anyhow::Error::from(error).context(
            "Promotion refused: the --stock-backup copy could not be kept; the feeder runs the \
             replacement from bank 0 and still holds stock in bank 1",
        ),
        Error::Setup(_) => anyhow::Error::from(error).context(
            "Bluetooth setup failed: the feeder runs the replacement from bank 0 and still holds \
             stock in bank 1. Run again with --from-bank0; with no Wi-Fi stored, the \
             replacement starts setup again at its next boot",
        ),
        Error::OtherDevice {
            expected,
            enrolled,
            address,
        } => Refused::OtherDevice {
            expected,
            enrolled,
            address,
        }
        .into(),
        Error::NotAnswering { feeder } => Refused::NotAnswering {
            feeder: fingerprint(&feeder),
        }
        .into(),
        Error::NoHomeNetwork => Refused::NoHomeNetwork.into(),
        Error::NoAddress => Refused::NoAddress.into(),
        error => error.into(),
    }
}

/// The update key the run signs with: `--update-key` (created if
/// missing), or the bench key, never created here, for a bench review.
/// `None` with `--manifest`, which is promoted as signed; the two together
/// are refused.
fn update_key(args: &InstallArgs, provisioning: Provisioning) -> Result<Option<Key>> {
    match (&args.update_key, &args.manifest, provisioning) {
        (Some(_), Some(_), _) => Err(Refused::KeyWithManifest.into()),
        (Some(path), None, _) => open_update_key(path).map(Some),
        (None, Some(_), _) => Ok(None),
        (None, None, Provisioning::Bench) => {
            let path = Path::new(wellfed_signing::KEY_PATH);
            let key = Key::load(path).with_context(|| format!("load {}", path.display()))?;
            Ok(Some(key))
        }
        (None, None, Provisioning::Consumer) => Err(Refused::NoSigner.into()),
    }
}

/// What the promotion is signed with: `--manifest` as given, or the
/// review signed here with `key` above every version signed so far.
fn signer(args: &InstallArgs, review: &Review, key: Option<Key>) -> Result<Signer> {
    match (&args.manifest, key) {
        (Some(path), _) => {
            let manifest = std::fs::read(path).context("read --manifest")?;
            let version =
                manifest_version(&manifest).context("--manifest is not a signed manifest")?;
            let (frozen, _) = open_review(&args.review)?;
            frozen
                .require_same_image(&manifest)
                .context("--manifest does not match --review")?;
            Ok(Signer::Manifest { manifest, version })
        }
        (None, Some(key)) => {
            let (frozen, root) = frozen_review(&args.review, review.build())?;
            let backups = std::path::absolute(&args.backups).context("--backups")?;
            let above = signed_so_far(&root, &backups).map_or(0, |(version, _)| version);
            eprintln!(
                "The run signs build {} with update key {} once it has read the feeder's \
                 floor, above version {above}.",
                short(review.build()),
                wellfed_formats::hex::encode(&key.id())
            );
            Ok(Signer::Key {
                key: Box::new(key),
                frozen: Box::new(frozen),
                above,
            })
        }
        (None, None) => Err(Refused::NoSigner.into()),
    }
}

/// Load the owner's update key at `path`, creating one there first if
/// there is none, as `wellfed update-key new` would.
fn open_update_key(path: &Path) -> Result<Key> {
    if !path.exists() {
        let public = wellfed_signing::keygen(path, KeyScope::Owner)
            .with_context(|| format!("write {}", path.display()))?;
        eprintln!(
            "Created update key {} (public key {public}); it holds the private key: keep it \
             private.",
            path.display()
        );
    }
    Key::load(path).with_context(|| format!("load {}", path.display()))
}

/// The review, verified as the signer verifies it; refused when it is not
/// the served build, or when it moves the motor at startup (signed only
/// attended). With it, the checkout root the review sits under.
fn frozen_review(review: &Path, build: &str) -> Result<(Frozen, PathBuf)> {
    let (frozen, root) = open_review(review)?;
    if frozen.build_id() != build {
        return Err(Refused::OtherBuild {
            signed: frozen.build_id(),
            served: build.to_string(),
        }
        .into());
    }
    if let Some(policy) = frozen.motion_policy() {
        return Err(Refused::Motion {
            policy: policy.to_string(),
        }
        .into());
    }
    Ok((frozen, root))
}

/// The review, verified as the signer verifies it, and the checkout root it
/// sits under.
fn open_review(review: &Path) -> Result<(Frozen, PathBuf)> {
    let absolute = std::path::absolute(review).context("--review")?;
    let root = absolute
        .ancestors()
        .find(|dir| dir.file_name().is_some_and(|name| name == "private"))
        .and_then(Path::parent)
        .ok_or_else(|| Refused::NotPrivate {
            review: review.to_path_buf(),
        })?;
    let frozen = Frozen::open(root, &absolute).context("verify the review to sign it")?;
    Ok((frozen, root.to_path_buf()))
}

/// The Wi-Fi Bluetooth setup gives a consumer image: `--home-network`'s
/// file, or `--home-ssid` (or the frame's `--ssid`) with the passphrase
/// read once.
fn home_wifi(
    args: &InstallArgs,
    route: Route,
    passphrase: Option<&str>,
) -> Result<Option<Credentials>> {
    if route == Route::Recovery {
        return match (&args.home_ssid, &args.home_network) {
            (None, None) => Ok(None),
            _ => Err(Refused::HomeForBench.into()),
        };
    }
    let network = match (&args.home_network, &args.home_ssid, &args.ssid, passphrase) {
        (Some(path), _, _, _) => Some(home_network(path)?),
        (None, Some(ssid), _, Some(passphrase)) | (None, None, Some(ssid), Some(passphrase)) => {
            Some((ssid.clone(), passphrase.to_string()))
        }
        _ => None,
    };
    match network {
        Some((ssid, password)) => Ok(Some(
            Credentials::new(ssid.into_bytes(), password).context("the home network")?,
        )),
        // A resumed run may find the feeder on the network already.
        None if args.from_bank0 => Ok(None),
        None => Err(Refused::NoHomeNetwork.into()),
    }
}

/// The SSID and passphrase in `path`, a file shaped as the bench's
/// `private/hotspot.json` is. Neither the error nor anything else here
/// prints the passphrase.
fn home_network(path: &Path) -> Result<(String, String)> {
    let WifiCredential { ssid, password } = wellfed_formats::load(path)
        .with_context(|| format!("read the home network from {}", path.display()))?;
    Ok((ssid, password))
}

/// How stock reaches the installer's broker: the setup frame after the Reset
/// hold (`--ssid`: the owner's network and the installer's broker), or already.
fn provision(
    args: &InstallArgs,
    config: &ServerConfig,
    passphrase: Option<String>,
) -> Result<Provision> {
    let Some(ssid) = &args.ssid else {
        return Ok(Provision::Done);
    };
    let broker = args.broker.unwrap_or(config.mqtt_listen);
    let Some(password) = passphrase else {
        bail!("the frame's passphrase was not read");
    };
    Ok(Provision::Frame(SetupFrame {
        ssid: ssid.clone(),
        password,
        host: broker.ip().to_string(),
        port: broker.port(),
        id: args.account.clone(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_the_setup_frame_says_stock_s_plans_are_erased() {
        let frame = args(&["--ssid", "Maple St"]);
        assert_eq!(stock_plans(&frame), StockPlans::Erased);
        assert_eq!(stock_plans(&args(&[])), StockPlans::Unknown);
    }

    #[derive(clap::Parser)]
    struct Line {
        #[command(flatten)]
        args: InstallArgs,
    }

    /// A config with a plan snapshot, even an empty one, is refused before
    /// the lease, the server or the feeder: the server would send stock
    /// that snapshot in place of its own plans.
    #[tokio::test]
    async fn a_config_with_a_plan_snapshot_is_refused_before_anything_starts() {
        use clap::Parser;

        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("server.json");
        std::fs::write(
            &config,
            json!({"api_token": "t", "http_listen": "127.0.0.1:8765",
                   "mqtt_listen": "10.42.103.1:1883", "plans": []})
            .to_string(),
        )
        .unwrap();
        let line = Line::parse_from([
            "install",
            "--review",
            "missing-review",
            "--manifest",
            "missing.bin",
            "--stock-config",
            config.to_str().unwrap(),
        ]);

        let error = run(line.args).await.unwrap_err();
        assert!(matches!(
            error.downcast_ref::<ConfigRefused>(),
            Some(ConfigRefused::PlanSnapshot { .. })
        ));
        // Nothing beside the config: no lease (nor its lock), no enrollment.
        let beside: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(beside, ["server.json"]);
    }

    /// The frame route reads its passphrase at a terminal, so a run asked
    /// to prompt in lines is refused before it reads anything.
    #[tokio::test]
    async fn the_frame_route_is_refused_with_prompts_in_lines() {
        use clap::Parser;

        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("server.json");
        let line = Line::parse_from([
            "install",
            "--review",
            "missing-review",
            "--manifest",
            "missing.bin",
            "--stock-config",
            config.to_str().unwrap(),
            "--ssid",
            "Home",
            "--prompts",
            "lines",
        ]);

        let error = run(line.args).await.unwrap_err();
        assert!(matches!(
            error.downcast_ref::<PromptsRefused>(),
            Some(PromptsRefused::Frame)
        ));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);

        // A consumer image's home network reads its passphrase the same way.
        let home = Line::parse_from([
            "install",
            "--review",
            "missing-review",
            "--stock-config",
            config.to_str().unwrap(),
            "--home-ssid",
            "Home",
            "--prompts",
            "lines",
        ]);
        let error = run(home.args).await.unwrap_err();
        assert!(matches!(
            error.downcast_ref::<PromptsRefused>(),
            Some(PromptsRefused::HomeNetwork)
        ));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    fn args(line: &[&str]) -> InstallArgs {
        use clap::Parser;

        let base = [
            "install",
            "--review",
            "private/review",
            "--stock-config",
            "private/installer.json",
        ];
        Line::parse_from([&base[..], line].concat()).args
    }

    /// A consumer image trusts only the release key and has no network:
    /// the run needs a key of its own (or a manifest) and the network to
    /// give it, before anything starts.
    #[test]
    fn a_consumer_run_without_a_signer_or_a_network_is_refused() {
        fn refused<T>(result: Result<T>) -> Option<Refused> {
            result
                .err()
                .and_then(|error| error.downcast::<Refused>().ok())
        }

        let bare = args(&[]);
        assert!(matches!(
            refused(update_key(&bare, Provisioning::Consumer)),
            Some(Refused::NoSigner)
        ));
        let given = args(&["--manifest", "private/v9.manifest"]);
        assert!(
            update_key(&given, Provisioning::Consumer)
                .unwrap()
                .is_none()
        );

        assert!(matches!(
            refused(home_wifi(&bare, Route::Setup, None)),
            Some(Refused::NoHomeNetwork)
        ));
        let resumed = args(&["--from-bank0"]);
        assert_eq!(home_wifi(&resumed, Route::Setup, None).unwrap(), None);
        let home = args(&["--home-ssid", "Home"]);
        assert!(
            home_wifi(&home, Route::Setup, Some("pass word!"))
                .unwrap()
                .is_some()
        );
        // The frame's network is the home network too.
        let frame = args(&["--ssid", "Home"]);
        assert!(
            home_wifi(&frame, Route::Setup, Some("pass word!"))
                .unwrap()
                .is_some()
        );
        assert!(matches!(
            refused(home_wifi(&home, Route::Recovery, Some("pass word!"))),
            Some(Refused::HomeForBench)
        ));
        assert_eq!(home_wifi(&frame, Route::Recovery, Some("x")).unwrap(), None);
    }

    /// `--home-network` gives a consumer image the network from a file, so
    /// it reads nothing at a terminal: it runs with lines prompts, and it
    /// takes the place of `--home-ssid` and `--ssid`, never beside them.
    #[tokio::test]
    async fn a_home_network_file_runs_with_lines_prompts_and_alone() {
        use clap::Parser;

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("hotspot.json");
        std::fs::write(
            &file,
            json!({"connection": "bench", "ssid": "Bench", "password": "pass word!"}).to_string(),
        )
        .unwrap();
        let path = file.to_str().unwrap();

        let given = args(&["--home-network", path]);
        let wifi = home_wifi(&given, Route::Setup, None).unwrap();
        assert!(wifi.is_some());
        assert!(matches!(
            home_wifi(&given, Route::Recovery, None)
                .unwrap_err()
                .downcast::<Refused>(),
            Ok(Refused::HomeForBench)
        ));
        let missing = args(&["--home-network", "private/none.json"]);
        let error = home_wifi(&missing, Route::Setup, None).unwrap_err();
        assert!(
            format!("{error:#}").contains("private/none.json"),
            "{error:#}"
        );

        // Refused before it reads anything only for a missing review: the
        // prompts mode lets it through.
        let config = dir.path().join("server.json");
        let line = Line::parse_from([
            "install",
            "--review",
            "missing-review",
            "--stock-config",
            config.to_str().unwrap(),
            "--home-network",
            path,
            "--prompts",
            "lines",
        ]);
        let error = run(line.args).await.unwrap_err();
        assert!(
            error.downcast_ref::<PromptsRefused>().is_none(),
            "{error:#}"
        );
        assert!(!format!("{error:#}").contains("pass word!"), "{error:#}");

        for other in [["--home-ssid", "Home"], ["--ssid", "Home"]] {
            let line = Line::try_parse_from(
                [
                    &["install", "--review", "r", "--stock-config", "c"][..],
                    &["--home-network", path],
                    &other,
                ]
                .concat(),
            );
            assert!(line.is_err(), "{other:?}");
        }
    }

    /// `--update-key` names a file the run creates once and reuses.
    #[test]
    fn the_update_key_is_created_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update-key.json");
        let created = open_update_key(&path).unwrap();
        assert_eq!(created.scope(), KeyScope::Owner);
        assert_eq!(open_update_key(&path).unwrap().public(), created.public());
    }

    /// A given manifest is promoted as signed: an update key beside it is
    /// refused rather than enrolled for nothing.
    #[test]
    fn an_update_key_beside_a_manifest_is_refused() {
        let both = args(&[
            "--manifest",
            "private/v9.manifest",
            "--update-key",
            "private/update-key.json",
        ]);
        let Err(refused) = update_key(&both, Provisioning::Bench) else {
            panic!("a key beside a manifest is refused");
        };
        assert!(matches!(
            refused.downcast_ref::<Refused>(),
            Some(Refused::KeyWithManifest)
        ));
    }

    /// The run's refusals that name a flag are worded with it, and a
    /// stopped run's report is the run's own.
    #[test]
    fn the_run_s_refusals_name_this_command_s_flags() {
        use wellfed_install::Error;

        let error = explain(Error::OtherDevice {
            expected: "AF_OTHER".into(),
            enrolled: "AF_FEEDER".into(),
            address: "10.42.103.172".into(),
        });
        match error.downcast_ref::<Refused>() {
            Some(Refused::OtherDevice {
                expected,
                enrolled,
                address,
            }) => assert_eq!(
                (expected.as_str(), enrolled.as_str(), address.as_str()),
                ("AF_OTHER", "AF_FEEDER", "10.42.103.172")
            ),
            other => panic!("{other:?}"),
        }
        assert!(error.to_string().contains("--expect-device is AF_OTHER"));

        let error = explain(Error::NotAnswering { feeder: [0xab; 32] });
        assert!(error.to_string().contains("feeder abababababababab"));
        assert!(error.to_string().contains("pass --address <ip> to resume"));
    }
}
