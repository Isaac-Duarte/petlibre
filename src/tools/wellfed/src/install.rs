//! `wellfed install`: one run from a feeder on stock 1.1.15 to the
//! replacement confirmed on bank 1 ([consumer installation]).
//!
//! The steps and every decision between them are `wellfed_client::install`'s,
//! the same the Android app runs; this module is their IO for a computer:
//! - stock's side is `wellfed-stock`, run as child processes (`serve`, which
//!   trusts the first feeder that connects during the run, and
//!   `ota-images`), spoken to over its loopback API;
//! - stock reaches the broker DNS first: the owner's resolver answers
//!   stock's broker name with this computer and the owner restarts the
//!   feeder (the run guides it and looks the name up as advice; stock
//!   arriving at the broker is the check);
//! - or, the fallback, the setup frame: one UDP datagram, sent after the
//!   owner accepted that the Reset hold erases stock's plans and joined
//!   this computer to the feeder's setup access point (joining Wi-Fi is the
//!   platform's job, so the run waits for the owner to say it is done);
//! - the replacement is reached over the device API, with this computer's
//!   key, enrolled through the recovery window when the build does not know
//!   it yet.
//!
//! The run holds the bench lease beside the stock config for its whole
//! length; `serve` runs on it as a descendant. Nothing is retried that the
//! steps say must not be: the install request goes once, and a refusal or
//! an uncertain outcome ends the run; so does the put of stock's plans,
//! which an unanswered put judges by reading the plans back.
//!
//! What the run asks the owner goes through one `Asker`, at a terminal or,
//! with `--prompts lines`, as JSON lines for the bench server that runs it
//! as a job. SIGTERM (a job's cancel) and SIGINT end the run through the
//! same stop as its end. The report names the step it stopped in and any
//! write whose outcome had not settled (the request until the replacement
//! answers, the promotion until bank 1 confirms it, the plans' put until
//! the end); that write is uncertain, and never sent again.
//!
//! [consumer installation]: ../../../../docs/firmware/consumer-installation.md

use std::io::Write;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Args;
use serde::de::IgnoredAny;
use tokio::process::{Child, Command};
use tokio::signal::unix::{SignalKind, signal};
use wellfed_client::install::{
    CONTAINER_HEADER, DnsCheck, DnsOverride, FeederTrust, Install, InstallError, Phase, PlansPut,
    Provision, Replacement, Requested, SETUP_ADDRESS, STOCK_BROKER_PORT, SetupFrame, Step,
    StockBackup, StockPower, StockView, manifest_version, plans,
};
use wellfed_client::update::{Image, InstallError as UpdateError};
use wellfed_formats::bench::lease::Lease;
use wellfed_formats::bench::prompt::Prompt;
use wellfed_formats::manifest::BuildManifest;
use wellfed_formats::stock::{
    ImageLease, OtaReport, ServerConfig, StockStatus, TrialAnswer, TrialPhase, TrialRequest,
};
use wellfed_model::{Answer, Generation, Op, PLANS, Plan, Plans};
use wellfed_proto::api::PORT;
use wellfed_stock_api::{ApiError, Auth, StockApi};

use crate::ask::{Asker, Prompts};
use crate::keys::{OwnerKey, fingerprint};
use crate::push::{Pushed, push};

/// How long the stock server's loopback API gets per call.
const API_TIMEOUT: Duration = Duration::from_secs(10);
/// How often the run looks at stock's status and the replacement.
const POLL: Duration = Duration::from_secs(5);
/// Stock joins the owner's Wi-Fi and reports within a minute or two of the
/// frame; the enrollment window bounds this wait.
const STOCK_GRACE: Duration = Duration::from_secs(5 * 60);
/// Stock downloads in about 40 s, then restarts; generous, as on the bench.
const INSTALL_WAIT: Duration = Duration::from_secs(10 * 60);
/// From stock's restart until the replacement answers the device API.
const REPLACEMENT_WAIT: Duration = Duration::from_secs(5 * 60);
/// The stock server refuses to close its record while stock's telemetry is
/// current (90 s after its last report).
const CLOSE_WAIT: Duration = Duration::from_secs(150);
/// Bank 1's trial boot confirms within a few minutes of activation.
const CONFIRM_WAIT: Duration = Duration::from_secs(6 * 60);
/// The image server's lease appears within seconds of its start.
const IMAGES_WAIT: Duration = Duration::from_secs(30);
/// How long a child process gets to exit after SIGTERM.
const STOP_WAIT: Duration = Duration::from_secs(5);
/// Reading the plans back may wait for a session to open again.
const PLANS_WAIT: Duration = Duration::from_secs(60);

#[derive(Args)]
pub(crate) struct InstallArgs {
    /// The replacement's frozen review (its `review.json`, `image.bin` and
    /// `build-manifest.json`): what stock installs into bank 0.
    #[arg(long)]
    review: PathBuf,
    /// The signed update manifest for the same build, which bank 0 installs
    /// into bank 1 at promotion.
    #[arg(long)]
    manifest: PathBuf,
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
    /// Stock's broker name (the vendor's, which Wellfed does not ship):
    /// the owner's resolver is to answer it with this computer, and the
    /// restarted feeder connects here with nothing written to it, so its
    /// plans stay for the import. The DNS-first route.
    #[arg(long, conflicts_with_all = ["ssid", "from_bank0"])]
    broker_name: Option<String>,
    /// The fallback where the router cannot override a name: the owner
    /// holds Reset, which erases stock's plans, and the setup frame points
    /// stock at this network (passphrase from standard input) and the
    /// installer's broker. Without it or --broker-name, stock already
    /// connects to the installer's broker.
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
    /// refused.
    #[arg(long)]
    stock_config: PathBuf,
    /// The `wellfed-stock` binary; beside this one otherwise.
    #[arg(long)]
    stock_server: Option<PathBuf>,
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
    /// Promote even if stock's stored plans cannot be read (a cut save, or
    /// a build that does not serve them). Without it the run stops before
    /// promotion, with stock intact in bank 1.
    #[arg(long)]
    without_stock_plans: bool,
    /// Have the stock server set stock's clock even when its own reading is
    /// unknown or more than 10 minutes off. Stock may refuse the install
    /// with its clock unset, but once set it may run meals that came due in
    /// the jump.
    #[arg(long)]
    calibrate_clock: bool,
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
         --prompts lines; nothing was started. Use --broker-name on the bench"
    )]
    Frame,
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

