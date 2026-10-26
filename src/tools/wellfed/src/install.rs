//! `wellfed install`: one run from a feeder on stock 1.1.15 to the
//! replacement confirmed on bank 1 ([consumer installation]).
//!
//! The steps and every decision between them are `wellfed_client::install`'s,
//! the same the Android app runs; this module is their IO for a computer:
//! - stock's side is `wellfed-stock`, run as child processes (`serve`, which
//!   trusts the first feeder that connects during the run, and
//!   `ota-images`), spoken to over its loopback API;
//! - the setup frame is one UDP datagram, sent after the owner has joined
//!   this computer to the feeder's setup access point (joining Wi-Fi is the
//!   platform's job, so the run waits for the owner to say it is done);
//! - the replacement is reached over the device API, with this computer's
//!   key, enrolled through the recovery window when the build does not know
//!   it yet.
//!
//! The run holds the bench lease beside the stock config for its whole
//! length; `serve` runs on it as a descendant. Nothing is retried that the
//! steps say must not be: the install request goes once, and a refusal or
//! an uncertain outcome ends the run.
//!
//! [consumer installation]: ../../../../docs/firmware/consumer-installation.md

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Args;
use wellfed_client::install::{
    FeederTrust, Install, Provision, Replacement, Requested, SETUP_ADDRESS, SetupFrame, Step,
    StockPower, StockView, VerifiedBackup, manifest_version,
};
use wellfed_client::update::{Image, InstallError as UpdateError, Installer, Next};
use wellfed_formats::bench::lease::Lease;
use wellfed_formats::manifest::BuildManifest;
use wellfed_formats::stock::{
    ImageLease, OtaReport, ServerConfig, StockStatus, TrialAnswer, TrialPhase, TrialRequest,
};
use wellfed_model::{Answer, Op};
use wellfed_proto::api::PORT;
use wellfed_stock_api::{ApiError, Auth, StockApi};
use serde::de::IgnoredAny;
use tokio::process::{Child, Command};

use crate::keys::{OwnerKey, fingerprint};

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
/// Sessions the promotion reopens at most: a feeder that keeps dropping
/// them is not coming back.
const REOPEN_MAX: u32 = 20;
/// The OTA container's header precedes the bank contents.
const CONTAINER_HEADER: usize = 32;

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
    /// Promotion overwrites the feeder's only copy of stock, so without one
    /// the run stops before it, with the replacement on bank 0 and stock
    /// intact in bank 1.
    #[arg(long)]
    stock_backup: Option<PathBuf>,
    /// Point stock at this network (passphrase from standard input) and the
    /// installer's broker with the setup frame first. Without it, stock
    /// already connects to the installer's broker.
    #[arg(long)]
    ssid: Option<String>,
    /// The numeric account ID the frame carries; stock requires nonzero
    /// digits and keeps it beside the broker.
    #[arg(long, default_value = "1")]
    account: String,
    /// The broker the frame names; the stock config's MQTT address otherwise.
    #[arg(long)]
    broker: Option<SocketAddr>,
    /// The replacement is already on bank 0 from an earlier run: start at
    /// pairing, and ask stock nothing.
    #[arg(long, conflicts_with = "ssid")]
    from_bank0: bool,
    /// `wellfed-stock`'s config: an installer's (`init-installer`) or the
    /// bench's.
    #[arg(long, default_value = "private/server.json")]
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
}

