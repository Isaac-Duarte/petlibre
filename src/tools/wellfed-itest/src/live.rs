//! The live [`Rig`]: the bench's real hotspot, `wellfed-stock` children,
//! UART captures, device API sessions as the owner's key (requests, and the
//! updates it pushes on a thread of its own) and, with `--power`, the
//! feeder supply relay.
//!
//! The owner's standing authorization (AGENTS.md) covers bringing the
//! hotspot up and running `serve` as a bounded foreground process, provided
//! the prior network and service state comes back afterwards. So the rig
//! records the hotspot's and the supply's state before touching either,
//! refuses to start beside a `serve` it did not start, and restores
//! everything in [`LiveRig::restore`], which `Drop` also runs.
//!
//! The UART is held by back-to-back receive-only `wellfed-flash read
//! observe` segments of [`SEGMENT`]. The capture writer flushes each
//! received chunk, so the growing `serial-rx.bin` is read while the segment
//! runs. The chain runs on its own thread ([`Capture`]), so the port stays
//! held while the suite blocks on a build.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::net::{SocketAddr, TcpStream};

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde::de::IgnoredAny;

use crate::args::ItestArgs;
use wellfed_formats::bench::live::ActivityKind;
use wellfed_formats::bench::progress::StepState;
use wellfed_formats::bench::{CAPTURES, VERIFICATION};
use wellfed_formats::require;
use wellfed_formats::stock::{ImageLease, ServerConfig};
use wellfed_relay::{POWER, STRAP, Switch};
use wellfed_rig::commands::Bench;
use wellfed_rig::commands::ota::{self, OtaArgs, Staged, Target, Timing};
use wellfed_rig::device::{Answer, Ask, Device, Intruder, Rebuffed};
use wellfed_rig::server::{StockStatus, TrialAnswer, TrialRequest, TrialResolve};
use wellfed_rig::tools::{
    Capture, CaptureFailure, InstallJob, Installing, PushError, Tracked, own_group,
};
use wellfed_rig::{Waited, say, wait_child};
use wellfed_stock_api::{Auth, StockApi};

use super::{Armed, Error, Fault, Hotspot, Result, Rig, Server};

/// Why the live rig could not set the bench up or keep it running.
#[derive(Debug, thiserror::Error)]
pub enum LiveError {
    #[error("{} has no connection", hotspot.display())]
    NoConnection { hotspot: PathBuf },
    #[error("UART port {port} is missing")]
    UartMissing { port: String },
    #[error("{} not found; cargo build --release -p wellfed-stock", path.display())]
    StockMissing { path: PathBuf },
    /// The suite runs its own `serve` and never shares one.
    #[error(
        "Something already answers on {address}; stop that serve first (the suite runs its own)"
    )]
    ServeRunning { address: SocketAddr },
    #[error("nmcli: {0}")]
    Nmcli(std::io::Error),
    /// `stderr` as nmcli printed it, trimmed.
    #[error("nmcli could not list connections: {stderr}")]
    NmcliList { stderr: String },
    #[error("nmcli connection {verb} failed: {stderr}")]
    NmcliVerb { verb: &'static str, stderr: String },
    /// A server child exited before it was ready; `log` root-relative.
    #[error("{what} exited ({status}); see {log}")]
    ChildExited {
        what: &'static str,
        status: std::process::ExitStatus,
        log: String,
    },
    #[error("{what} did not become ready in {:.0} s", START_WAIT.as_secs_f64())]
    ChildTimeout { what: &'static str },
    #[error("A power cycle needs --power")]
    NoPower,
    #[error("The UART capture is not running")]
    NoCapture,
    #[error("The UART capture stopped: {0}")]
    CaptureStopped(Arc<CaptureFailure>),
    #[error("The image server's lease disappeared")]
    LeaseGone,
}

/// The observer's own limit for one capture.
const SEGMENT: Duration = Duration::from_secs(180);
const START_WAIT: Duration = Duration::from_secs(20);
const STOP_WAIT: Duration = Duration::from_secs(10);
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// `private/hotspot.json` (`--hotspot`).
#[derive(Deserialize)]
struct HotspotFile {
    /// The NetworkManager connection that serves the feeder's network.
    connection: String,
}

/// The fixed trial port stock's URL names (`wellfed_stock::ota::image::PORT`).
const IMAGE_PORT: u16 = 8088;

/// A `wellfed-stock` child, registered with the bench's children while it
/// runs, so a cancel reaches it.
type Running = (Child, Tracked);

#[derive(Default)]
struct Children {
    serve: Option<Running>,
    images: Option<Running>,
}

pub(super) struct LiveRig<'b> {
    bench: &'b mut Bench,
    name: String,
    started: Instant,
    config_arg: String,
    api: StockApi,
    device: Device,
    /// The owner key file and server config every push uses.
    keys: PathBuf,
    config: PathBuf,
    /// The update being pushed, if any.
    pushing: Option<Box<dyn Installing>>,
    /// The server's MQTT listener, whose host also serves stock's images.
    mqtt: SocketAddr,
    connection: String,
    /// The hotspot's state before the run; `None` until recorded.
    hotspot_was: Option<Hotspot>,
    hotspot_now: Hotspot,
    power: bool,
    power_was_on: Option<bool>,
    /// `On` while an unattended run holds the motor: teardown cuts relay 1.
    kill_armed: Armed,
    children: Children,
    stock_bin: PathBuf,
    capture: Option<Box<dyn Capture>>,
    /// The segment being read, by index, and how far into it.
    reading: (usize, u64),
    /// Root-relative capture directories, for the summary.
    pub(super) captures: Vec<String>,
    server_log: PathBuf,
    restored: bool,
}