/// The run's IO state beside the steps.
struct Run {
    args: InstallArgs,
    config_path: PathBuf,
    api: StockApi,
    stock_bin: PathBuf,
    key: OwnerKey,
    /// The replacement's image (an OTA container) and its update manifest.
    container: Vec<u8>,
    manifest: Vec<u8>,
    /// `serve` and `ota-images`, killed when the run ends.
    serve: Option<Child>,
    images: Option<Child>,
    /// The one request's id, once made.
    request: Option<uuid::Uuid>,
    /// Where the feeder connected from (the image lease's peer).
    peer: Option<IpAddr>,
    /// The feeder's identity, once its key is known.
    identity: Option<[u8; 32]>,
    /// The `--stock-backup` copy, kept under `--backups` before promotion
    /// once the feeder's identity is known.
    given: Option<StockBackup>,
    asker: Asker<Box<dyn Write>>,
    /// Where the run is, for the report of a signal that ends it.
    track: Track,
}

/// A write the run sent whose outcome is not settled yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unsettled {
    /// Stock's install request, under this id: settled once the
    /// replacement answers its status.
    Request(uuid::Uuid),
    /// The signed update, pushed from bank 0 into bank 1: settled once
    /// bank 1 confirms it.
    Promotion,
    /// Stock's converted plans, put on the replacement: settled once the
    /// run ends, a put that went unanswered being judged by a read back.
    Plans,
}

impl Unsettled {
    /// Whether the run reaching `phase` settles this write.
    fn settled_at(self, phase: Phase) -> bool {
        match self {
            Self::Request(_) => phase > Phase::ReadStatus,
            Self::Promotion => phase > Phase::AwaitConfirm,
            Self::Plans => phase == Phase::Done,
        }
    }
}

/// The steps' phase, as the report of a stopped run names it.
const fn phase_text(phase: Phase) -> &'static str {
    match phase {
        Phase::Provision => "pointing stock at this computer, before anything was sent to it",
        Phase::AwaitStock => "waiting for stock to report, before anything was sent to it",
        Phase::ConfirmFeeder => "the feeder check, before anything was sent to stock",
        Phase::Request => "the install request",
        Phase::AwaitInstall => "stock's install of the replacement",
        Phase::Pair => "pairing through the recovery window",
        Phase::ReadStatus => "reading the replacement's status",
        Phase::CloseTrial => "closing the stock server's record",
        Phase::Backup => "reading the stock backup",
        Phase::StockPlans => "reading stock's plans",
        Phase::Promote => "the promotion",
        Phase::AwaitConfirm => "waiting for bank 1 to confirm the promotion",
        Phase::ApplyPlans => "applying stock's plans to the replacement",
        Phase::Done => "the end of the run",
    }
}

impl std::fmt::Display for Unsettled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Request(id) => write!(f, "stock's install request {id}"),
            Self::Promotion => f.write_str("the promotion's push"),
            Self::Plans => f.write_str("the put of stock's plans"),
        }
    }
}

/// The phase the run is in and the write it has not seen settle.
#[derive(Debug, Clone, Copy)]
struct Track {
    phase: Phase,
    unsettled: Option<Unsettled>,
}

impl Track {
    const fn new() -> Self {
        Self {
            phase: Phase::Provision,
            unsettled: None,
        }
    }

    /// The run moved on to `phase`; a write that phase settles is no
    /// longer out.
    fn enter(&mut self, phase: Phase) {
        self.phase = phase;
        if self.unsettled.is_some_and(|write| write.settled_at(phase)) {
            self.unsettled = None;
        }
    }

    /// `write` is about to go out. It stays unsettled from here on, even
    /// before its answer, since a signal may come at any later await.
    fn sent(&mut self, write: Unsettled) {
        self.unsettled = Some(write);
    }

    fn stopped(self, signal: Signal) -> Stopped {
        Stopped {
            signal,
            phase: self.phase,
            unsettled: self.unsettled,
        }
    }
}

/// The signals that end a run through its cleanup: a job's cancel sends
/// SIGTERM, a terminal's Ctrl-C SIGINT.
#[derive(Debug, Clone, Copy)]
enum Signal {
    Terminate,
    Interrupt,
}

impl std::fmt::Display for Signal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Terminate => "SIGTERM",
            Self::Interrupt => "SIGINT",
        })
    }
}

/// A run a signal ended, after it stopped the stock server: the step it
/// was in, and the write whose outcome had not settled, if any.
#[derive(Debug, thiserror::Error)]
#[error(
    "stopped by {signal} during {}{}",
    phase_text(*.phase),
    unsettled_note(*.phase, *.unsettled)
)]
struct Stopped {
    signal: Signal,
    phase: Phase,
    unsettled: Option<Unsettled>,
}

/// What the report adds after the phase. Pairing is only an owner
/// enrollment, safe to repeat, but a write still unsettled beside it is not,
/// so the two are told apart.
fn unsettled_note(phase: Phase, unsettled: Option<Unsettled>) -> String {
    let pairing = phase == Phase::Pair;
    match unsettled {
        None if pairing => {
            " (an owner enrollment, safe to run again); no write was waiting on its outcome"
                .to_string()
        }
        None => "; no write was waiting on its outcome".to_string(),
        Some(write) => {
            let enrollment = if pairing {
                " The enrollment is safe to repeat; that write is not."
            } else {
                ""
            };
            format!(
                ", with {write} sent and its outcome not yet settled: it may have run, and it is \
                 not sent again.{enrollment} Read what the feeder runs (wellfed status, or \
                 stock's own report) before running again"
            )
        }
    }
}

pub(crate) async fn run(args: InstallArgs) -> Result<()> {
    if args.prompts == Prompts::Lines && args.ssid.is_some() {
        return Err(PromptsRefused::Frame.into());
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
    // touched: a bad backup or a missing manifest stops it here.
    let build = review_build(&args.review)?;
    let container = std::fs::read(args.review.join("image.bin")).context("read image.bin")?;
    let manifest = std::fs::read(&args.manifest).context("read --manifest")?;
    let backup = match &args.stock_backup {
        Some(path) => {
            let image = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
            Some(StockBackup::open(image).context("--stock-backup")?)
        }
        None => {
            eprintln!(
                "No --stock-backup: bank 1 is read from the replacement before promotion, \
                 which would overwrite the feeder's only copy of stock."
            );
            None
        }
    };

    let version = manifest_version(&manifest).context("--manifest is not a signed manifest")?;
    let mut install = if args.from_bank0 {
        Install::from_bank0(&build, version)
    } else {
        Install::new(provision(&args, &config)?, &build, version)?
    };
    if args.without_stock_plans {
        install.without_stock_plans();
    }
    if let Some(backup) = &backup {
        install.backup(
            backup
                .verified()
                .context("--stock-backup is not stock's pinned image")?,
        );
    }

    // From here on a signal ends the run through `stop`, so the stock
    // server's broker child is never left on the MQTT port.
    let mut terminate = signal(SignalKind::terminate()).context("handle SIGTERM")?;
    let mut interrupt = signal(SignalKind::interrupt()).context("handle SIGINT")?;
    let mut run = Run {
        stock_bin: stock_bin(args.stock_server.as_deref())?,
        api: StockApi::new(&config, API_TIMEOUT),
        key: OwnerKey::open(&args.keys, None, OwnerKey::system())?,
        // After the passphrase, which `provision` read from standard input.
        asker: Asker::standard(args.prompts),
        args,
        config_path,
        container,
        manifest,
        serve: None,
        images: None,
        request: None,
        peer: None,
        identity: None,
        given: backup,
        track: Track::new(),
    };
    // A signal is seen at the run's next await: one that lands during a
    // blocking call (a stock API request, the frame's socket) waits for it.
    // Once seen, the handlers stay installed but nothing listens, so a
    // second Ctrl-C does not cut `stop` short; `stop` is itself bounded.
    let ended = tokio::select! {
        result = run.drive(&mut install) => Ok(result),
        _ = terminate.recv() => Err(Signal::Terminate),
        _ = interrupt.recv() => Err(Signal::Interrupt),
    };
    run.stop().await;

    match ended {
        Ok(result) => result,
        Err(signal) => Err(run.track.stopped(signal).into()),
    }
}

/// The startup identity the review's build manifest records; the image
/// server checks it against the review's digests before serving.
fn review_build(review: &Path) -> Result<String> {
    let manifest: BuildManifest = wellfed_formats::load(&review.join("build-manifest.json"))
        .context("read the review's build-manifest.json")?;
    let id = manifest.startup_identity.unwrap_or_default();
    if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("the review's build-manifest.json has no 64-hex startup identity");
    }
    Ok(id.to_ascii_lowercase())
}

