//! The live [`Rig`]: the bench's real hotspot, `wellfed-local` children,
//! UART captures and, with `--power`, the feeder supply relay.
//!
//! The owner's standing authorization (AGENTS.md) covers bringing the
//! hotspot up and running `serve` as a bounded foreground process, provided
//! the prior network and service state comes back afterwards. So the rig
//! records the hotspot's and the supply's state before touching either,
//! refuses to start beside a `serve` it did not start, and restores
//! everything in [`LiveRig::restore`], which `Drop` also runs.
//!
//! The UART is held by back-to-back receive-only `wellfed-flash read
//! observe` segments of [`SEGMENT_SECONDS`]. The capture writer flushes each
//! received chunk, so the growing `serial-rx.bin` is read while the segment
//! runs. The chain runs on its own thread ([`Capture`]), so the port stays
//! held while the suite blocks on a build or the forger.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::de::{DeserializeOwned, IgnoredAny};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Fault, ItestArgs, Rig, Server};
use crate::commands::Bench;
use crate::commands::ota::{OtaArgs, Staged, Timing};
use crate::config::{CAPTURES, VERIFICATION};
use crate::relay::{POWER, STRAP};
use crate::server::{ForgedCase, LocalStatus, Request};
use crate::status::StepState;
use crate::tools::Capture;
use crate::{Error, Result, require};

/// The observer's own limit for one capture.
const SEGMENT_SECONDS: f64 = 180.0;
const START_SECONDS: f64 = 20.0;
const STOP_SECONDS: f64 = 10.0;
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
/// The forger waits up to 90 s for health, then 10 s per case.
const FORGE_SECONDS: u64 = 90;

/// The parts of `wellfed-local`'s config (`--config`) the rig uses.
#[derive(Deserialize)]
struct ServerConfig {
    api_token: String,
    http_listen: SocketAddr,
    mqtt_listen: SocketAddr,
}

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

/// The server's answer to a request it sent on.
#[derive(Deserialize)]
struct Sent {
    id: String,
}

/// Whether an API call carries the server's bearer token.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Auth {
    Token,
    /// `/health`, which answers anyone on loopback.
    Anonymous,
}

#[derive(Default)]
struct Children {
    serve: Option<Child>,
    broker: Option<Child>,
    fault: Option<Child>,
}