impl<'b> LiveRig<'b> {
    /// Check everything that can refuse before the bench changes: configs,
    /// binaries, ports, unused evidence names and no `serve` running.
    pub(super) fn open(bench: &'b mut Bench, args: &ItestArgs, stock_bin: PathBuf) -> Result<Self> {
        let root = bench.root.clone();
        let name = args.name().to_string();
        let config: ServerConfig = wellfed_rig::load(&root, &args.config)?;
        let hotspot: HotspotFile = wellfed_rig::load(&root, &args.hotspot)?;
        require(!hotspot.connection.is_empty(), || LiveError::NoConnection {
            hotspot: args.hotspot.clone(),
        })?;

        let rig = Self {
            name: name.clone(),
            started: Instant::now(),
            config_arg: args.config.display().to_string(),
            api: StockApi::new(&config, HTTP_TIMEOUT),
            device: Device::open(&root, &args.keys, &args.config)?,
            keys: args.keys.clone(),
            config: args.config.clone(),
            pushing: None,
            mqtt: config.mqtt_listen,
            connection: hotspot.connection,
            hotspot_was: None,
            hotspot_now: Hotspot::Down,
            power: args.power,
            power_was_on: None,
            kill_armed: Armed::Off,
            children: Children::default(),
            capture: None,
            reading: (0, 0),
            captures: Vec::new(),
            server_log: root.join(VERIFICATION).join(format!("{name}-servers.log")),
            stock_bin,
            restored: false,
            bench,
        };

        require(Path::new(&rig.bench.config.uart_port).exists(), || {
            LiveError::UartMissing {
                port: rig.bench.config.uart_port.clone(),
            }
        })?;
        require(rig.stock_bin().is_file(), || LiveError::StockMissing {
            path: rig.stock_bin(),
        })?;
        rig.bench.fresh(&rig.server_log)?;
        rig.bench.fresh(&rig.segment_path(1))?;
        require(
            rig.api
                .get::<IgnoredAny>("/health", Auth::Anonymous)
                .is_err(),
            || LiveError::ServeRunning {
                address: rig.api.address,
            },
        )?;
        Ok(rig)
    }

    /// Bring the bench to its test state: hotspot up, feeder powered (with
    /// `--power`), the UART held and `server` answering.
    pub(super) fn start(&mut self, server: Server) -> Result<()> {
        let was = if self.hotspot_active()? {
            Hotspot::Up
        } else {
            Hotspot::Down
        };
        self.hotspot_was = Some(was);
        match was {
            Hotspot::Up => self.hotspot_now = Hotspot::Up,
            Hotspot::Down => self.hotspot(Hotspot::Up)?,
        }

        if self.power {
            let on = self.bench.get(POWER)?;
            self.power_was_on = Some(on);
            if !on {
                self.note("Feeder supply was off; switching it on");
                self.bench.set(STRAP, Switch::Off)?;
            }
            // Also connects a powered feeder's UART, which may be isolated.
            self.bench.power_on()?;
        }

        let (root, name) = (self.bench.root.clone(), self.name.clone());
        let segment = move |n: u32| root.join(CAPTURES).join(format!("{name}-uart-{n:02}"));
        let port = self.bench.config.uart_port.clone();
        self.capture = Some(
            self.bench
                .tools
                .capture(&port, SEGMENT, Box::new(segment))?,
        );
        self.uart()?;
        self.server(server)
    }