/// How stock reaches the installer's broker: a DNS override of its
/// broker name (`--broker-name`), the setup frame after the Reset hold
/// (`--ssid`: the owner's network and the installer's broker), or already.
fn provision(args: &InstallArgs, config: &ServerConfig) -> Result<Provision> {
    if let Some(name) = &args.broker_name {
        return Ok(Provision::Dns(
            DnsOverride::new(name, config.mqtt_listen).context("--broker-name")?,
        ));
    }
    let Some(ssid) = &args.ssid else {
        return Ok(Provision::Done);
    };
    let broker = args.broker.unwrap_or(config.mqtt_listen);
    Ok(Provision::Frame(SetupFrame {
        ssid: ssid.clone(),
        password: crate::passphrase()?,
        host: broker.ip().to_string(),
        port: broker.port(),
        id: args.account.clone(),
    }))
}

/// `wellfed-stock`, as given or beside this binary.
fn stock_bin(given: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = given {
        return Ok(path.to_path_buf());
    }
    let me = std::env::current_exe()?;
    let beside = me.with_file_name("wellfed-stock");
    if !beside.exists() {
        bail!(
            "{} not found; build it (cargo build -p wellfed-stock) or pass --stock-server",
            beside.display()
        );
    }
    Ok(beside)
}

/// Whether a session answering from `bank` with `build` shows the
/// promotion already activated. During a promotion that started on bank 0,
/// bank 1 can only run the served build once bank 0 activated it.
fn activated(bank: u8, build: &str, served: &str) -> bool {
    bank == 1 && build.eq_ignore_ascii_case(served)
}

/// A child's standard output, sent to this process's standard error: with
/// `--prompts lines` this process's standard output carries prompts only,
/// and in a terminal both show alike.
fn onto_stderr() -> Result<Stdio> {
    let stderr = std::io::stderr()
        .as_fd()
        .try_clone_to_owned()
        .context("duplicate standard error for a child")?;
    Ok(Stdio::from(stderr))
}

/// SIGTERM, up to [`STOP_WAIT`] to exit, then SIGKILL.
async fn stop_child(mut child: Child) {
    let pid = child
        .id()
        .and_then(|id| i32::try_from(id).ok())
        .and_then(rustix::process::Pid::from_raw);
    if let Some(pid) = pid
        && rustix::process::kill_process(pid, rustix::process::Signal::TERM).is_ok()
        && tokio::time::timeout(STOP_WAIT, child.wait()).await.is_ok()
    {
        return;
    }
    let _ = child.kill().await;
}

/// Whether the owner recognizes the feeder the broker trusted. Trust on
/// first use admits whichever stock-shaped client reached the broker first;
/// a host on the LAN that won that race could otherwise be walked through
/// the whole install, so the owner checks the device ID (on the feeder's
/// label or in the vendor app) and the address (the router's client list)
/// before anything is sent.
async fn recognized(asker: &mut Asker<Box<dyn Write>>, trust: FeederTrust) -> Result<bool> {
    let (device, address) = match trust {
        FeederTrust::Enrolled { device, address } => (device, address),
        // The bench: its credentials came from a capture of this feeder.
        FeederTrust::Captured => return Ok(true),
        FeederTrust::Contested => return Ok(false),
    };
    Ok(asker
        .yes(Prompt::RecognizeFeeder { device, address })
        .await?)
}

/// Guide the owner through the DNS override, then look the name up from
/// this computer and show what it found, before the feeder restarts. The
/// lookup is advice only: this computer may ask another resolver than the
/// feeder does, so the run waits for stock at the broker whatever it shows.
async fn point_dns(asker: &mut Asker<Box<dyn Write>>, dns: &DnsOverride) -> Result<()> {
    let (name, broker) = (dns.name(), dns.broker());
    asker
        .done(Prompt::SetOverride {
            name: name.to_string(),
            broker,
        })
        .await?;

    let resolved: Vec<IpAddr> = tokio::net::lookup_host((name, STOCK_BROKER_PORT))
        .await
        .map(|addresses| addresses.map(|address| address.ip()).collect())
        .unwrap_or_default();
    match dns.check(&resolved) {
        DnsCheck::Points => eprintln!("This computer resolves {name} to {broker}, as it should."),
        DnsCheck::Mixed(addresses) => eprintln!(
            "This computer resolves {name} to {addresses:?}: {broker} and others. Stock may \
             take any of them, so make {broker} the only answer."
        ),
        DnsCheck::Elsewhere(addresses) => eprintln!(
            "This computer resolves {name} to {addresses:?}, not {broker}. If it asks the \
             same resolver as the feeder, the override is not in place yet."
        ),
        DnsCheck::Unresolved => eprintln!("This computer cannot resolve {name}."),
    }
    Ok(())
}

/// Ask the owner to restart the feeder, with the broker already listening.
async fn restart_feeder(asker: &mut Asker<Box<dyn Write>>) -> Result<()> {
    asker.done(Prompt::RestartFeeder).await?;
    eprintln!(
        "Waiting for stock to connect here. If it does not within a few minutes, check the \
         override in the resolver your router hands out and restart the feeder again; \
         nothing is sent to stock until it does."
    );
    Ok(())
}