pub(super) struct LiveRig<'b> {
    bench: &'b mut Bench,
    name: String,
    started: Instant,
    config_arg: String,
    http: SocketAddr,
    mqtt: SocketAddr,
    token: String,
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
            http: config.http_listen,
            mqtt: config.mqtt_listen,
            token: config.api_token,
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
                "{} not found; cargo build --release -p wellfed-local --examples",
                rig.local_bin().display()
            ),
        )?;
        for example in ["ota_fault_server", "request_forger"] {
            let path = rig.example(example);
            require(
                path.is_file(),
                format!(
                    "{} not found; cargo build --release -p wellfed-local --examples",
                    path.display()
                ),
            )?;
        }
        rig.bench.fresh(&rig.server_log)?;
        rig.bench.fresh(&rig.segment_path(1))?;
        require(
            rig.get::<IgnoredAny>("/health", Auth::Anonymous).is_err(),
            format!(
                "Something already answers on {}; stop that serve first (the suite runs its own)",
                rig.http
            ),
        )?;
        Ok(rig)
    }

    /// Bring the bench to its test state: hotspot up, feeder powered (with
    /// `--power`), the UART held and `serve` answering.
    pub(super) fn start(&mut self) -> Result<()> {
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
        self.server(Server::Serve)
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

    fn start_serve(&mut self) -> Result<()> {
        let mut child = self.spawn(&self.local_bin(), &["serve", "--config", &self.config_arg])?;
        let ready = self.until_ready(&mut child, "wellfed-local serve", || {
            self.get::<IgnoredAny>("/health", Auth::Anonymous).is_ok()
        });
        self.children.serve = Some(child);
        ready
    }

    fn start_broker(&mut self) -> Result<()> {
        let mut child = self.spawn(&self.local_bin(), &["broker", "--config", &self.config_arg])?;
        let mqtt = self.mqtt;
        let ready = self.until_ready(&mut child, "wellfed-local broker", || {
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
        Self::stop(self.children.fault.take());
        Self::stop(self.children.serve.take());
        Self::stop(self.children.broker.take());
    }

    /// One HTTP/1.1 exchange with `serve`'s loopback API; the status code
    /// and JSON body.
    fn http(&self, method: &str, path: &str, auth: Auth, body: &str) -> Result<(u16, Value)> {
        let mut stream = TcpStream::connect_timeout(&self.http, Duration::from_secs(2))?;
        stream.set_read_timeout(Some(HTTP_TIMEOUT))?;
        stream.set_write_timeout(Some(HTTP_TIMEOUT))?;

        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
            self.http,
            body.len()
        );
        if auth == Auth::Token {
            request += &format!("Authorization: Bearer {}\r\n", self.token);
        }
        request += "\r\n";
        stream.write_all(request.as_bytes())?;
        stream.write_all(body.as_bytes())?;

        let mut response = Vec::new();
        stream.read_to_end(&mut response)?;
        parse_response(&response)
    }

    fn get<T: DeserializeOwned>(&self, path: &str, auth: Auth) -> Result<T> {
        match self.http("GET", path, auth, "")? {
            (200, value) => Ok(serde_json::from_value(value)?),
            (code, value) => Err(Error::Bench(format!("GET {path}: {code} {value}"))),
        }
    }

    fn post<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> Result<T> {
        let body = serde_json::to_string(body)?;
        match self.http("POST", path, Auth::Token, &body)? {
            (200, value) => Ok(serde_json::from_value(value)?),
            (code, value) => Err(Error::Bench(format!("POST {path}: {code} {value}"))),
        }
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

/// Split an HTTP/1.1 response into its status code and JSON body, decoding
/// a chunked body. The body stays a `Value` until the caller types it, so a
/// refusal's body can be shown whatever its shape.
pub(super) fn parse_response(response: &[u8]) -> Result<(u16, Value)> {
    let text = String::from_utf8_lossy(response);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| Error::Bench("Malformed HTTP response".into()))?;
    let code = head
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| Error::Bench("HTTP response has no status code".into()))?;

    let chunked = head.lines().any(|line| {
        let line = line.to_ascii_lowercase();
        line.starts_with("transfer-encoding:") && line.contains("chunked")
    });
    let body = if chunked {
        dechunk(body)
    } else {
        body.to_string()
    };
    let value = if body.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&body)?
    };
    Ok((code, value))
}

fn dechunk(mut body: &str) -> String {
    let mut out = String::new();
    while let Some((size, rest)) = body.split_once("\r\n") {
        let Ok(size) = usize::from_str_radix(size.trim(), 16) else {
            break;
        };
        if size == 0 || rest.len() < size {
            break;
        }
        out += &rest[..size];
        body = rest[size..].trim_start_matches("\r\n");
    }
    out
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
            Server::Serve => "wellfed-local serve".to_string(),
            Server::Broker => "wellfed-local broker alone".to_string(),
            Server::Faulty { fault, staged } => format!(
                "broker + ota_fault_server ({}, version {})",
                fault.arg(),
                staged.version
            ),
        };
        self.note(&format!("  Server: {label}"));

        match server {
            Server::Serve => self.start_serve(),
            Server::Broker => self.start_broker(),
            Server::Faulty { fault, staged } => {
                self.start_broker()?;
                self.start_fault(fault, &staged)
            }
        }
    }

    fn forge(&mut self) -> Result<Vec<ForgedCase>> {
        require(
            self.children.serve.is_none(),
            "request_forger runs beside the bare broker only",
        )?;
        let seconds = FORGE_SECONDS.to_string();
        let output = Command::new(self.example("request_forger"))
            .current_dir(&self.bench.root)
            .args(["--config", &self.config_arg, "--seconds", &seconds])
            .stdin(Stdio::null())
            .stderr(Stdio::inherit())
            .output()?;
        require(
            output.status.success(),
            format!("request_forger exited {}", output.status),
        )?;
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| Ok(serde_json::from_str(line)?))
            .collect()
    }

    fn offer(&mut self, staged: Option<&Staged>) -> Result<()> {
        let body = Offer {
            manifest: staged.map(|s| s.manifest.as_str()),
            image: staged.map(|s| s.image.as_str()),
        };
        self.post::<IgnoredAny>("/api/replacement/ota/offer", &body)
            .map(|_| ())
    }

    fn request(&mut self, request: &Request) -> Result<String> {
        let sent: Sent = self.post("/api/replacement/request", request)?;
        Ok(sent.id)
    }

    fn status(&mut self) -> Result<LocalStatus> {
        self.get("/api/status", Auth::Token)
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
}