    fn stock_bin(&self) -> PathBuf {
        self.stock_bin.clone()
    }

    fn segment_path(&self, n: u32) -> PathBuf {
        self.bench
            .root
            .join(CAPTURES)
            .join(format!("{}-uart-{n:02}", self.name))
    }

    fn hotspot_active(&self) -> Result<bool> {
        let output = Command::new("nmcli")
            .args(["-t", "-f", "NAME", "connection", "show", "--active"])
            .stdin(Stdio::null())
            .output()
            .map_err(LiveError::Nmcli)?;
        require(output.status.success(), || LiveError::NmcliList {
            stderr: String::from_utf8_lossy(&output.stderr).trim().into(),
        })?;
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line == self.connection))
    }

    fn spawn(&self, program: &Path, args: &[&str]) -> Result<Running> {
        let log = File::options()
            .create(true)
            .append(true)
            .open(&self.server_log)?;
        let child = own_group(&mut Command::new(program))
            .current_dir(&self.bench.root)
            .args(args)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()?;
        let label = args.first().copied().unwrap_or_default();
        let tracked = self
            .bench
            .children
            .track(&child, ActivityKind::Stock, label);
        Ok((child, tracked))
    }

    /// Wait for `ready`, failing early if `child` exits first.
    fn until_ready(
        &self,
        child: &mut Child,
        what: &'static str,
        ready: impl FnMut() -> bool,
    ) -> Result<()> {
        let waited = wait_child(
            child,
            &[&self.bench.cancel],
            Some(Instant::now() + START_WAIT),
            Duration::from_millis(200),
            ready,
        )?;
        // A child still running is left for `stop_servers`, which the
        // restore runs whatever happened here.
        match waited {
            Waited::Ready => Ok(()),
            Waited::Cancelled => Err(wellfed_rig::Error::Interrupted.into()),
            Waited::Exited(status) => Err(LiveError::ChildExited {
                what,
                status,
                log: self.bench.rel(&self.server_log),
            }
            .into()),
            Waited::TimedOut => Err(LiveError::ChildTimeout { what }.into()),
        }
    }

    /// `serve`; as [`Server::StockTrial`], `--enable-ota-trial`, which
    /// refuses a binary not built from a clean commit.
    fn start_serve(&mut self, server: &Server) -> Result<()> {
        let mut args = vec!["serve", "--config", &self.config_arg];
        if *server == Server::StockTrial {
            args.push("--enable-ota-trial");
        }
        let mut child = self.spawn(&self.stock_bin(), &args)?;
        let ready = self.until_ready(&mut child.0, "wellfed-stock serve", || {
            self.api
                .get::<IgnoredAny>("/health", Auth::Anonymous)
                .is_ok()
        });
        self.children.serve = Some(child);
        ready
    }

    /// SIGTERM, which `serve` handles by stopping its broker child too; a
    /// kill if it is still running after [`STOP_WAIT`].
    fn stop(running: Option<Running>) {
        let Some((mut child, tracked)) = running else {
            return;
        };
        let pid = rustix::process::Pid::from_child(&child);
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);

        // Cleanup, often after an interrupt: no handle cancels this wait.
        let waited = wait_child(
            &mut child,
            &[],
            Some(Instant::now() + STOP_WAIT),
            Duration::from_millis(100),
            || false,
        );
        if !matches!(waited, Ok(Waited::Exited(_))) {
            let _ = child.kill();
            let _ = child.wait();
        }
        drop(tracked);
    }

    fn stop_servers(&mut self) {
        Self::stop(self.children.images.take());
        Self::stop(self.children.serve.take());
    }

    /// End the push still running, if any, and log what it said.
    fn stop_push(&mut self) {
        if let Some(mut pushing) = self.pushing.take() {
            pushing.stop();
            for note in pushing.notes() {
                self.note(&format!("  push: {note}"));
            }
        }
    }

    /// Put the bench back as [`Self::start`] found it. Safe to repeat.
    pub(super) fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;

        // An unattended run that did not end cleanly: power off first.
        if self.kill_armed == Armed::On
            && let Err(error) = self.bench.cut_power()
        {
            eprintln!("Could not cut relay 1 (cut the feeder supply by hand): {error}");
        }

        self.stop_push();
        self.stop_servers();
        if let Some(mut capture) = self.capture.take() {
            capture.stop();
        }
        self.bench.status.capture(None, 0.0);
        if let Some(was) = self.hotspot_was
            && was != self.hotspot_now
            && let Err(error) = self.hotspot(was)
        {
            eprintln!("Could not restore the hotspot {}: {error}", self.connection);
        }
        if self.power_was_on == Some(false)
            && let Err(error) = self.bench.cut_power()
        {
            eprintln!("Could not switch the feeder supply back off: {error}");
        }
    }
}