impl Run {
    async fn drive(&mut self, install: &mut Install) -> Result<()> {
        loop {
            let step = install.step();
            self.track.enter(install.phase());
            match step {
                Step::PointDns(dns) => {
                    point_dns(&mut self.asker, &dns).await?;
                    // The enrollment window opens only now, just before
                    // stock restarts into it.
                    self.serve().await?;
                    restart_feeder(&mut self.asker).await?;
                    install.dns_pointed();
                }
                Step::ConfirmReset => {
                    // Warned up front: the Reset hold erases stock's plans
                    // before the installer can read them.
                    if self.asker.yes(Prompt::EraseStockPlans).await? {
                        install.reset_accepted();
                    } else {
                        install.reset_declined();
                    }
                }
                Step::SendFrame(frame) => {
                    self.serve().await?;
                    self.send_frame(frame.bytes()).await?;
                    install.frame_sent();
                }
                Step::AwaitStock => {
                    self.serve().await?;
                    self.await_stock(install).await?;
                }
                Step::ConfirmFeeder(trust) => {
                    if recognized(&mut self.asker, trust).await? {
                        install.feeder_confirmed();
                    } else {
                        install.feeder_rejected();
                    }
                }
                Step::RequestInstall => {
                    let outcome = self.request().await?;
                    install.requested(outcome);
                }
                Step::AwaitInstall => self.await_install(install).await?,
                Step::Pair => {
                    self.pair(install).await?;
                }
                Step::ReadStatus => {
                    let (build, bank) = self.status().await?;
                    eprintln!(
                        "The replacement answers: build {} on bank {bank}",
                        short(&build)
                    );
                    install.replacement(Replacement {
                        build: &build,
                        bank,
                    });
                }
                Step::CloseTrial => {
                    self.close_trial(install.build()).await;
                    install.trial_closed();
                }
                Step::Backup => {
                    let backup = self.backup().await.context(
                        "Promotion refused: no verified stock backup. The feeder runs the \
                         replacement from bank 0 and still holds stock in bank 1; run again \
                         with --from-bank0 once a backup can be read, or with \
                         --stock-backup <stock 1.1.15 OTA image>",
                    )?;
                    install.backup(
                        backup
                            .verified()
                            .context("--stock-backup is not stock's pinned image")?,
                    );
                }
                Step::ReadStockPlans => self.stock_plans(install).await?,
                Step::Promote => {
                    self.keep_given()?;
                    let outcome = self.promote(install.build()).await?;
                    install.promoted(outcome);
                }
                Step::AwaitConfirm => {
                    self.kept_plans(install)?;
                    self.await_confirm(install).await?;
                }
                Step::ReadPlans => self.read_plans(install).await?,
                Step::PutPlans { base, plans } => {
                    let outcome = self.put_plans(base, plans).await?;
                    install.plans_put(outcome);
                }
                Step::AwaitPut => bail!("the plans' put was answered before this step"),
                Step::Done => {
                    eprintln!(
                        "Installed: build {} confirmed on bank 1. This computer is its owner.",
                        short(install.build())
                    );
                    match (install.import(), install.plans_skipped()) {
                        (_, Some(skipped)) => {
                            eprintln!("Stock's plans were not imported: {skipped}.");
                        }
                        (None, None) => eprintln!(
                            "Stock's plans were not read by this run, and no earlier run kept \
                             a copy it has not applied: nothing was set."
                        ),
                        (Some(import), None) if import.plans.iter().any(Option::is_some) => {
                            eprintln!("Stock's converted plans are set on the replacement.");
                            self.kept_plans_applied()?;
                        }
                        (Some(import), None) if import.outcomes.is_empty() => {
                            eprintln!("Stock had no plans to set on the replacement.");
                        }
                        (Some(_), None) => eprintln!(
                            "None of stock's plans could be converted (see the report above): \
                             nothing was set."
                        ),
                    }
                    return Ok(());
                }
                Step::Failed(
                    error @ (InstallError::StockPlans(_) | InstallError::StockPlansUnread(_)),
                ) => {
                    return Err(anyhow::Error::from(error).context(
                        "Promotion refused: nothing was promoted, and the feeder runs the \
                         replacement from bank 0 and still holds stock in bank 1. Run again with \
                         --from-bank0 --without-stock-plans to install without stock's plans",
                    ));
                }
                Step::Failed(error @ InstallError::StockClockUnset)
                    if !self.args.calibrate_clock =>
                {
                    return Err(anyhow::Error::from(error).context(
                        "If the stock server's log above shows its NTP reply held stock's \
                         clock, run again with --calibrate-clock to have it set, accepting that \
                         stock may run meals that came due while its clock jumps",
                    ));
                }
                Step::Failed(error) => return Err(error.into()),
            }
        }
    }

    /// `serve` with the trial enabled and, for an installer's config, the
    /// enrollment window; started once, before stock is pointed at it.
    async fn serve(&mut self) -> Result<()> {
        if self.serve.is_some() {
            return Ok(());
        }
        let mut command = Command::new(&self.stock_bin);
        command
            .arg("serve")
            .arg("--config")
            .arg(&self.config_path)
            .arg("--enable-ota-trial")
            .arg("--enroll-minutes")
            .arg(self.args.enroll_minutes.to_string())
            .stdout(onto_stderr()?)
            .kill_on_drop(true);
        if self.args.calibrate_clock {
            command.arg("--calibrate-clock");
        }
        self.serve = Some(command.spawn().context("start wellfed-stock serve")?);
        eprintln!("Started the stock server (its broker trusts the first feeder that connects).");
        Ok(())
    }

    /// Stop the image server, then `serve`. Each gets SIGTERM and a bounded
    /// wait first: `serve` stops its broker child on SIGTERM (a SIGKILL
    /// would orphan it on the MQTT port, its enrollment window maybe still
    /// open), and `ota-images` removes its lease.
    async fn stop(&mut self) {
        for child in [self.images.take(), self.serve.take()]
            .into_iter()
            .flatten()
        {
            stop_child(child).await;
        }
    }

    async fn send_frame(&mut self, frame: &[u8]) -> Result<()> {
        // Static analysis: Reset held about 5 s on an unlocked stock panel
        // starts its factory restore, which is how the vendor app's "add
        // device" begins; that it leaves stock in setup mode is still to be
        // seen on hardware.
        self.asker.done(Prompt::SetupMode).await?;
        let socket = UdpSocket::bind("0.0.0.0:0").context("open a UDP socket")?;
        socket.set_read_timeout(Some(Duration::from_secs(5)))?;
        socket
            .send_to(frame, SETUP_ADDRESS)
            .with_context(|| format!("send the setup frame to {SETUP_ADDRESS}"))?;
        let mut answer = [0; 256];
        match socket.recv_from(&mut answer) {
            Ok((n, _)) => eprintln!(
                "Sent the setup frame once; the feeder answered {}",
                wellfed_formats::hex::encode(&answer[..n])
            ),
            Err(_) => eprintln!("Sent the setup frame once; no answer within 5 s (not resent)"),
        }
        Ok(self.asker.done(Prompt::JoinBack).await?)
    }

