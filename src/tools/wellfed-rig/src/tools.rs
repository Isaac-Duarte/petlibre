//! The child processes the bench chains: `wellfed-flash`, `wellfed-build`
//! (the build and review for `ota --build`), and `sigrok-cli`; and the device API
//! sessions that push an update ([`Tools::install`]) and check its result.
//!
//! `wellfed-flash` runs as its own process, never linked in. Its plans bind
//! the SHA-256 of the running executable, and that has to stay the flash
//! client's own. It keeps its stdout summary, exit codes and Ctrl-C handling.

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use wellfed_formats::canonical::sha256;
use serde::Deserialize;

use crate::device::{Answer, Ask, Device, DeviceError, DeviceStatus, Fault};
use crate::{Cancel, Error, Result, Waited, wait_child};
use wellfed_formats::bench::progress::{FlashTool, Status};
use wellfed_formats::bench::relative;
use wellfed_formats::require;
use wellfed_formats::session::Status as SessionStatus;
use wellfed_signing::CONTAINER_HEADER;

/// How long a receive-only capture may take to hold the UART port.
const OBSERVER_START: Duration = Duration::from_secs(10);
/// How often a running child is looked at again.
const CHILD_POLL: Duration = Duration::from_millis(50);

pub trait Tools {
    /// Start a receive-only capture of `length` and return once it holds the
    /// UART port.
    fn observe(&mut self, port: &str, output: &Path, length: Duration)
    -> Result<Box<dyn Observer>>;
    /// Run one `wellfed-flash` command to completion, echoing and recording
    /// its output; returns its exit status (0 complete, 1 stopped, 2 refused).
    fn flash(&mut self, args: &[String], status: &mut Status) -> Result<i32>;
    /// The feeder's status over the device API, as the owner key file at
    /// `keys` (root-relative); `config` is the server config whose hotspot
    /// fields find the feeder when mDNS does not.
    fn device_status(&mut self, keys: &Path, config: &Path) -> Result<DeviceStatus>;
    /// Cancel any update and forget its recorded candidate over the device
    /// API, as [`Self::device_status`] reaches the feeder; the answer.
    fn cancel_update(&mut self, keys: &Path, config: &Path) -> Result<Answer>;
    /// Run one `wellfed-build` command (a firmware build or review) from the
    /// checkout root, streaming both of its outputs as they come; returns its
    /// exit status.
    fn build(&mut self, args: &[String], status: &mut Status) -> Result<i32>;
    /// Run one `wellfed-build` query (`source-id`) and keep its output.
    fn build_query(&mut self, args: &[String]) -> Result<Captured>;
    /// Hold the UART with back-to-back receive-only captures of `length`
    /// each, the `n`th (from 1) into `segment(n)`, until stopped.
    fn capture(
        &mut self,
        port: &str,
        length: Duration,
        segment: Box<dyn Fn(u32) -> PathBuf + Send>,
    ) -> Result<Box<dyn Capture>>;
    /// Start one sigrok capture of all eight channels as raw bytes, bounded
    /// to `length`.
    fn analyzer(
        &mut self,
        output: &Path,
        length: Duration,
        samplerate: u64,
    ) -> Result<Box<dyn Analyzer>>;
    fn sigrok_available(&self) -> bool;
    fn flash_tool(&self) -> Option<FlashTool>;
    /// Push the update `job` names over the device API on a thread of its
    /// own, until bank 1 is activated or the install stops.
    fn install(&mut self, job: &InstallJob) -> Result<Box<dyn Installing>>;
}

/// An update to push: root-relative paths under `private/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallJob {
    /// The owner key file the sessions open with, and the server config
    /// whose hotspot fields find the feeder.
    pub keys: PathBuf,
    pub config: PathBuf,
    /// The signed version 2 manifest, and the review's `image.bin` (an OTA
    /// container; its bank contents are what the manifest names).
    pub manifest: PathBuf,
    pub image: PathBuf,
    /// Sign a downgrade statement with this key, bound to the feeder and its
    /// floor as the feeder reports them: stock always needs one.
    pub downgrade: Option<PathBuf>,
    pub fault: Fault,
}

