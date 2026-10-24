//! The live [`Rig`]: the bench's real hotspot, `petlibre-local` children,
//! UART captures, device API sessions as the owner's key and, with
//! `--power`, the feeder supply relay.
//!
//! The owner's standing authorization (AGENTS.md) covers bringing the
//! hotspot up and running `serve` as a bounded foreground process, provided
//! the prior network and service state comes back afterwards. So the rig
//! records the hotspot's and the supply's state before touching either,
//! refuses to start beside a `serve` it did not start, and restores
//! everything in [`LiveRig::restore`], which `Drop` also runs.
//!
//! The UART is held by back-to-back receive-only `petlibre-flash read
//! observe` segments of [`SEGMENT_SECONDS`]. The capture writer flushes each
//! received chunk, so the growing `serial-rx.bin` is read while the segment
//! runs. The chain runs on its own thread ([`Capture`]), so the port stays
//! held while the suite blocks on a build.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

use super::{Fault, ItestArgs, Rig, Server};
use crate::commands::Bench;
use crate::commands::ota::{OtaArgs, Staged, Timing};
use crate::config::{CAPTURES, VERIFICATION};
use crate::device::{Answer, Ask, Device, Intruder, Rebuffed};
use crate::local_api::{Auth, LocalApi, ServerConfig};
use crate::relay::{POWER, STRAP};
use crate::server::{LocalStatus, TrialRequest, TrialResolve};
use crate::status::StepState;
use crate::tools::Capture;
use crate::{Error, Result, require};

/// The observer's own limit for one capture.
const SEGMENT_SECONDS: f64 = 180.0;
const START_SECONDS: f64 = 20.0;
const STOP_SECONDS: f64 = 10.0;
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// `private/hotspot.json` (`--hotspot`).
#[derive(Deserialize)]
struct Hotspot {
    /// The NetworkManager connection that serves the feeder's network.
    connection: String,
}

/// `POST /api/replacement/ota/offer`; no fields clears the offer.
#[derive(Serialize)]
struct Offer<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    manifest: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image: Option<&'a str>,
}

/// `petlibre-local ota-images` writes this beside the config; only its token
/// goes into the trial request.
#[derive(Deserialize, PartialEq)]
struct ImageLease {
    token: String,
}

/// `ota-images` writes it beside the server config.
const IMAGE_LEASE: &str = "ota-image-server.json";
/// The fixed trial port stock's URL names (`petlibre-local` `ota::image::PORT`).
const IMAGE_PORT: u16 = 8088;

#[derive(Default)]
struct Children {
    serve: Option<Child>,
    broker: Option<Child>,
    fault: Option<Child>,
    images: Option<Child>,
}