    fn stock_status(&self) -> Result<StockStatus, ApiError> {
        self.api.get("/api/status", Auth::Token)
    }

    /// Until stock reports ready, or the window and its grace run out.
    async fn await_stock(&mut self, install: &mut Install) -> Result<()> {
        let deadline =
            Instant::now() + Duration::from_secs(self.args.enroll_minutes * 60) + STOCK_GRACE;
        let mut said = String::new();
        loop {
            let reason = match self.stock_status() {
                Ok(status) => match install.stock(&view(&status)) {
                    Ok(()) => return Ok(()),
                    Err(reason) => reason.to_string(),
                },
                Err(error) => format!("the stock server is not answering yet ({error})"),
            };
            if reason != said {
                eprintln!("Waiting: {reason}");
                said = reason;
            }
            if Instant::now() >= deadline {
                install.gave_up();
                return Ok(());
            }
            tokio::time::sleep(POLL).await;
        }
    }

    /// Serve the image and ask once. What the server answered is read back
    /// from its ledger, since a refused call may still have recorded the
    /// request (a dropped broker session marks it uncertain).
    async fn request(&mut self) -> Result<Requested> {
        let lease = self.images().await?;
        let id = uuid::Uuid::new_v4();
        self.request = Some(id);
        self.peer = Some(IpAddr::V4(lease.peer));

        eprintln!(
            "Asking stock once to install {} (request {id})",
            short(&lease.build)
        );
        let body = TrialRequest {
            request_id: id.to_string(),
            server_token: lease.token.to_string(),
        };
        self.track.sent(Unsettled::Request(id));
        let answered = self.api.post::<IgnoredAny>("/api/ota/trial", &body);

        // Refused only on the server's word and a ledger read that shows no
        // record; anything unread is uncertain.
        let recorded = match self.stock_status() {
            Ok(status) => Ok(status.trial(&id.to_string()).map(|trial| trial.phase)),
            Err(error) => Err(error),
        };
        Ok(match (answered, recorded) {
            (Ok(_), _) | (_, Ok(Some(TrialPhase::Submitted))) => Requested::Sent,
            (Err(ApiError::Refused { .. }), Ok(None)) => Requested::Refused,
            _ => Requested::Uncertain,
        })
    }