/// Why a child tool or a device API answer stopped a sequence.
#[derive(Debug, thiserror::Error)]
pub enum ToolsError {
    /// The serial client is a separate executable the bench never links.
    #[error(
        "{} not found; build it with cargo build -p wellfed-flash or pass --flash-bin",
        path.display()
    )]
    FlashMissing { path: PathBuf },
    #[error(
        "{} not found; build it with cargo build --release -p wellfed-build or pass --build-bin",
        path.display()
    )]
    BuildMissing { path: PathBuf },
    #[error("Observer exited before holding the port")]
    ObserverExited,
    #[error("Observer did not hold the port in time")]
    ObserverTimeout,
    #[error("Observer failed; see {}", output.display())]
    ObserverFailed { output: PathBuf },
    /// The OTA container has no payload past its header.
    #[error("{} is too short", image.display())]
    ImageTooShort { image: PathBuf },
    /// The key that signs a downgrade statement; `key` root-relative.
    #[error("{}: {error}", key.display())]
    DowngradeKey {
        key: PathBuf,
        error: wellfed_signing::Error,
    },
    #[error("Status answered {0:?}")]
    StatusAnswer(Answer),
}

/// How a push ended short of activating bank 1. It is shared ([`Arc`]),
/// since every poll of [`Installing::finished`] reports the same end and
/// the device errors it wraps cannot be cloned.
#[derive(Debug, thiserror::Error)]
pub enum PushError {
    /// The session, or the install over it, stopped.
    #[error(transparent)]
    Device(#[from] DeviceError),
    /// The downgrade statement could not be signed.
    #[error(transparent)]
    Downgrade(#[from] wellfed_signing::Error),
    /// Asking for the feeder's floor, which a downgrade binds, failed.
    #[error("the feeder's floor: Err({0:?})")]
    FloorUnasked(DeviceError),
    /// The feeder answered the floor query with something else.
    #[error("the feeder's floor: Ok({0:?})")]
    FloorAnswer(Answer),
    #[error("the install thread panicked")]
    Panicked,
}

/// An update being pushed.
pub trait Installing {
    /// How it ended, once it has: `Ok` once bank 1 is activated. Every
    /// call after the end reports it again.
    fn finished(&mut self) -> Option<std::result::Result<(), Arc<PushError>>>;
    /// Its steps since the last call.
    fn notes(&mut self) -> Vec<String>;
    /// End it at its next poll and wait for that.
    fn stop(&mut self);
}

/// [`Tools::install`] on a thread with its own device session.
struct Pushing {
    stop: Arc<AtomicBool>,
    notes: mpsc::Receiver<String>,
    done: Option<std::thread::JoinHandle<std::result::Result<(), PushError>>>,
    result: Option<std::result::Result<(), Arc<PushError>>>,
}

impl Pushing {
    fn start(root: &Path, job: &InstallJob) -> Result<Self> {
        let manifest = fs::read(root.join(&job.manifest))?;
        let container = fs::read(root.join(&job.image))?;
        require(container.len() > CONTAINER_HEADER, || {
            ToolsError::ImageTooShort {
                image: job.image.clone(),
            }
        })?;
        let mut device = Device::open(root, &job.keys, &job.config)?;
        let downgrade =
            match &job.downgrade {
                Some(key) => Some(wellfed_signing::Key::load(&root.join(key)).map_err(
                    |error| ToolsError::DowngradeKey {
                        key: key.clone(),
                        error,
                    },
                )?),
                None => None,
            };
        let fault = job.fault;
        let stop = Arc::new(AtomicBool::new(false));
        let (send, notes) = mpsc::channel();
        let stopping = Arc::clone(&stop);
        let done = std::thread::spawn(move || {
            let mut note = |text: String| {
                let _ = send.send(text);
            };
            let statement = match &downgrade {
                Some(key) => {
                    let floor = match device.ask(&Ask::UpdateState) {
                        Ok(Answer::Update(view)) => view.min_version,
                        Ok(other) => return Err(PushError::FloorAnswer(other)),
                        Err(error) => return Err(PushError::FloorUnasked(error)),
                    };
                    note(format!("Signing a downgrade at floor {floor}"));
                    let identity = device.identity();
                    Some(key.downgrade(&manifest, &identity, floor)?)
                }
                None => None,
            };
            let image = wellfed_client::update::Image {
                manifest: &manifest,
                downgrade: statement.as_deref(),
                payload: &container[CONTAINER_HEADER..],
            };
            Ok(device.install(image, fault, &stopping, &mut note)?)
        });
        Ok(Self {
            stop,
            notes,
            done: Some(done),
            result: None,
        })
    }
}

impl Installing for Pushing {
    fn finished(&mut self) -> Option<std::result::Result<(), Arc<PushError>>> {
        if self.result.is_none() && self.done.as_ref().is_some_and(|d| d.is_finished()) {
            let joined = self.done.take().map(std::thread::JoinHandle::join);
            self.result = Some(match joined {
                Some(Ok(result)) => result.map_err(Arc::new),
                _ => Err(Arc::new(PushError::Panicked)),
            });
        }
        self.result.clone()
    }

    fn notes(&mut self) -> Vec<String> {
        self.notes.try_iter().collect()
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(done) = self.done.take() {
            let joined = done.join().unwrap_or_else(|_| Err(PushError::Panicked));
            self.result = Some(joined.map_err(Arc::new));
        }
    }
}

impl Drop for Pushing {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A finished child's exit status and output.
#[derive(Debug, Clone, Default)]
pub struct Captured {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub trait Observer {
    /// Wait for the capture to end; its report's `result` on success.
    fn finish(&mut self) -> Result<Observation>;
    fn kill(&mut self);
}

/// The `result` of a `wellfed-flash read observe` report, as far as the
/// bench judges a boot or a loader entry by it. A missing field reads as
/// nothing seen, which never confirms a loader.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Observation {
    pub bytes: u64,
    pub nak_bytes: u64,
    pub longest_nak_run: u64,
    pub download_banner_seen: bool,
    /// How many stock `app_main:version:` lines arrived.
    pub stock_startup_markers: u64,
    pub fault_marker_seen: bool,
}

/// An observe's `report.json`: `running` once it holds the port, then
/// finished with its result.
#[derive(Deserialize)]
struct ObserveReport {
    #[serde(default)]
    status: Option<SessionStatus>,
    #[serde(default)]
    result: Option<Observation>,
}

fn observe_report(directory: &Path) -> Result<ObserveReport> {
    crate::load(directory, Path::new("report.json"))
}

/// A chain of receive-only captures on its own thread. Each observe is still
/// bounded (the flash client caps one at 180 s), but the next starts as soon
/// as the last exits, so the port stays held while the caller blocks on a
/// build or a child of its own.
pub trait Capture {
    /// The segment directories started so far, in order.
    fn segments(&mut self) -> Vec<PathBuf>;
    /// Why the chain stopped on its own (an observe failed, a name was
    /// taken); every call after the stop reports it again.
    fn failure(&mut self) -> Option<Arc<CaptureFailure>>;
    /// Stop the running observe and the chain; safe to repeat.
    fn stop(&mut self);
}

/// Why a capture chain stopped on its own. Shared ([`Arc`]) because every
/// later check of the chain reports the same stop.
#[derive(Debug, thiserror::Error)]
pub enum CaptureFailure {
    /// Segments are never reused, so a taken one ends the chain.
    #[error("Refusing to reuse {}", segment.display())]
    NameTaken { segment: PathBuf },
    #[error("wellfed-flash: {0}")]
    Spawn(std::io::Error),
    #[error("observe: {0}")]
    Wait(std::io::Error),
    /// The `n`th observe (from 1) exited unsuccessfully.
    #[error("Observe segment {n} exited {status}")]
    Exited {
        n: u32,
        status: std::process::ExitStatus,
    },
}

pub trait Analyzer {
    fn running(&mut self) -> bool;
    /// The exit status, or None if it is still running after `timeout`.
    fn wait(&mut self, timeout: Duration) -> Result<Option<i32>>;
    fn kill(&mut self);
    fn stderr(&mut self) -> String;
}

/// The real tools, run from the checkout root.
pub struct Processes {
    pub root: PathBuf,
    pub flash_bin: PathBuf,
    pub stock_bin: PathBuf,
    pub build_bin: PathBuf,
    /// Ends every wait on a child, which is then killed or left to its own
    /// interrupt handling.
    cancel: Cancel,
    /// Opened at the first status check and kept, with its session.
    device: Option<Device>,
}

impl Processes {
    pub fn new(
        root: PathBuf,
        flash_bin: PathBuf,
        stock_bin: PathBuf,
        build_bin: PathBuf,
        cancel: Cancel,
    ) -> Self {
        Self {
            root,
            flash_bin,
            stock_bin,
            build_bin,
            cancel,
            device: None,
        }
    }
}

fn beside_this(name: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

/// `wellfed-flash` beside this executable, as `cargo build` places it.
pub fn default_flash_bin() -> PathBuf {
    beside_this("wellfed-flash")
}

/// `wellfed-stock` beside this executable.
pub fn default_stock_bin() -> PathBuf {
    beside_this("wellfed-stock")
}

/// `wellfed-build` beside this executable.
pub fn default_build_bin() -> PathBuf {
    beside_this("wellfed-build")
}

fn flash_tool(root: &Path, path: &Path) -> Option<FlashTool> {
    let data = fs::read(path).ok()?;
    Some(FlashTool {
        path: relative(root, path),
        sha256: sha256(&data),
    })
}

impl Processes {
    fn flash_command(&self) -> Result<Command> {
        require(self.flash_bin.is_file(), || ToolsError::FlashMissing {
            path: self.flash_bin.clone(),
        })?;
        let mut command = Command::new(&self.flash_bin);
        command
            .current_dir(&self.root)
            .arg("--root")
            .arg(&self.root);
        Ok(command)
    }
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    // A signal-ended child counts as stopped.
    status.code().unwrap_or(1)
}

/// Run a child to completion: stderr streams as it comes, stdout (a summary)
/// is shown after; both are kept with the progress.
fn stream(mut command: Command, status: &mut Status) -> Result<i32> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    // Drain stdout on its own thread so neither pipe fills and blocks the
    // child while stderr streams here.
    let mut stdout = child.stdout.take().expect("piped stdout");
    let summary = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });

    for line in BufReader::new(child.stderr.take().expect("piped stderr")).lines() {
        let line = line?;
        eprintln!("{line}");
        status.line(&line);
    }

    let code = exit_code(child.wait()?);
    for line in summary.join().unwrap_or_default().lines() {
        say!("{line}");
        status.line(line);
    }
    Ok(code)
}

/// Run a child to completion with both outputs streamed line by line as they
/// come (a build prints its progress on stdout). Lines are read as bytes, so
/// output that is not UTF-8 is shown lossily rather than left unread.
fn stream_both(mut command: Command, status: &mut Status) -> Result<i32> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let (sender, lines) = mpsc::channel();
    let pipes: [(Box<dyn Read + Send>, bool); 2] = [
        (Box::new(child.stdout.take().expect("piped stdout")), false),
        (Box::new(child.stderr.take().expect("piped stderr")), true),
    ];
    let readers: Vec<_> = pipes
        .into_iter()
        .map(|(pipe, is_stderr)| {
            let sender = sender.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(pipe);
                let mut line = Vec::new();
                while matches!(reader.read_until(b'\n', &mut line), Ok(n) if n > 0) {
                    let text = String::from_utf8_lossy(&line).trim_end().to_string();
                    if sender.send((is_stderr, text)).is_err() {
                        break;
                    }
                    line.clear();
                }
            })
        })
        .collect();
    // The loop below ends once both readers have hung up.
    drop(sender);

    for (is_stderr, line) in lines {
        if is_stderr {
            eprintln!("{line}");
        } else {
            say!("{line}");
        }
        status.line(&line);
    }
    for reader in readers {
        let _ = reader.join();
    }
    Ok(exit_code(child.wait()?))
}