pub(crate) async fn run(args: InstallArgs) -> Result<()> {
    let config_path = std::path::absolute(&args.stock_config).context("--stock-config")?;
    let config: ServerConfig = wellfed_formats::load(&config_path)
        .with_context(|| format!("read {}", config_path.display()))?;
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
            Some(VerifiedBackup::verify(&image).context("--stock-backup")?)
        }
        None => {
            eprintln!(
                "No --stock-backup: the run stops before promotion, which would overwrite \
                 the feeder's only copy of stock."
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
    if let Some(backup) = backup {
        install.backup(backup);
    }

    let mut run = Run {
        stock_bin: stock_bin(args.stock_server.as_deref())?,
        api: StockApi::new(&config, API_TIMEOUT),
        key: OwnerKey::open(&args.keys, None, OwnerKey::system())?,
        args,
        config_path,
        container,
        manifest,
        serve: None,
        images: None,
        request: None,
        peer: None,
        identity: None,
    };
    let result = run.drive(&mut install).await;
    run.stop().await;
    result
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

/// The setup frame, when `--ssid` asks for one: the owner's network and
/// the installer's broker.
fn provision(args: &InstallArgs, config: &ServerConfig) -> Result<Provision> {
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

/// What reopening a promotion's session found.
enum Reopened {
    Session(Box<wellfed_connect::Connection>),
    /// Bank 1 already runs the served build.
    Activated,
    /// The feeder did not come back in time.
    Gone,
}

/// Whether a session answering from `bank` with `build` shows the
/// promotion already activated. During a promotion that started on bank 0,
/// bank 1 can only run the served build once bank 0 activated it.
fn activated(bank: u8, build: &str, served: &str) -> bool {
    bank == 1 && build.eq_ignore_ascii_case(served)
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
fn recognized(trust: &FeederTrust) -> Result<bool> {
    let (device, address) = match trust {
        FeederTrust::Enrolled { device, address } => (device, address),
        // The bench: its credentials came from a capture of this feeder.
        FeederTrust::Captured => return Ok(true),
        FeederTrust::Contested => return Ok(false),
    };
    eprintln!(
        "The broker trusted feeder {device}, connecting from {address}. Is this your feeder? [y/N]"
    );
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("read standard input")?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes"))
}

/// Wait for the owner to press Enter after `prompt`.
fn confirm(prompt: &str) -> Result<()> {
    eprintln!("{prompt} Then press Enter.");
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("read standard input")?;
    Ok(())
}

impl Run {
    async fn drive(&mut self, install: &mut Install) -> Result<()> {
        loop {
            match install.step() {
                Step::SendFrame(frame) => {
                    self.serve().await?;
                    self.send_frame(frame.bytes())?;
                    install.frame_sent();
                }
                Step::AwaitStock => {
                    self.serve().await?;
                    self.await_stock(install).await?;
                }
                Step::ConfirmFeeder(trust) => {
                    if recognized(&trust)? {
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
                Step::Backup => bail!(
                    "Promotion refused: no verified stock backup. The feeder runs the \
                     replacement from bank 0 and still holds stock in bank 1. Run again with \
                     --from-bank0 --stock-backup <stock 1.1.15 OTA image>."
                ),
                Step::Promote => {
                    let outcome = self.promote(install.build()).await?;
                    install.promoted(outcome);
                }
                Step::AwaitConfirm => self.await_confirm(install).await?,
                Step::Done => {
                    eprintln!(
                        "Installed: build {} confirmed on bank 1. This computer is its owner.",
                        short(install.build())
                    );
                    return Ok(());
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
            .kill_on_drop(true);
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

    fn send_frame(&self, frame: &[u8]) -> Result<()> {
        // Static analysis: Reset held about 5 s on an unlocked stock panel
        // starts its factory restore, which is how the vendor app's "add
        // device" begins; that it leaves stock in setup mode is still to be
        // seen on hardware.
        confirm(
            "Put stock into setup mode: unlock its panel (hold Lock about 5 s if the lock \
             light is on), then hold Reset (the rightmost button) about 5 s. Join this \
             computer to the access point the feeder then opens.",
        )?;
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
        confirm("Join this computer back to your own network.")
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
        confirm(
            "Pair with the replacement: on its panel (unlocked), hold Lock, then Dispense, \
             to open the recovery window.",
        )?;
        let (identity, text) = crate::recover(&mut self.key, address).await?;
        eprintln!("Paired with feeder {} ({text}).", fingerprint(&identity));
        self.identity = Some(identity);
        install.paired();
        Ok(())
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
    /// update's own error. A dropped session loses nothing: the installer
    /// resumes on the next one (chunks are idempotent by offset, and bank 0
    /// waits a minute for the next), so a lost answer or a restart reopens
    /// the session rather than ending the promotion.
    async fn promote(&self, served: &str) -> Result<Result<(), UpdateError>> {
        let payload = self
            .container
            .get(CONTAINER_HEADER..)
            .context("image.bin is shorter than its header")?;
        let mut installer = Installer::new(Image {
            manifest: &self.manifest,
            downgrade: None,
            payload,
        });

        eprintln!("Promoting: bank 0 writes the replacement into bank 1.");
        let mut connection = None;
        let mut reopened = 0;
        let mut written = 0;
        loop {
            let session = match &mut connection {
                Some(session) => session,
                None if reopened >= REOPEN_MAX => return Ok(Err(UpdateError::Lost)),
                None => {
                    reopened += 1;
                    match self.reopen(&mut installer, served).await? {
                        Reopened::Session(session) => connection.insert(*session),
                        Reopened::Activated => {
                            eprintln!("Bank 1 already runs the build: it was activated.");
                            return Ok(Ok(()));
                        }
                        Reopened::Gone => return Ok(Err(UpdateError::Lost)),
                    }
                }
            };

            match installer.poll() {
                Next::Send(sent, op) => {
                    match session.ask(crate::device::random()?, &op).await {
                        // An answer the installer rejects ends the install;
                        // the next poll says so.
                        Ok(answer) => {
                            let _ = installer.answer(sent, &answer);
                        }
                        Err(error) => {
                            eprintln!("  The session dropped ({error}); resuming on a new one.");
                            installer.lost();
                            connection = None;
                        }
                    }
                    if let Some(view) = installer.view()
                        && view.written / 65_536 != written / 65_536
                    {
                        written = view.written;
                        eprintln!("  {} of {} bytes written", view.written, view.image_len);
                    }
                }
                Next::Wait => tokio::time::sleep(Duration::from_millis(200)).await,
                Next::Activated => {
                    eprintln!("Activated: the feeder restarts into bank 1.");
                    return Ok(Ok(()));
                }
                Next::Failed(error) => return Ok(Err(error)),
                // Bank 1 gave way (a feeder that answered from bank 1):
                // reconnect once it has restarted into bank 0.
                Next::Reconnect => {
                    installer.lost();
                    connection = None;
                    tokio::time::sleep(POLL).await;
                }
                Next::Cancelled => return Ok(Err(UpdateError::Lost)),
            }
        }
    }

    /// A new session for the promotion, with the installer told which bank
    /// answers. If bank 1 answers with the served build, the activation's
    /// answer was lost and the feeder already restarted into its trial boot:
    /// offering the manifest there would interrupt the trial, so the
    /// promotion counts as activated instead.
    async fn reopen(&self, installer: &mut Installer<'_>, served: &str) -> Result<Reopened> {
        let deadline = Instant::now() + REPLACEMENT_WAIT;
        loop {
            if let Ok(mut session) = self.connect().await
                && let Ok(Answer::Status(status)) =
                    session.ask(crate::device::random()?, &Op::Status).await
            {
                if activated(status.bank, &status.build, served) {
                    return Ok(Reopened::Activated);
                }
                installer.opened(status.bank);
                return Ok(Reopened::Session(Box::new(session)));
            }
            if Instant::now() >= deadline {
                return Ok(Reopened::Gone);
            }
            tokio::time::sleep(POLL).await;
        }
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

fn short(build: &str) -> &str {
    &build[..build.len().min(12)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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

    #[test]
    fn a_review_without_a_build_identity_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("build-manifest.json"), "{}").unwrap();
        assert!(review_build(dir.path()).is_err());
    }
}