    /// `ota-images` for the review, and its lease once it listens.
    async fn images(&mut self) -> Result<ImageLease> {
        let path = ImageLease::path(&self.config_path);
        let before = std::fs::read(&path).ok();
        let mut command = Command::new(&self.stock_bin);
        command
            .arg("ota-images")
            .arg("--config")
            .arg(&self.config_path)
            .arg("--directory")
            .arg(&self.args.review)
            .stdout(onto_stderr()?)
            .kill_on_drop(true);
        self.images = Some(command.spawn().context("start wellfed-stock ota-images")?);

        let deadline = Instant::now() + IMAGES_WAIT;
        loop {
            let now = std::fs::read(&path).ok();
            if now.is_some() && now != before {
                return wellfed_formats::load(&path).context("read the image lease");
            }
            if Instant::now() >= deadline {
                bail!("the image server did not start within {IMAGES_WAIT:?}");
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// Stock's reports on this run's request, until its session ends.
    async fn await_install(&mut self, install: &mut Install) -> Result<()> {
        let id = self.request.map(|id| id.to_string()).unwrap_or_default();
        let deadline = Instant::now() + INSTALL_WAIT;
        let mut shown = None;
        while Instant::now() < deadline {
            if let Ok(status) = self.stock_status() {
                let report = latest_report(&status, &id);
                if let Some(progress) = report.and_then(|report| report.progress) {
                    install.stock_progress(progress);
                }
                if install.progress() != shown {
                    shown = install.progress();
                    eprintln!("Stock: {}% downloaded", shown.unwrap_or(0));
                }
                match status.trial(&id).map(|trial| trial.phase) {
                    Some(TrialPhase::ReportedError) => {
                        install.stock_refused(report.and_then(|report| report.error_code));
                        return Ok(());
                    }
                    _ if !status.device_online && status.last_seen.is_some() => {
                        eprintln!("Stock went quiet: it restarts into the replacement.");
                        install.stock_gone();
                        return Ok(());
                    }
                    _ => {}
                }
            }
            tokio::time::sleep(POLL).await;
        }
        install.gave_up();
        Ok(())
    }

    /// The replacement's address: given, or where stock connected from.
    fn address(&self) -> Result<SocketAddr> {
        if let Some(address) = &self.args.address {
            return Ok(wellfed_connect::parse_address(address)?);
        }
        let peer = self.peer.or_else(|| {
            let lease: ImageLease =
                wellfed_formats::load(&ImageLease::path(&self.config_path)).ok()?;
            Some(IpAddr::V4(lease.peer))
        });
        match peer {
            Some(ip) => Ok(SocketAddr::new(ip, PORT)),
            None => bail!("pass --address: no image lease names where the feeder connected from"),
        }
    }

    /// Become the replacement's owner: a key it already knows answers at
    /// once; otherwise the owner opens the recovery window and keys in the
    /// code. The replacement may still be starting, so a session is tried
    /// until it answers.
    async fn pair(&mut self, install: &mut Install) -> Result<()> {
        let address = self.address()?;
        if let Ok(identity) = self.key.feeder(self.args.feeder.as_deref()) {
            let deadline = Instant::now() + REPLACEMENT_WAIT;
            loop {
                match wellfed_connect::Connection::open(
                    self.key.keypair(),
                    identity,
                    address,
                    crate::device::random()?,
                )
                .await
                {
                    Ok(_) => {
                        eprintln!("The replacement knows this computer's key already.");
                        self.identity = Some(identity);
                        install.paired();
                        return Ok(());
                    }
                    Err(wellfed_connect::Error::UnknownKey) => break,
                    Err(_) if Instant::now() < deadline => tokio::time::sleep(POLL).await,
                    // Another identity answers there (a store reset since),
                    // or none: the recovery window settles which.
                    Err(error) => {
                        eprintln!("No session with the known feeder key ({error}).");
                        break;
                    }
                }
            }
        }

        // The window is opened on a running replacement: wait for its API.
        let deadline = Instant::now() + REPLACEMENT_WAIT;
        while tokio::net::TcpStream::connect(address).await.is_err() {
            if Instant::now() >= deadline {
                install.gave_up();
                return Ok(());
            }
            tokio::time::sleep(POLL).await;
        }
        self.asker.done(Prompt::OpenRecoveryWindow).await?;
        let (identity, text) = crate::pairing::recover(&mut self.key, address).await?;
        eprintln!("Paired with feeder {} ({text}).", fingerprint(&identity));
        self.identity = Some(identity);
        install.paired();
        Ok(())
    }

    /// Keep the `--stock-backup` copy as a backup read from the feeder is
    /// kept, before the promotion overwrites the feeder's own.
    fn keep_given(&mut self) -> Result<()> {
        let Some(backup) = self.given.take() else {
            return Ok(());
        };
        let identity = self
            .identity
            .context("the feeder's identity is not known yet")?;
        let path = crate::backup::path(&self.args.backups, &identity);
        crate::backup::keep(&path, &backup).context(
            "Promotion refused: the --stock-backup copy could not be kept; the feeder \
             runs the replacement from bank 0 and still holds stock in bank 1",
        )?;
        eprintln!("Kept the --stock-backup copy at {}.", path.display());
        Ok(())
    }

    /// The stock backup for the paired feeder: kept from an earlier run, or
    /// read from bank 1 now and kept.
    async fn backup(&self) -> Result<StockBackup> {
        let identity = self
            .identity
            .context("the feeder's identity is not known yet")?;
        let mut connection = crate::device::Connection::from(self.connect().await?);
        let (_, backup) =
            crate::backup::fetch(&mut connection, &self.args.backups, &identity).await?;
        Ok(backup)
    }

    /// Stock's plan sectors from the bank-0 role: kept beside the backup,
    /// then decoded and converted, and the import shown to the owner.
    async fn stock_plans(&self, install: &mut Install) -> Result<()> {
        let identity = self
            .identity
            .context("the feeder's identity is not known yet")?;
        let mut connection = self.connect().await?;
        // A feeder that predates the backup would end the session on it.
        if !connection.capabilities().backup {
            bail!("this build serves no stock backup or plans; update bank 0 first");
        }

        let answer = connection
            .ask(crate::device::random()?, &Op::ReadStockPlans)
            .await?;
        let sectors = match answer {
            Answer::StockPlans(sectors) => sectors,
            Answer::Refused(refusal) => {
                install.stock_plans_unread(refusal);
                return Ok(());
            }
            other => bail!("the feeder answered {other:?} to a stock plans request"),
        };
        let path = crate::backup::plans_path(&self.args.backups, &identity);
        crate::backup::keep_plans(&path, &plans::kept(sectors.count, &sectors.entries)).context(
            "Promotion refused: stock's plans could not be kept; the feeder runs the \
             replacement from bank 0 and still holds stock in bank 1",
        )?;
        eprintln!("Kept stock's plan sectors at {}.", path.display());

        install.stock_plans(sectors.count, &sectors.entries, now_ms()?);
        if let Some(import) = install.import() {
            eprint!(
                "Stock's plans, as the replacement will run them:\n{}",
                plans::report(import)
            );
        }
        if let Some(skipped) = install.plans_skipped() {
            eprintln!("Going on without stock's plans (--without-stock-plans): {skipped}.");
        }
        Ok(())
    }

    /// A run that reaches the confirmation without reading stock's
    /// sectors (resumed after the promotion) takes the copy an earlier
    /// run kept beside the backup, if there is one.
    fn kept_plans(&self, install: &mut Install) -> Result<()> {
        if install.import().is_some() || install.plans_skipped().is_some() {
            return Ok(());
        }
        let Some(identity) = self.identity else {
            return Ok(());
        };
        let path = crate::backup::plans_path(&self.args.backups, &identity);
        let kept = match std::fs::read(&path) {
            Ok(kept) => kept,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
        };
        install.kept_stock_plans(&kept, now_ms()?);
        if let Some(import) = install.import() {
            eprint!(
                "Stock's plans, from the copy kept at {}:\n{}",
                path.display(),
                plans::report(import)
            );
        }
        Ok(())
    }

    /// Set the kept copy aside once its plans are on the replacement, as
    /// `stock-plans.applied.bin` (or a name with the time if one is there
    /// already), so a later resumed run does not put them back over plans
    /// the owner has changed since.
    fn kept_plans_applied(&self) -> Result<()> {
        let Some(identity) = self.identity else {
            return Ok(());
        };
        let kept = crate::backup::plans_path(&self.args.backups, &identity);
        if let Some(applied) = set_aside(&kept, now_ms()?)? {
            eprintln!("Set the applied copy aside at {}.", applied.display());
        }
        Ok(())
    }

    /// The replacement's plans for the steps. A read changes nothing, so a
    /// session that fails is opened again until [`PLANS_WAIT`] runs out.
    async fn read_plans(&self, install: &mut Install) -> Result<()> {
        let deadline = Instant::now() + PLANS_WAIT;
        loop {
            match self.plans().await {
                Ok(view) => {
                    install.plans(&view);
                    return Ok(());
                }
                Err(error) if Instant::now() >= deadline => {
                    eprintln!("The replacement's plans could not be read ({error:#}).");
                    install.gave_up();
                    return Ok(());
                }
                Err(_) => tokio::time::sleep(POLL).await,
            }
        }
    }

    async fn plans(&self) -> Result<Plans> {
        let mut connection = self.connect().await?;
        match connection
            .ask(crate::device::random()?, &Op::GetPlans)
            .await?
        {
            Answer::Plans(view) => Ok(view),
            other => bail!("the feeder answered {other:?} to a plans request"),
        }
    }

    /// Put the import once. Anything but an answer is unanswered, which
    /// the steps judge by reading the plans back; it is never sent again.
    async fn put_plans(
        &mut self,
        base: Generation,
        plans: [Option<Plan>; PLANS],
    ) -> Result<PlansPut> {
        eprintln!("Setting stock's converted plans on the replacement.");
        // Opening a session sends nothing, so it may be tried again; the
        // put itself goes once.
        let deadline = Instant::now() + PLANS_WAIT;
        let mut connection = loop {
            match self.connect().await {
                Ok(connection) => break connection,
                Err(error) if Instant::now() >= deadline => {
                    return Err(error
                        .context("stock's plans were not sent: no session with the replacement"));
                }
                Err(_) => tokio::time::sleep(POLL).await,
            }
        };
        let op = Op::PutPlans { base, plans };
        let id = crate::device::random()?;
        self.track.sent(Unsettled::Plans);
        Ok(match connection.ask(id, &op).await {
            Ok(Answer::Done { .. }) => PlansPut::Done,
            Ok(Answer::Refused(refusal)) => PlansPut::Refused(refusal),
            Ok(_) | Err(_) => PlansPut::Unanswered,
        })
    }

    async fn connect(&self) -> Result<wellfed_connect::Connection> {
        let identity = self
            .identity
            .context("the feeder's identity is not known yet")?;
        Ok(wellfed_connect::Connection::open(
            self.key.keypair(),
            identity,
            self.address()?,
            crate::device::random()?,
        )
        .await?)
    }

    /// The build and bank the replacement reports.
    async fn status(&self) -> Result<(String, u8)> {
        let mut connection = self.connect().await?;
        match connection
            .ask(crate::device::random()?, &Op::Status)
            .await?
        {
            Answer::Status(status) => Ok((status.build, status.bank)),
            other => bail!("the feeder answered {other:?} to a status request"),
        }
    }

    /// Tell the stock server the served build answered from bank 0. It
    /// refuses while stock's telemetry is still current, so it is asked
    /// again for a while; a record left open only makes a later run wait.
    async fn close_trial(&self, build: &str) {
        let Some(id) = self.request else {
            return;
        };
        let answer = TrialAnswer {
            request_id: id.to_string(),
            build: build.to_string(),
            bank: 0,
            note: format!(
                "wellfed install: build {} answered from bank 0",
                short(build)
            ),
        };
        let deadline = Instant::now() + CLOSE_WAIT;
        loop {
            match self
                .api
                .post::<IgnoredAny>("/api/ota/trial/answered", &answer)
            {
                Ok(_) => {
                    eprintln!("The stock server closed its record: replacement healthy.");
                    return;
                }
                Err(error) if Instant::now() >= deadline => {
                    eprintln!("The stock server kept its record open ({error}).");
                    return;
                }
                Err(_) => tokio::time::sleep(POLL).await,
            }
        }
    }

    /// Push the signed update from bank 0 until bank 1 is activated, or the
    /// update's own error ([`push`]). If bank 1 answers a reopened session
    /// with the served build, the activation's answer was lost and the
    /// feeder already restarted into its trial boot: offering the manifest
    /// there would interrupt the trial, so the promotion counts as
    /// activated instead.
    async fn promote(&mut self, served: &str) -> Result<Result<(), UpdateError>> {
        let payload = self
            .container
            .get(CONTAINER_HEADER..)
            .context("image.bin is shorter than its header")?;
        let image = Image {
            manifest: &self.manifest,
            downgrade: None,
            payload,
        };

        eprintln!("Promoting: bank 0 writes the replacement into bank 1.");
        self.track.sent(Unsettled::Promotion);
        let pushed = push(
            image,
            async || self.connect().await,
            |bank, build| activated(bank, build, served),
        )
        .await?;
        Ok(match pushed {
            Pushed::Activated | Pushed::AlreadyRunning => Ok(()),
            Pushed::Failed(error) => Err(error),
            Pushed::Gone { .. } => Err(UpdateError::Lost),
        })
    }

    /// Until bank 1 runs the build and reports its version confirmed.
    async fn await_confirm(&self, install: &mut Install) -> Result<()> {
        tokio::time::sleep(Duration::from_secs(20)).await;
        let deadline = Instant::now() + CONFIRM_WAIT;
        while Instant::now() < deadline {
            if let Ok(mut connection) = self.connect().await
                && let Ok(Answer::Status(status)) =
                    connection.ask(crate::device::random()?, &Op::Status).await
                && let Ok(Answer::Update(view)) = connection
                    .ask(crate::device::random()?, &Op::UpdateState)
                    .await
            {
                install.confirmed(
                    Replacement {
                        build: &status.build,
                        bank: status.bank,
                    },
                    view.min_version,
                );
                if !matches!(install.step(), Step::AwaitConfirm) {
                    return Ok(());
                }
            }
            tokio::time::sleep(POLL).await;
        }
        install.gave_up();
        Ok(())
    }
}

/// Stock's latest OTA report on request `id`: one naming it, or naming
/// none, received since the request was recorded. The server keeps reports
/// from earlier requests too.
fn latest_report<'a>(status: &'a StockStatus, id: &str) -> Option<&'a OtaReport> {
    let since = status.trial(id)?.created_at;
    status.ota_reports.iter().rev().find(|report| {
        report.received_at >= since && report.message_id.as_deref().is_none_or(|m| m == id)
    })
}

/// The steps' view of stock, from the server's status document.
fn view(status: &StockStatus) -> StockView {
    let attribute = |name: &str| status.attributes.get(name);
    StockView {
        online: status.device_online,
        version: status
            .version_reported_at
            .and(attribute("softwareVersion"))
            .and_then(|value| value.as_str())
            .map(str::to_string),
        power: attribute("powerType")
            .and_then(serde_json::Value::as_u64)
            .map(StockPower::from_type),
        trust: match &status.enrollment {
            None => FeederTrust::Captured,
            Some(enrollment) if enrollment.contested => FeederTrust::Contested,
            Some(enrollment) => FeederTrust::Enrolled {
                device: enrollment.device.clone(),
                address: enrollment.address.to_string(),
            },
        },
    }
}

/// Rename the kept plans at `kept` to `stock-plans.applied.bin` beside it,
/// or to a name with `now_ms` when an earlier applied copy holds that name:
/// nothing kept is overwritten. `None` when nothing is kept there.
fn set_aside(kept: &Path, now_ms: u64) -> Result<Option<PathBuf>> {
    if !kept.exists() {
        return Ok(None);
    }
    let mut applied = kept.with_file_name("stock-plans.applied.bin");
    if applied.exists() {
        applied = kept.with_file_name(format!("stock-plans.applied-{now_ms}.bin"));
    }
    std::fs::rename(kept, &applied).with_context(|| {
        format!(
            "set {} aside as applied; delete it so a resumed run does not apply it again",
            kept.display()
        )
    })?;
    Ok(Some(applied))
}

/// Unix milliseconds now, which decides whether a stock skip still holds.
fn now_ms() -> Result<u64> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("the clock is before 1970")?;
    u64::try_from(now.as_millis()).context("the clock is past 2554")
}