impl Processes {
    fn build_command(&self) -> Result<Command> {
        require(self.build_bin.is_file(), || ToolsError::BuildMissing {
            path: self.build_bin.clone(),
        })?;
        let mut command = Command::new(&self.build_bin);
        command.current_dir(&self.root);
        Ok(command)
    }
}

impl Tools for Processes {
    fn observe(
        &mut self,
        port: &str,
        output: &Path,
        length: Duration,
    ) -> Result<Box<dyn Observer>> {
        let child = self
            .flash_command()?
            .args(["read", "observe", "--port", port, "--output"])
            .arg(output)
            .args(["--seconds", &length.as_secs_f64().to_string()])
            .stdout(Stdio::null())
            .spawn()?;
        let mut observer = Observing {
            child,
            output: output.to_path_buf(),
        };

        let waited = wait_child(
            &mut observer.child,
            &[&self.cancel],
            Some(Instant::now() + OBSERVER_START),
            CHILD_POLL,
            || {
                observe_report(output)
                    .is_ok_and(|report| report.status == Some(SessionStatus::Running))
            },
        )?;
        match waited {
            Waited::Ready => Ok(Box::new(observer)),
            Waited::Exited(_) => Err(ToolsError::ObserverExited.into()),
            Waited::Cancelled => {
                observer.kill();
                Err(Error::Interrupted)
            }
            Waited::TimedOut => {
                observer.kill();
                Err(ToolsError::ObserverTimeout.into())
            }
        }
    }