impl Drop for LiveRig<'_> {
    fn drop(&mut self) {
        self.restore();
    }
}

impl Rig for LiveRig<'_> {
    fn stage(&mut self, args: &OtaArgs) -> Result<Option<Staged>> {
        Ok(self.bench.ota_stage(args, &mut Timing::default())?)
    }

    fn review_build(&mut self, review: &Path) -> Result<String> {
        let manifest = wellfed_rig::load(&self.bench.root, &review.join("build-manifest.json"))?;
        Ok(ota::build_id(&manifest)?)
    }

    fn server(&mut self, server: Server) -> Result<()> {
        self.stop_servers();
        let label = match &server {
            Server::Serve => "wellfed-stock serve",
            Server::StockTrial => "wellfed-stock serve --enable-ota-trial",
        };
        self.note(&format!("  Server: {label}"));

        self.start_serve(&server)
    }

    fn offer(&mut self, staged: Option<&Staged>, fault: Fault) -> Result<()> {
        self.stop_push();
        let Some(staged) = staged else {
            return Ok(());
        };
        let job = InstallJob {
            keys: self.keys.clone(),
            config: self.config.clone(),
            manifest: staged.manifest.clone().into(),
            image: staged.image.clone().into(),
            // Stock has no trial to come back from: the owner's statement.
            downgrade: (staged.target == Target::Stock).then(|| ota::KEY_PATH.into()),
            fault: fault.device(),
        };
        if let Some(stage) = fault.stage() {
            self.note(&format!(
                "  Pushing version {} with a fault bank 0 must stop at {stage}",
                staged.version
            ));
        }
        self.pushing = Some(self.bench.tools.install(&job)?);
        Ok(())
    }

    fn offered(&mut self) -> Option<std::result::Result<(), Arc<PushError>>> {
        let pushing = self.pushing.as_mut()?;
        let notes = pushing.notes();
        let finished = pushing.finished();
        for note in notes {
            self.note(&format!("  push: {note}"));
        }
        finished
    }

    fn device(&mut self, ask: &Ask) -> Result<Answer> {
        Ok(self.device.ask(ask)?)
    }

    fn hold(&mut self) -> Result<()> {
        Ok(self.device.hold()?)
    }

    fn held(&mut self, ask: &Ask) -> Result<Answer> {
        Ok(self.device.ask_held(ask)?)
    }

    fn release(&mut self) {
        self.device.release();
    }

    fn intrude(&mut self, intruder: Intruder) -> Result<Rebuffed> {
        Ok(self.device.intrude(intruder)?)
    }

    fn unix_ms(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
    }

    fn status(&mut self) -> Result<StockStatus> {
        Ok(self.api.get("/api/status", Auth::Token)?)
    }

    fn hotspot(&mut self, hotspot: Hotspot) -> Result<()> {
        let verb = hotspot.verb();
        self.note(&format!("  nmcli connection {verb} {}", self.connection));
        let output = Command::new("nmcli")
            .args(["connection", verb, &self.connection])
            .stdin(Stdio::null())
            .output()
            .map_err(LiveError::Nmcli)?;
        require(output.status.success(), || LiveError::NmcliVerb {
            verb,
            stderr: String::from_utf8_lossy(&output.stderr).trim().into(),
        })?;
        self.hotspot_now = hotspot;
        Ok(())
    }

    fn power_cycle(&mut self) -> Result<()> {
        require(self.power, || LiveError::NoPower)?;
        self.note("  Cold boot: supply off, then on");
        self.bench.cut_power()?;
        self.bench.pause(self.bench.off_time)?;
        Ok(self.bench.power_on()?)
    }

    fn kill(&mut self) -> Result<()> {
        require(self.power, || LiveError::NoPower)?;
        self.note("  Relay 1 off: the feeder stays unpowered");
        // `restore` only ever switches the supply off, so it stays off.
        Ok(self.bench.cut_power()?)
    }

    fn arm_kill(&mut self, armed: Armed) {
        self.kill_armed = armed;
    }

    fn uart(&mut self) -> Result<Vec<u8>> {
        let capture = self.capture.as_mut().ok_or(LiveError::NoCapture)?;
        let segments = capture.segments();
        if let Some(failure) = capture.failure() {
            return Err(LiveError::CaptureStopped(failure).into());
        }
        for segment in segments.iter().skip(self.captures.len()) {
            self.captures.push(self.bench.rel(segment));
            self.bench
                .status
                .capture(Some(segment), SEGMENT.as_secs_f64());
        }

        let mut bytes = Vec::new();
        while let Some(segment) = segments.get(self.reading.0) {
            let (index, offset) = &mut self.reading;
            if let Ok(mut file) = File::open(segment.join("serial-rx.bin")) {
                file.seek(SeekFrom::Start(*offset))?;
                *offset += file.read_to_end(&mut bytes)? as u64;
            }
            // The next segment starts only once this one's observe exited,
            // after its last flush, so nothing more arrives here.
            if *index + 1 >= segments.len() {
                break;
            }
            *index += 1;
            *offset = 0;
        }
        Ok(bytes)
    }

    fn wait(&mut self, span: Duration) -> Result<()> {
        Ok(self.bench.pause(span)?)
    }

    fn clock(&self) -> Duration {
        self.started.elapsed()
    }

    fn note(&mut self, text: &str) {
        say!("{text}");
        self.bench.status.line(text);
    }

    fn scenario(&mut self, name: &str, state: StepState) {
        self.bench.status.mark(name, state);
    }

    /// The server finds the feeder's address by its MAC, so it must have
    /// joined the hotspot first. A lease left by an earlier server is told
    /// apart by its token.
    fn images(&mut self, review: &Path) -> Result<String> {
        Self::stop(self.children.images.take());
        let config = self.bench.root.join(&self.config_arg);
        let lease_path = ImageLease::path(&config);
        let read = |path: &Path| -> Option<ImageLease> {
            serde_json::from_slice(&std::fs::read(path).ok()?).ok()
        };
        let before = read(&lease_path);

        let review = review.display().to_string();
        self.note(&format!("  Image server: {review}"));
        let args = [
            "ota-images",
            "--config",
            &self.config_arg,
            "--directory",
            &review,
        ];
        let mut child = self.spawn(&self.stock_bin(), &args)?;
        let listener = SocketAddr::new(self.mqtt.ip(), IMAGE_PORT);
        let ready = self.until_ready(&mut child.0, "wellfed-stock ota-images", || {
            read(&lease_path).is_some_and(|lease| Some(&lease) != before.as_ref())
                && TcpStream::connect_timeout(&listener, Duration::from_millis(500)).is_ok()
        });
        self.children.images = Some(child);
        ready?;

        read(&lease_path)
            .map(|lease| lease.token.to_string())
            .ok_or_else(|| LiveError::LeaseGone.into())
    }

    fn stock_trial(&mut self, trial: &TrialRequest) -> Result<()> {
        self.api
            .post::<IgnoredAny>("/api/ota/trial", trial)
            .map(|_| ())
            .map_err(Error::from)
    }

    fn resolve_trial(&mut self, request_id: &str, note: &str) -> Result<()> {
        let body = TrialResolve { request_id, note };
        self.api
            .post::<IgnoredAny>("/api/ota/trial/resolve", &body)
            .map(|_| ())
            .map_err(Error::from)
    }

    fn trial_answered(&mut self, answer: &TrialAnswer) -> Result<()> {
        self.api
            .post::<IgnoredAny>("/api/ota/trial/answered", answer)
            .map(|_| ())
            .map_err(Error::from)
    }
}