pub(super) struct LiveRig<'b> {
    bench: &'b mut Bench,
    name: String,
    started: Instant,
    config_arg: String,
    api: LocalApi,
    device: Device,
    mqtt: SocketAddr,
    connection: String,
    /// The hotspot's state before the run; `None` until recorded.
    hotspot_was_up: Option<bool>,
    hotspot_up: bool,
    power: bool,
    power_was_on: Option<bool>,
    children: Children,
    local_bin: PathBuf,
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
    pub(super) fn open(bench: &'b mut Bench, args: &ItestArgs, local_bin: PathBuf) -> Result<Self> {
        let root = bench.root.clone();
        let name = args.name().to_string();
        let config: ServerConfig = crate::load(&root, &args.config)?;
        let hotspot: Hotspot = crate::load(&root, &args.hotspot)?;
        require(
            !hotspot.connection.is_empty(),
            format!("{} has no connection", args.hotspot.display()),
        )?;

        let rig = Self {
            name: name.clone(),
            started: Instant::now(),
            config_arg: args.config.display().to_string(),
            api: LocalApi::new(&config, HTTP_TIMEOUT),
            device: Device::open(&root, &args.keys, &args.config)?,
            mqtt: config.mqtt_listen,
            connection: hotspot.connection,
            hotspot_was_up: None,
            hotspot_up: false,
            power: args.power,
            power_was_on: None,
            children: Children::default(),
            capture: None,
            reading: (0, 0),
            captures: Vec::new(),
            server_log: root.join(VERIFICATION).join(format!("{name}-servers.log")),
            local_bin,
            restored: false,
            bench,
        };

        require(
            Path::new(&rig.bench.config.uart_port).exists(),
            format!("UART port {} is missing", rig.bench.config.uart_port),
        )?;
        require(
            rig.local_bin().is_file(),
            format!(
                "{} not found; cargo build --release -p petlibre-local --examples",
                rig.local_bin().display()
            ),
        )?;
        let fault_server = rig.example("ota_fault_server");
        require(
            fault_server.is_file(),
            format!(
                "{} not found; cargo build --release -p petlibre-local --examples",
                fault_server.display()
            ),
        )?;
        rig.bench.fresh(&rig.server_log)?;
        rig.bench.fresh(&rig.segment_path(1))?;
        require(
            rig.api
                .get::<IgnoredAny>("/health", Auth::Anonymous)
                .is_err(),
            format!(
                "Something already answers on {}; stop that serve first (the suite runs its own)",
                rig.api.address
            ),
        )?;
        Ok(rig)
    }

    /// Bring the bench to its test state: hotspot up, feeder powered (with
    /// `--power`), the UART held and `server` answering.
    pub(super) fn start(&mut self, server: Server) -> Result<()> {
        let up = self.hotspot_active()?;
        self.hotspot_was_up = Some(up);
        if up {
            self.hotspot_up = true;
        } else {
            self.hotspot(true)?;
        }

        if self.power {
            let on = self.bench.get(POWER)?;
            self.power_was_on = Some(on);
            if !on {
                self.note("Feeder supply was off; switching it on");
                self.bench.set(STRAP, false)?;
                self.bench.set(POWER, true)?;
            }
        }

        let (root, name) = (self.bench.root.clone(), self.name.clone());
        let segment = move |n: u32| root.join(CAPTURES).join(format!("{name}-uart-{n:02}"));
        let port = self.bench.config.uart_port.clone();
        self.capture = Some(
            self.bench
                .tools
                .capture(&port, SEGMENT_SECONDS, Box::new(segment))?,
        );
        self.uart()?;
        self.server(server)
    }

    fn local_bin(&self) -> PathBuf {
        self.local_bin.clone()
    }

    /// `cargo build --examples` puts them in `examples/` beside the binary.
    fn example(&self, name: &str) -> PathBuf {
        let bin = self.local_bin();
        bin.parent()
            .unwrap_or(Path::new("."))
            .join("examples")
            .join(name)
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
            .map_err(|error| Error::Bench(format!("nmcli: {error}")))?;
        require(
            output.status.success(),
            format!(
                "nmcli could not list connections: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        )?;
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line == self.connection))
    }

    fn spawn(&self, program: &Path, args: &[&str]) -> Result<Child> {
        let log = File::options()
            .create(true)
            .append(true)
            .open(&self.server_log)?;
        let child = Command::new(program)
            .current_dir(&self.bench.root)
            .args(args)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()?;
        Ok(child)
    }

    /// Wait for `ready`, failing early if `child` exits first.
    fn until_ready(&self, child: &mut Child, what: &str, ready: impl Fn() -> bool) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs_f64(START_SECONDS);
        while Instant::now() < deadline {
            if crate::interrupted() {
                return Err(Error::Interrupted);
            }
            if let Some(status) = child.try_wait()? {
                return Err(Error::Bench(format!(
                    "{what} exited ({status}); see {}",
                    self.bench.rel(&self.server_log)
                )));
            }
            if ready() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Err(Error::Bench(format!(
            "{what} did not become ready in {START_SECONDS:.0} s"
        )))
    }

    /// `serve`; with `trial`, `--enable-ota-trial`, which refuses a binary
    /// not built from a clean commit.
    fn start_serve(&mut self, trial: bool) -> Result<()> {
        let mut args = vec!["serve", "--config", &self.config_arg];
        if trial {
            args.push("--enable-ota-trial");
        }
        let mut child = self.spawn(&self.local_bin(), &args)?;
        let ready = self.until_ready(&mut child, "petlibre-local serve", || {
            self.api
                .get::<IgnoredAny>("/health", Auth::Anonymous)
                .is_ok()
        });
        self.children.serve = Some(child);
        ready
    }

    fn start_broker(&mut self) -> Result<()> {
        let mut child = self.spawn(&self.local_bin(), &["broker", "--config", &self.config_arg])?;
        let mqtt = self.mqtt;
        let ready = self.until_ready(&mut child, "petlibre-local broker", || {
            TcpStream::connect_timeout(&mqtt, Duration::from_millis(500)).is_ok()
        });
        self.children.broker = Some(child);
        ready
    }

    fn start_fault(&mut self, fault: Fault, staged: &Staged) -> Result<()> {
        let at = fault.at().to_string();
        let args = [
            "--config",
            &self.config_arg,
            "--manifest",
            &staged.manifest,
            "--image",
            &staged.image,
            "--fault",
            fault.arg(),
            "--at",
            &at,
        ];
        let child = self.spawn(&self.example("ota_fault_server"), &args)?;
        self.children.fault = Some(child);
        Ok(())
    }

    /// SIGTERM, which `serve` handles by stopping its broker child too; a
    /// kill if it is still running after [`STOP_SECONDS`].
    fn stop(child: Option<Child>) {
        let Some(mut child) = child else { return };
        let pid = rustix::process::Pid::from_child(&child);
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);

        let deadline = Instant::now() + Duration::from_secs_f64(STOP_SECONDS);
        while Instant::now() < deadline {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = child.kill();
        let _ = child.wait();
    }

    fn stop_servers(&mut self) {
        Self::stop(self.children.images.take());
        Self::stop(self.children.fault.take());
        Self::stop(self.children.serve.take());
        Self::stop(self.children.broker.take());
    }

    /// Put the bench back as [`Self::start`] found it. Safe to repeat.
    pub(super) fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;

        self.stop_servers();
        if let Some(mut capture) = self.capture.take() {
            capture.stop();
        }
        self.bench.status.capture(None, 0.0);
        if let Some(was) = self.hotspot_was_up
            && was != self.hotspot_up
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
        self.bench.ota_stage(args, &mut Timing::default())
    }

    fn review_build(&mut self, review: &Path) -> Result<String> {
        let manifest = crate::load(&self.bench.root, &review.join("build-manifest.json"))?;
        crate::commands::ota::build_id(&manifest)
    }

    fn server(&mut self, server: Server) -> Result<()> {
        self.stop_servers();
        let label = match &server {
            Server::Serve => "petlibre-local serve".to_string(),
            Server::StockTrial => "petlibre-local serve --enable-ota-trial".to_string(),
            Server::Faulty { fault, staged } => format!(
                "broker + ota_fault_server ({}, version {})",
                fault.arg(),
                staged.version
            ),
        };
        self.note(&format!("  Server: {label}"));

        match server {
            Server::Serve => self.start_serve(false),
            Server::StockTrial => self.start_serve(true),
            Server::Faulty { fault, staged } => {
                self.start_broker()?;
                self.start_fault(fault, &staged)
            }
        }
    }

    fn offer(&mut self, staged: Option<&Staged>) -> Result<()> {
        let body = Offer {
            manifest: staged.map(|s| s.manifest.as_str()),
            image: staged.map(|s| s.image.as_str()),
        };
        self.api
            .post::<IgnoredAny>("/api/replacement/ota/offer", &body)
            .map(|_| ())
    }

    fn device(&mut self, ask: &Ask) -> Result<Answer> {
        Ok(self.device.ask(ask)?)
    }

    fn intrude(&mut self, intruder: Intruder) -> Result<Rebuffed> {
        Ok(self.device.intrude(intruder)?)
    }

    fn unix_ms(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
    }

    fn status(&mut self) -> Result<LocalStatus> {
        self.api.get("/api/status", Auth::Token)
    }

    fn hotspot(&mut self, up: bool) -> Result<()> {
        let verb = if up { "up" } else { "down" };
        self.note(&format!("  nmcli connection {verb} {}", self.connection));
        let output = Command::new("nmcli")
            .args(["connection", verb, &self.connection])
            .stdin(Stdio::null())
            .output()
            .map_err(|error| Error::Bench(format!("nmcli: {error}")))?;
        require(
            output.status.success(),
            format!(
                "nmcli connection {verb} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        )?;
        self.hotspot_up = up;
        Ok(())
    }

    fn power_cycle(&mut self) -> Result<()> {
        require(self.power, "A power cycle needs --power")?;
        self.note("  Cold boot: supply off, then on");
        self.bench.cut_power()?;
        (self.bench.sleep)(self.bench.off_seconds)?;
        self.bench.set(POWER, true)
    }

    fn uart(&mut self) -> Result<Vec<u8>> {
        let capture = self
            .capture
            .as_mut()
            .ok_or_else(|| Error::Bench("The UART capture is not running".into()))?;
        let segments = capture.segments();
        if let Some(reason) = capture.failure() {
            return Err(Error::Bench(format!("The UART capture stopped: {reason}")));
        }
        for segment in segments.iter().skip(self.captures.len()) {
            self.captures.push(self.bench.rel(segment));
            self.bench.status.capture(Some(segment), SEGMENT_SECONDS);
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

    fn wait(&mut self, seconds: f64) -> Result<()> {
        (self.bench.sleep)(seconds)
    }

    fn clock(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
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
        let lease_path = config.parent().unwrap_or(Path::new(".")).join(IMAGE_LEASE);
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
        let mut child = self.spawn(&self.local_bin(), &args)?;
        let listener = SocketAddr::new(self.mqtt.ip(), IMAGE_PORT);
        let ready = self.until_ready(&mut child, "petlibre-local ota-images", || {
            read(&lease_path).is_some_and(|lease| Some(&lease) != before.as_ref())
                && TcpStream::connect_timeout(&listener, Duration::from_millis(500)).is_ok()
        });
        self.children.images = Some(child);
        ready?;

        read(&lease_path)
            .map(|lease| lease.token)
            .ok_or_else(|| Error::Bench("The image server's lease disappeared".into()))
    }

    fn stock_trial(&mut self, trial: &TrialRequest) -> Result<()> {
        self.api
            .post::<IgnoredAny>("/api/ota/trial", trial)
            .map(|_| ())
    }

    fn resolve_trial(&mut self, request_id: &str, note: &str) -> Result<()> {
        let body = TrialResolve { request_id, note };
        self.api
            .post::<IgnoredAny>("/api/ota/trial/resolve", &body)
            .map(|_| ())
    }
}