    fn flash(&mut self, args: &[String], status: &mut Status) -> Result<i32> {
        let mut command = self.flash_command()?;
        command.args(args);
        stream(command, status)
    }

    fn install(&mut self, job: &InstallJob) -> Result<Box<dyn Installing>> {
        Ok(Box::new(Pushing::start(&self.root, job)?))
    }

    fn device_status(&mut self, keys: &Path, config: &Path) -> Result<DeviceStatus> {
        let device = match &mut self.device {
            Some(device) => device,
            None => self.device.insert(Device::open(&self.root, keys, config)?),
        };
        match device.ask(&Ask::Status)? {
            Answer::Status(status) => Ok(status),
            other => Err(ToolsError::StatusAnswer(other).into()),
        }
    }

    fn cancel_update(&mut self, keys: &Path, config: &Path) -> Result<Answer> {
        let device = match &mut self.device {
            Some(device) => device,
            None => self.device.insert(Device::open(&self.root, keys, config)?),
        };
        Ok(device.ask(&Ask::CancelUpdate)?)
    }

    fn build(&mut self, args: &[String], status: &mut Status) -> Result<i32> {
        let mut command = self.build_command()?;
        command.args(args);
        stream_both(command, status)
    }

    fn build_query(&mut self, args: &[String]) -> Result<Captured> {
        let output = self
            .build_command()?
            .args(args)
            .stdin(Stdio::null())
            .output()?;
        Ok(Captured {
            code: exit_code(output.status),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    fn capture(
        &mut self,
        port: &str,
        length: Duration,
        segment: Box<dyn Fn(u32) -> PathBuf + Send>,
    ) -> Result<Box<dyn Capture>> {
        // Checked here, so a missing binary refuses before anything starts.
        self.flash_command()?;
        let shared = Arc::new(Chained::default());
        let chain = Arc::clone(&shared);
        let port = port.to_string();
        let seconds = length.as_secs_f64().to_string();
        let (flash, root, cancel) = (
            self.flash_bin.clone(),
            self.root.clone(),
            self.cancel.clone(),
        );

        let thread = std::thread::spawn(move || {
            for n in 1.. {
                let output = segment(n);
                if output.exists() {
                    return chain.fail(CaptureFailure::NameTaken { segment: output });
                }
                let mut command = Command::new(&flash);
                command.current_dir(&root).arg("--root").arg(&root);
                let child = command
                    .args(["read", "observe", "--port", &port, "--output"])
                    .arg(&output)
                    .args(["--seconds", &seconds])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .spawn();
                let mut child = match child {
                    Ok(child) => child,
                    Err(error) => return chain.fail(CaptureFailure::Spawn(error)),
                };
                chain.started(output);

                let waited = wait_child(
                    &mut child,
                    &[&chain.stop, &cancel],
                    None,
                    CHILD_POLL,
                    || false,
                );
                let status = match waited {
                    Ok(Waited::Exited(status)) => status,
                    // With no deadline and nothing to be ready for, only a
                    // stop or the interrupt ends the wait early.
                    Ok(Waited::Cancelled | Waited::Ready | Waited::TimedOut) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return;
                    }
                    Err(error) => return chain.fail(CaptureFailure::Wait(error)),
                };
                if !status.success() {
                    return chain.fail(CaptureFailure::Exited { n, status });
                }
            }
        });
        Ok(Box::new(Chain {
            shared,
            thread: Some(thread),
        }))
    }

    fn analyzer(
        &mut self,
        output: &Path,
        length: Duration,
        samplerate: u64,
    ) -> Result<Box<dyn Analyzer>> {
        let channels: Vec<String> = (0..8).map(|i| format!("D{i}")).collect();
        let child = Command::new("sigrok-cli")
            .current_dir(&self.root)
            .args(["--driver", "fx2lafw", "--config"])
            .arg(format!("samplerate={samplerate}"))
            .arg("--time")
            .arg(length.as_millis().to_string())
            .arg("--channels")
            .arg(channels.join(","))
            .args(["--output-format", "binary", "--output-file"])
            .arg(output)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        Ok(Box::new(Sigrok {
            child,
            cancel: self.cancel.clone(),
        }))
    }

    fn sigrok_available(&self) -> bool {
        std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join("sigrok-cli").is_file())
        })
    }

    fn flash_tool(&self) -> Option<FlashTool> {
        flash_tool(&self.root, &self.flash_bin)
    }
}