fn short(build: &str) -> &str {
    &build[..build.len().min(12)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wellfed_client::update::{Installer, Next};

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
            "--broker-name",
            "broker.example.net",
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
    }

    /// Stopped while stock downloads: the request is out and unsettled,
    /// through pairing, until the replacement answers its status.
    #[test]
    fn a_request_stays_unsettled_until_the_replacement_answers() {
        let id = uuid::Uuid::from_u128(7);
        let mut track = Track::new();
        track.enter(Phase::Request);
        track.sent(Unsettled::Request(id));
        for phase in [Phase::AwaitInstall, Phase::Pair, Phase::ReadStatus] {
            track.enter(phase);
            assert_eq!(track.unsettled, Some(Unsettled::Request(id)), "{phase:?}");
        }
        track.enter(Phase::AwaitInstall);
        let stopped = track.stopped(Signal::Terminate).to_string();
        assert!(stopped.starts_with("stopped by SIGTERM during stock's install"));
        assert!(stopped.contains(&format!("stock's install request {id} sent")));
        assert!(stopped.contains("not sent again"));

        track.enter(Phase::CloseTrial);
        assert_eq!(track.unsettled, None);
        let stopped = track.stopped(Signal::Interrupt).to_string();
        assert!(
            stopped.ends_with("no write was waiting on its outcome"),
            "{stopped}"
        );
    }

    /// Stopped after the push came back activated but before bank 1
    /// confirmed: the promotion is still unsettled.
    #[test]
    fn a_promotion_stays_unsettled_until_bank_1_confirms() {
        let mut track = Track::new();
        track.enter(Phase::Promote);
        track.sent(Unsettled::Promotion);
        track.enter(Phase::AwaitConfirm);
        let stopped = track.stopped(Signal::Terminate);
        assert_eq!(stopped.unsettled, Some(Unsettled::Promotion));
        assert!(
            stopped
                .to_string()
                .contains("waiting for bank 1 to confirm the promotion, with the promotion's push")
        );

        track.enter(Phase::ApplyPlans);
        assert_eq!(track.unsettled, None);
        track.enter(Phase::ApplyPlans);
        track.sent(Unsettled::Plans);
        // A put that went unanswered is judged by reading the plans back.
        track.enter(Phase::ApplyPlans);
        assert_eq!(track.unsettled, Some(Unsettled::Plans));
        track.enter(Phase::Done);
        assert_eq!(track.unsettled, None);
    }

    /// Pairing sends no write that matters twice: the report says so.
    #[test]
    fn a_stop_while_pairing_says_it_is_safe_to_run_again() {
        let mut track = Track::new();
        track.enter(Phase::Pair);
        let stopped = track.stopped(Signal::Interrupt).to_string();
        assert!(stopped.contains("safe to run again"), "{stopped}");
        assert!(stopped.ends_with("no write was waiting on its outcome"));

        // Paired before the replacement's status settled the request: the
        // enrollment may be repeated, the request may not.
        track.sent(Unsettled::Request(uuid::Uuid::from_u128(7)));
        let stopped = track.stopped(Signal::Interrupt).to_string();
        assert!(!stopped.contains("safe to run again"), "{stopped}");
        assert!(
            stopped.contains("The enrollment is safe to repeat; that write is not."),
            "{stopped}"
        );
    }

    #[test]
    fn the_server_s_document_becomes_the_steps_view() {
        let mut status = StockStatus {
            device_online: true,
            ..StockStatus::default()
        };
        status
            .attributes
            .insert("softwareVersion".into(), json!("1.1.15"));
        status.attributes.insert("powerType".into(), json!(1));

        // A version the server has not seen reported is no version.
        let stale = view(&status);
        assert_eq!(stale.version, None);
        assert_eq!(stale.power, Some(StockPower::Adapter));

        status.version_reported_at = Some(chrono::DateTime::UNIX_EPOCH);
        let fresh = view(&status);
        assert_eq!(fresh.version.as_deref(), Some("1.1.15"));
        assert!(fresh.online);
        assert_eq!(fresh.trust, FeederTrust::Captured);

        let mut enrollment = wellfed_formats::stock::StockEnrollment {
            device: "AF_NEW".into(),
            address: "192.168.4.20".parse().unwrap(),
            contested: false,
        };
        status.enrollment = Some(enrollment.clone());
        assert_eq!(
            view(&status).trust,
            FeederTrust::Enrolled {
                device: "AF_NEW".into(),
                address: "192.168.4.20".into()
            }
        );
        enrollment.contested = true;
        status.enrollment = Some(enrollment);
        assert_eq!(view(&status).trust, FeederTrust::Contested);
    }

    /// A session lost after the last chunk, reopened once the feeder
    /// restarted into its trial boot, is the activation: the manifest is not
    /// offered to bank 1 (that would read as a failure, or give way and
    /// abandon the trial).
    #[test]
    fn bank_1_running_the_served_build_on_reopen_is_the_activation() {
        let served = "ab".repeat(32);
        assert!(activated(1, &served.to_ascii_uppercase(), &served));
        assert!(
            !activated(0, &served, &served),
            "bank 0 resumes the install"
        );
        assert!(!activated(1, &"cd".repeat(32), &served));

        // What the installer would do instead: from Receiving, a session on
        // bank 1 goes back to offering, which is what this avoids.
        let manifest = [0u8; 4];
        let payload = [0u8; 4];
        let mut installer = Installer::new(Image {
            manifest: &manifest,
            downgrade: None,
            payload: &payload,
        });
        installer.opened(1);
        assert!(matches!(
            installer.poll(),
            Next::Send(wellfed_client::update::Sent::Offer, _)
        ));
    }

    #[test]
    fn progress_is_read_only_from_this_request_s_reports() {
        use wellfed_formats::stock::{OtaCommand, TrialInput, TrialRecord};

        let id = uuid::Uuid::from_u128(7);
        let at = |seconds| chrono::DateTime::from_timestamp(seconds, 0).unwrap();
        let report = |seconds, message: Option<&str>, progress| OtaReport {
            received_at: at(seconds),
            command: OtaCommand::Progress,
            message_id: message.map(str::to_string),
            phase: None,
            state: None,
            progress: Some(progress),
            code: None,
            error_code: None,
        };
        let status = StockStatus {
            ota_trials: vec![TrialRecord {
                sequence: 1,
                input: TrialInput {
                    request_id: id,
                    server_token: id,
                },
                created_at: at(100),
                updated_at: at(100),
                phase: TrialPhase::Submitted,
                download_reported: false,
                resolution_note: None,
                image_sha256: String::new(),
                build: None,
                stock_reported: false,
            }],
            ota_reports: vec![
                report(150, Some(&id.to_string()), 40),
                // An earlier request's, and one from before this request.
                report(160, Some("other"), 90),
                report(50, None, 80),
            ],
            ..StockStatus::default()
        };
        let latest = latest_report(&status, &id.to_string()).unwrap();
        assert_eq!(latest.progress, Some(40));
        assert!(latest_report(&status, "unknown").is_none());
    }

    /// An applied copy is set aside, never over an earlier one, so a
    /// resumed run finds no kept copy to apply again.
    #[test]
    fn an_applied_copy_is_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let kept = crate::backup::plans_path(dir.path(), &[0xab; 32]);
        assert_eq!(set_aside(&kept, 7).unwrap(), None);

        crate::backup::keep_plans(&kept, &[1]).unwrap();
        let first = set_aside(&kept, 7).unwrap().unwrap();
        assert!(first.ends_with("stock-plans.applied.bin"));
        assert!(!kept.exists());

        crate::backup::keep_plans(&kept, &[2]).unwrap();
        let second = set_aside(&kept, 7).unwrap().unwrap();
        assert!(second.ends_with("stock-plans.applied-7.bin"));
        assert_eq!(std::fs::read(first).unwrap(), [1]);
        assert_eq!(std::fs::read(second).unwrap(), [2]);
    }

    #[test]
    fn a_review_without_a_build_identity_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("build-manifest.json"), "{}").unwrap();
        assert!(review_build(dir.path()).is_err());
    }
}