#[derive(Default)]
struct Chained {
    segments: Mutex<Vec<PathBuf>>,
    failure: Mutex<Option<Arc<CaptureFailure>>>,
    /// The chain's own stop, apart from the operator's interrupt.
    stop: Cancel,
}

impl Chained {
    fn started(&self, output: PathBuf) {
        self.segments
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(output);
    }

    fn fail(&self, reason: CaptureFailure) {
        *self.failure.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(reason));
    }
}

struct Chain {
    shared: Arc<Chained>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Capture for Chain {
    fn segments(&mut self) -> Vec<PathBuf> {
        self.shared
            .segments
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn failure(&mut self) -> Option<Arc<CaptureFailure>> {
        self.shared
            .failure
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn stop(&mut self) {
        self.shared.stop.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Chain {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Observing {
    child: Child,
    output: PathBuf,
}

impl Observer for Observing {
    fn finish(&mut self) -> Result<Observation> {
        let code = exit_code(self.child.wait()?);
        require(code == 0, || ToolsError::ObserverFailed {
            output: self.output.clone(),
        })?;

        let report = observe_report(&self.output)?;
        Ok(report.result.unwrap_or_default())
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Sigrok {
    child: Child,
    cancel: Cancel,
}

impl Analyzer for Sigrok {
    fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn wait(&mut self, timeout: Duration) -> Result<Option<i32>> {
        let deadline = Instant::now() + timeout;
        let waited = wait_child(
            &mut self.child,
            &[&self.cancel],
            Some(deadline),
            CHILD_POLL,
            || false,
        )?;
        if let Waited::Exited(status) = waited {
            return Ok(Some(exit_code(status)));
        }

        // A capture that ended as the interrupt or the deadline came still
        // counts: its exit is looked at before either, as it always was.
        if let Some(status) = self.child.try_wait()? {
            return Ok(Some(exit_code(status)));
        }
        if self.cancel.is_cancelled() {
            return Err(Error::Interrupted);
        }
        Ok(None)
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn stderr(&mut self) -> String {
        let mut text = String::new();
        if let Some(stderr) = &mut self.child.stderr {
            let _ = stderr.read_to_string(&mut text);
        }
        text
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    use wellfed_formats::bench::progress::BenchCommand;

    /// Held by every test that writes or runs a child: a script being
    /// written while another test forks can fail to exec with ETXTBSY.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static CHILDREN: std::sync::Mutex<()> = std::sync::Mutex::new(());
        CHILDREN
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A stand-in `wellfed-flash`: progress on stderr, a summary on stdout.
    fn fake_flash(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("wellfed-flash");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn flash_streams_progress_and_returns_the_exit_status() {
        let _serial = serial();
        let tmp = tempfile::tempdir().unwrap();
        let flash_bin = fake_flash(
            tmp.path(),
            r#"echo "Read 4096/8192 bytes" >&2; echo '{"status":"failed"}'; exit 1"#,
        );
        let mut tools = Processes {
            root: tmp.path().into(),
            flash_bin,
            stock_bin: tmp.path().join("absent"),
            build_bin: tmp.path().join("absent"),
            cancel: Cancel::new(),
            device: None,
        };
        let mut status = Status::new(tmp.path(), tmp.path().join("status"));
        status.begin(BenchCommand::Dump, Some("unit"), &[]).unwrap();

        let code = tools
            .flash(&["read".into(), "full".into()], &mut status)
            .unwrap();

        assert_eq!(code, 1);
        let state = status.state().unwrap();
        assert_eq!(state.progress.as_ref().unwrap().done, 4096);
        assert_eq!(state.lines.last().unwrap(), r#"{"status":"failed"}"#);
    }

    #[test]
    fn observer_waits_for_the_port_then_returns_the_result() {
        let _serial = serial();
        let tmp = tempfile::tempdir().unwrap();
        // Arguments: --root R read observe --port P --output O --seconds S
        let flash_bin = fake_flash(
            tmp.path(),
            r#"mkdir -p "$8"; echo '{"status":"running"}' > "$8/report.json"; sleep 0.2
echo '{"status":"complete","result":{"bytes":7}}' > "$8/report.json""#,
        );
        let mut tools = Processes {
            root: tmp.path().into(),
            flash_bin,
            stock_bin: tmp.path().join("absent"),
            build_bin: tmp.path().join("absent"),
            cancel: Cancel::new(),
            device: None,
        };
        let output = tmp.path().join("capture");

        let mut observer = tools
            .observe("uart", &output, Duration::from_secs(1))
            .unwrap();
        assert_eq!(observer.finish().unwrap().bytes, 7);
    }

    #[test]
    fn an_observer_that_exits_early_is_a_stop() {
        let _serial = serial();
        let tmp = tempfile::tempdir().unwrap();
        let flash_bin = fake_flash(tmp.path(), "exit 2");
        let mut tools = Processes {
            root: tmp.path().into(),
            flash_bin,
            stock_bin: tmp.path().join("absent"),
            build_bin: tmp.path().join("absent"),
            cancel: Cancel::new(),
            device: None,
        };

        let error = tools
            .observe("uart", &tmp.path().join("c"), Duration::from_secs(1))
            .err()
            .unwrap();

        assert!(
            matches!(error, Error::Tool(ToolsError::ObserverExited)),
            "{error:?}"
        );
    }

    /// A chain over a stand-in observe that records one line per segment
    /// and fails the third; `body` runs first (a sleep, say).
    fn chain(tmp: &Path, body: &str) -> Box<dyn Capture> {
        let script = format!(
            "{body}\nmkdir -p \"$8\" && echo \"seg $8\" > \"$8/serial-rx.bin\"\n\
             n=$(cat \"$2/count\" 2>/dev/null || echo 0); n=$((n + 1)); echo $n > \"$2/count\"\n\
             [ $n -lt 3 ] || exit 3"
        );
        let mut tools = Processes {
            root: tmp.into(),
            flash_bin: fake_flash(tmp, &script),
            stock_bin: "/bin/sh".into(),
            build_bin: tmp.join("absent"),
            cancel: Cancel::new(),
            device: None,
        };
        let root = tmp.to_path_buf();
        tools
            .capture(
                "/dev/null",
                Duration::from_secs(180),
                Box::new(move |n| root.join(format!("uart-{n:02}"))),
            )
            .unwrap()
    }

    fn until(mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "timed out");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_capture_chain_starts_each_segment_as_the_last_exits() {
        let _serial = serial();
        let tmp = tempfile::tempdir().unwrap();
        let mut capture = chain(tmp.path(), "");

        until(|| capture.failure().is_some());

        let segments = capture.segments();
        assert_eq!(segments.len(), 3);
        for (n, segment) in segments.iter().enumerate() {
            assert_eq!(*segment, tmp.path().join(format!("uart-{:02}", n + 1)));
            assert!(segment.join("serial-rx.bin").is_file());
        }
        let failure = capture.failure().unwrap();
        assert!(
            matches!(*failure, CaptureFailure::Exited { n: 3, .. }),
            "{failure:?}"
        );
        // What the suite's run record keeps of the stop.
        assert_eq!(
            failure.to_string(),
            "Observe segment 3 exited exit status: 3"
        );
    }

    #[test]
    fn stopping_a_capture_chain_ends_the_running_observe() {
        let _serial = serial();
        let tmp = tempfile::tempdir().unwrap();
        let mut capture = chain(tmp.path(), "sleep 30");
        until(|| !capture.segments().is_empty());

        let started = Instant::now();
        capture.stop();

        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(capture.segments().len(), 1);
        assert!(capture.failure().is_none());
    }

    #[test]
    fn a_capture_chain_refuses_a_used_segment_name() {
        let _serial = serial();
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("uart-01")).unwrap();
        let mut capture = chain(tmp.path(), "");

        until(|| capture.failure().is_some());

        assert!(capture.segments().is_empty());
        let failure = capture.failure().unwrap();
        assert!(
            matches!(&*failure, CaptureFailure::NameTaken { segment } if *segment == tmp.path().join("uart-01")),
            "{failure:?}"
        );
    }

    #[test]
    fn the_builder_streams_both_outputs_from_the_root_and_must_exist() {
        let _serial = serial();
        let tmp = tempfile::tempdir().unwrap();
        let builder = tmp.path().join("wellfed-build");
        fs::write(
            &builder,
            "#!/bin/sh\necho \"$1 in $(pwd)\"; printf 'bad \\377\\n' >&2; exit 3\n",
        )
        .unwrap();
        fs::set_permissions(
            &builder,
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        let mut tools = Processes {
            root: tmp.path().into(),
            flash_bin: tmp.path().join("absent"),
            stock_bin: tmp.path().join("absent"),
            build_bin: builder,
            cancel: Cancel::new(),
            device: None,
        };
        let mut status = Status::new(tmp.path(), tmp.path().join("status"));
        status.begin(BenchCommand::Ota, Some("unit"), &[]).unwrap();

        assert_eq!(
            tools
                .build(&["build".into(), "firstboot".into()], &mut status)
                .unwrap(),
            3
        );
        let root = tmp.path().canonicalize().unwrap();
        let lines = &status.state().unwrap().lines;
        assert!(lines.contains(&format!("build in {}", root.display())));
        assert!(lines.contains(&"bad \u{fffd}".to_string()), "{lines:?}");
        let answer = tools.build_query(&["source-id".into()]).unwrap();
        assert_eq!(
            (answer.code, answer.stdout.split(' ').next()),
            (3, Some("source-id"))
        );

        tools.build_bin = tmp.path().join("absent");
        let error = tools.build_query(&[]).unwrap_err();
        assert!(
            matches!(&error, Error::Tool(ToolsError::BuildMissing { path }) if *path == tools.build_bin),
            "{error:?}"
        );
    }

    #[test]
    fn a_missing_flash_binary_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let mut tools = Processes {
            root: tmp.path().into(),
            flash_bin: tmp.path().join("absent"),
            stock_bin: tmp.path().join("absent"),
            build_bin: tmp.path().join("absent"),
            cancel: Cancel::new(),
            device: None,
        };
        let mut status = Status::disabled();

        let error = tools.flash(&[], &mut status).unwrap_err();
        assert!(
            matches!(&error, Error::Tool(ToolsError::FlashMissing { .. })),
            "{error:?}"
        );
        // The operator is told how to get the client.
        assert_eq!(
            error.to_string(),
            format!(
                "{} not found; build it with cargo build -p wellfed-flash or pass --flash-bin",
                tools.flash_bin.display()
            )
        );
    }
}
