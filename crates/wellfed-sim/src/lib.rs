//! Deterministic replacement-feeder scenarios on synthetic hardware; no
//! device or network access.
//!
//! The harness boots `virtual-feeder-worker` (the real owner on the fake SDK
//! with a file-backed journal), steps it one millisecond at a time through a
//! scenario's actions, and checks every event against the safety invariants:
//! a durable reservation before any drive, no invalid motor pair, and outputs
//! inactive whenever a result is durable. Power cuts and watchdog resets
//! restart the worker on the same journal image.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command as Process, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use clap::ValueEnum;
use wellfed_formats::canonical::{self, sha256};
use serde::Serialize;
use serde_json::Value;

pub mod protocol;
pub mod scenario;

use protocol::{
    Code, Command, Event, Fault, Journal, MOTORS, Outcome, Profile, Reason, State, Status,
    StoragePhase, Submission,
};
use scenario::{Action, Expectation, Scenario};

/// How long the worker may take to answer one command.
const RESPONSE: Duration = Duration::from_secs(5);
const MAX_RESPONSE_BYTES: usize = 65536;
const MAX_EVENTS: usize = 10000;

/// The worker's exit status after it persists a power cut.
const POWER_CUT_EXIT: i32 = 75;

/// An invalid scenario, a worker failure, or a malformed protocol.
#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("Invalid scenario: {0}")]
    Scenario(serde_json::Error),
    #[error("{0}")]
    Invalid(&'static str),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),

    #[error("Worker start failed: {0}")]
    WorkerStart(std::io::Error),
    #[error("Worker exited {}: {detail}", code.map_or_else(|| "None".into(), |c| c.to_string()))]
    WorkerExited { code: Option<i32>, detail: String },
    #[error("Worker response timed out")]
    WorkerTimeout,
    #[error("Malformed worker event: {0}")]
    WorkerEvent(serde_json::Error),
    #[error("Worker response exceeded bounds")]
    WorkerBounds,
    #[error("Worker command pipe failed")]
    WorkerPipe,

    #[error("{0:?} requires simulated power")]
    Unpowered(Command),
}

pub type Result<T> = std::result::Result<T, HarnessError>;

fn require(condition: bool, message: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(HarnessError::Invalid(message))
    }
}

/// One worker process: a bounded request/response transport, with stderr
/// kept for diagnostics.
pub struct Worker {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<std::io::Result<String>>,
    errors: fs::File,
}

/// A value's command-line spelling.
fn argument(value: impl ValueEnum) -> String {
    value
        .to_possible_value()
        .map(|possible| possible.get_name().to_owned())
        .unwrap_or_default()
}

impl Worker {
    pub fn start(
        executable: &Path,
        image: &Path,
        profile: Profile,
        journal: Journal,
    ) -> Result<Self> {
        let errors = tempfile::tempfile()?;
        let mut child = Process::new(executable)
            .arg(image)
            .arg(argument(profile))
            .arg(argument(journal))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(errors.try_clone()?)
            .spawn()
            .map_err(HarnessError::WorkerStart)?;

        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            unreachable!("both pipes were requested above");
        };
        let (send, lines) = channel();
        // A reader thread feeds a channel, so `read` can wait with a deadline.
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if send.send(line).is_err() {
                    return;
                }
            }
        });

        Ok(Self {
            child,
            stdin,
            lines,
            errors,
        })
    }

    /// Events up to and including the next status or power cut.
    pub fn read(&mut self) -> Result<Vec<Event>> {
        let deadline = Instant::now() + RESPONSE;
        let (mut events, mut bytes) = (Vec::new(), 0);

        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = match self.lines.recv_timeout(left) {
                Ok(Ok(line)) => line,
                Ok(Err(_)) | Err(RecvTimeoutError::Disconnected) => return Err(self.exited()),
                Err(RecvTimeoutError::Timeout) => return Err(HarnessError::WorkerTimeout),
            };

            bytes += line.len() + 1;
            let event: Event = serde_json::from_str(&line).map_err(HarnessError::WorkerEvent)?;

            let done = matches!(event, Event::Status(_) | Event::PowerCut { .. });
            events.push(event);
            if done {
                return Ok(events);
            }
            if bytes > MAX_RESPONSE_BYTES || events.len() > MAX_EVENTS {
                return Err(HarnessError::WorkerBounds);
            }
        }
    }

    fn exited(&mut self) -> HarnessError {
        let code = self.wait(RESPONSE).ok().flatten();

        let mut detail = String::new();
        let _ = self.errors.rewind();
        let _ = (&self.errors).take(4096).read_to_string(&mut detail);

        HarnessError::WorkerExited { code, detail }
    }

    pub fn command(&mut self, command: &Command) -> Result<Vec<Event>> {
        let line = serde_json::to_string(command)?;
        writeln!(self.stdin, "{line}")
            .and_then(|()| self.stdin.flush())
            .map_err(|_| HarnessError::WorkerPipe)?;
        self.read()
    }

    /// The exit status, if the worker exits within `timeout`.
    pub fn wait(&mut self, timeout: Duration) -> std::io::Result<Option<i32>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(Some(status.code().unwrap_or(-1)));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub fn close(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Python's default `json.dumps(value, sort_keys=True)`, for the scenario hash
/// the earlier reports carry. Unlike canonical form it keeps the default
/// `", "` and `": "` separators.
fn python_dumps(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.extend_from_slice(b", ");
                }
                python_dumps(item, out);
            }
            out.push(b']');
        }

        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push(b'{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.extend_from_slice(b", ");
                }
                canonical::string(key, out);
                out.extend_from_slice(b": ");
                python_dumps(&map[key], out);
            }
            out.push(b'}');
        }

        Value::String(text) => canonical::string(text, out),
        other => out.extend_from_slice(other.to_string().as_bytes()),
    }
}

/// Hashes of the worker and of every source that shapes it.
#[derive(Debug, Serialize)]
struct Identity {
    worker_sha256: String,
    sources: BTreeMap<String, String>,
}

fn identities(root: &Path, executable: &Path) -> Identity {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name() != Some("target".as_ref()) {
                    walk(&path, out);
                }
            } else if matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("rs" | "toml" | "h")
            ) {
                out.push(path);
            }
        }
    }

    let mut paths = Vec::new();
    for krate in ["core", "amebad", "testkit", "sim"] {
        walk(&root.join(format!("crates/wellfed-{krate}")), &mut paths);
    }
    if let Ok(entries) = fs::read_dir(root.join("firmware/sim")) {
        paths.extend(entries.flatten().map(|e| e.path()).filter(|p| p.is_file()));
    }
    paths.push(root.join("Cargo.lock"));

    let sources = paths
        .iter()
        .filter_map(|p| {
            let data = fs::read(p).ok()?;
            let name = p.strip_prefix(root).unwrap_or(p).display().to_string();
            Some((name, sha256(&data)))
        })
        .collect();

    Identity {
        worker_sha256: fs::read(executable).map(|d| sha256(&d)).unwrap_or_default(),
        sources,
    }
}

/// Notes the harness adds to the worker's events in the report.
#[derive(Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Note {
    Boot { profile: Profile },
    Action { action: Value },
    PowerRemoved { outputs: &'static str },
    WatchdogResetRequested,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum Logged {
    Worker(Event),
    Harness(Note),
}

#[derive(Debug, Serialize)]
struct Entry {
    at_ms: u64,
    boot: u64,
    #[serde(flatten)]
    event: Logged,
}

#[derive(Debug, Serialize)]
struct Assertion {
    at_ms: u64,
    label: String,
    passed: bool,
    actual: Value,
    expected: Value,
}

/// The name tables for the indexes the report's events carry.
#[derive(Debug, Serialize)]
struct Enums {
    state: &'static [State],
    fault: &'static [Fault],
    outcome: &'static [Outcome],
    reason: &'static [Reason],
    submission: &'static [Submission],
    storage_phase: &'static [StoragePhase],
    motor: [&'static str; 4],
}

/// What a passing report does not establish.
#[derive(Debug, Serialize)]
struct Limits {
    synthetic_hardware_and_timing: bool,
    physical_reliability_verified: bool,
    full_boot_or_rtos_emulated: bool,
    real_flash_provider: bool,
    network_or_scheduler_implemented: bool,
}

/// One scenario's evidence. Serialize it through [`serde_json::to_value`]
/// so its keys come out sorted, as the reports always have.
#[derive(Debug, Serialize)]
pub struct Report {
    version: u32,
    pub name: String,
    profile: Profile,
    scenario_sha256: String,
    identity: Identity,
    enums: Enums,
    limits: Limits,
    events: Vec<Entry>,
    assertions: Vec<Assertion>,
    pub passed: bool,
}

/// One scenario run: the worker for the current boot, the latest status, and
/// the harness's own view of storage that the invariants are checked against.
pub struct Simulation<'a> {
    raw: &'a Value,
    scenario: Scenario,
    root: PathBuf,
    executable: PathBuf,
    image: PathBuf,
    worker: Option<Worker>,

    now: u64,
    boots: u64,

    latest: Status,
    last_submission: Option<Submission>,

    events: Vec<Entry>,
    assertions: Vec<Assertion>,

    // `armed`: a `cut` or `power_cut` was sent, so a worker power cut is expected.
    // `reserved`: a reservation write ended ok and no finish write has ended since.
    armed: bool,
    reserved: bool,
    phase: StoragePhase,
}

fn json(value: impl Serialize) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

impl<'a> Simulation<'a> {
    pub fn new(raw: &'a Value, root: &Path, executable: &Path, directory: &Path) -> Result<Self> {
        Ok(Self {
            raw,
            scenario: Scenario::from_value(raw)?,
            root: root.to_path_buf(),
            executable: executable.to_path_buf(),
            image: directory.join("journal.bin"),
            worker: None,
            now: 0,
            boots: 0,
            latest: Status::default(),
            last_submission: None,
            events: Vec::new(),
            assertions: Vec::new(),
            armed: false,
            reserved: false,
            phase: StoragePhase::None,
        })
    }

    fn record(&mut self, event: Logged) {
        self.events.push(Entry {
            at_ms: self.now,
            boot: self.boots,
            event,
        });
    }

    fn note(&mut self, note: Note) {
        self.record(Logged::Harness(note));
    }

    fn check(&mut self, label: impl Into<String>, passed: bool, actual: Value, expected: Value) {
        self.assertions.push(Assertion {
            at_ms: self.now,
            label: label.into(),
            passed,
            actual,
            expected,
        });
    }

    fn invariant(&mut self, label: &str, passed: bool) {
        self.check(label, passed, Value::Null, Value::Null);
    }

    fn consume(&mut self, events: Vec<Event>) -> Result<()> {
        let mut reset = false;

        for event in events {
            self.record(Logged::Worker(event));
            match event {
                Event::StorageBegin { phase, .. } => {
                    self.phase = phase;
                    // A new reservation does not count until its write ends ok.
                    if phase == StoragePhase::Reserve {
                        self.reserved = false;
                    }
                }

                Event::StorageEnd { ok, .. } => {
                    if self.phase == StoragePhase::Reserve && ok {
                        self.reserved = true;
                    } else if self.phase == StoragePhase::Finish {
                        self.reserved = false;
                    }
                    self.phase = StoragePhase::None;
                }

                Event::Outputs { motor, .. } => {
                    if matches!(motor, 1 | 2) {
                        self.invariant("reservation_before_drive", self.reserved);
                    }
                    self.invariant("valid_motor_pair", motor != 3);
                }

                Event::Status(status) => {
                    self.latest = status;
                    if status.durable {
                        let inactive = status.motor == 0
                            && status.optics == 0
                            && status.stopped
                            && status.optics_off
                            && !status.pending;
                        self.invariant("durable_result_after_inactive_outputs", inactive);
                    }
                    // Applied after this batch, as a reboot on the same journal.
                    reset = status.watchdog_resets > 0;
                }

                Event::PowerCut { .. } => self.power_cut()?,
                Event::Feedback { .. } | Event::JournalIo { .. } => {}
            }
        }

        if reset && self.worker.is_some() {
            self.note(Note::WatchdogResetRequested);
            self.offline();
            self.boot()?;
        }

        Ok(())
    }

    fn power_cut(&mut self) -> Result<()> {
        let worker = self.worker.as_mut().filter(|_| self.armed);
        let Some(worker) = worker else {
            return Err(HarnessError::Invalid("Unexpected worker power cut"));
        };

        match worker.wait(RESPONSE) {
            Ok(Some(POWER_CUT_EXIT)) => {}
            Ok(None) => {
                return Err(HarnessError::Invalid(
                    "Worker failed to exit after power cut",
                ));
            }
            _ => return Err(HarnessError::Invalid("Invalid power-cut worker exit")),
        }

        self.note(Note::PowerRemoved {
            outputs: "unpowered",
        });
        self.offline();
        Ok(())
    }

    fn offline(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.close();
        }
        self.armed = false;
        self.reserved = false;
        self.phase = StoragePhase::None;
    }

    fn boot(&mut self) -> Result<()> {
        require(
            self.worker.is_none(),
            "boot requires the simulated power to be off",
        )?;

        self.boots += 1;
        self.last_submission = None;

        // Only the first boot provisions; later boots recover from the journal.
        let journal = if self.boots == 1 {
            Journal::Provision
        } else {
            Journal::Recover
        };
        let profile = self.scenario.profile;
        let mut worker = Worker::start(&self.executable, &self.image, profile, journal)?;

        // Keep the worker before checking its boot output, so `offline` stops it.
        let first = worker.read();
        self.worker = Some(worker);
        self.note(Note::Boot { profile });
        self.consume(first?)?;

        let period = self.scenario.feedback_period_ms;
        self.send(&Command::Auto { args: [period] })
    }

    fn send(&mut self, command: &Command) -> Result<()> {
        let Some(worker) = self.worker.as_mut() else {
            return Err(HarnessError::Unpowered(*command));
        };

        if command.cuts_power() {
            self.armed = true;
        }

        let events = worker.command(command)?;
        self.consume(events)?;

        if let Command::Submit { .. } = command {
            let submission = Submission::from_index(self.latest.response);
            require(submission.is_some(), "Invalid submission response")?;
            self.last_submission = submission;
        }

        Ok(())
    }

    fn expect(&mut self, expected: &Expectation) -> Result<()> {
        require(
            self.worker.is_some() || expected.power_only(),
            "Only online/boots can be asserted while power is off",
        )?;

        let status = self.latest;
        let online = self.worker.is_some();
        let (boots, submission) = (self.boots, self.last_submission);

        // In key order, as the reports have always listed them.
        macro_rules! compare {
            ($($field:ident: $actual:expr),+ $(,)?) => {$(
                if let Some(want) = expected.$field {
                    let got: Option<_> = $actual.into();
                    let passed = got == Some(want);
                    self.check(concat!("expect.", stringify!($field)), passed, json(got), json(want));
                }
            )+};
        }

        compare!(
            boots: boots,
            completed: status.completed,
            device_ms: status.device_ms,
            durable: status.durable,
            fault: status.fault,
            high_water_id: status.high_water_id,
            id: status.id,
            journal_error: status.journal_error,
            motor: status.motor,
            online: online,
            optics: status.optics,
            optics_off: status.optics_off,
            outcome: status.outcome,
            pending: status.pending,
            phase_known: status.phase_known,
            reason: status.reason,
            response: status.response,
            state: status.state,
            stopped: status.stopped,
            submission: submission,
            supervisor_fault: status.supervisor_fault,
            watchdog_fault: status.watchdog_fault,
            watchdog_resets: status.watchdog_resets,
        );

        Ok(())
    }

    pub fn run(mut self) -> Result<Report> {
        // Power off before surfacing a step's error, so no worker outlives the run.
        let result = self.steps();
        self.offline();
        result?;

        let mut scenario_text = Vec::new();
        python_dumps(self.raw, &mut scenario_text);
        let passed = self.assertions.iter().all(|a| a.passed);

        Ok(Report {
            version: 1,
            name: self.scenario.name,
            profile: self.scenario.profile,
            scenario_sha256: sha256(&scenario_text),
            identity: identities(&self.root, &self.executable),
            enums: Enums {
                state: State::ALL,
                fault: Fault::ALL,
                outcome: Outcome::ALL,
                reason: Reason::ALL,
                submission: Submission::ALL,
                storage_phase: StoragePhase::ALL,
                motor: MOTORS,
            },
            limits: Limits {
                synthetic_hardware_and_timing: true,
                physical_reliability_verified: false,
                full_boot_or_rtos_emulated: false,
                real_flash_provider: false,
                network_or_scheduler_implemented: false,
            },
            events: self.events,
            assertions: self.assertions,
            passed,
        })
    }

    fn steps(&mut self) -> Result<()> {
        self.boot()?;

        let steps = std::mem::take(&mut self.scenario.steps);
        let mut next = steps.into_iter().peekable();

        // Each millisecond: tick, apply that millisecond's actions, then poll.
        for now in 0..=self.scenario.end_ms {
            self.now = now;
            if now > 0 && self.worker.is_some() {
                self.send(&Command::TICK)?;
            }

            while let Some(step) = next.next_if(|s| s.at_ms == now) {
                self.note(Note::Action { action: step.raw });
                match &step.action {
                    Action::Expect { values } => self.expect(values)?,
                    Action::Boot { .. } => self.boot()?,
                    Action::Command(command) => self.send(command)?,
                }
            }

            if self.worker.is_some() {
                self.send(&Command::POLL)?;
            }
        }

        Ok(())
    }
}

/// Run one scenario on a fresh journal under `root/private/verification`.
pub fn run_scenario(scenario: &Value, root: &Path, executable: &Path) -> Result<Report> {
    Scenario::from_value(scenario)?;

    let directory = root.join("private/verification");
    fs::create_dir_all(&directory)?;
    let scratch = tempfile::Builder::new()
        .prefix("virtual-feeder-")
        .tempdir_in(&directory)?;

    Simulation::new(scenario, root, executable, scratch.path())?.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_dumps_matches_the_default_separators() {
        let mut out = Vec::new();
        python_dumps(
            &serde_json::json!({"b": [1, true, null], "a": "é"}),
            &mut out,
        );
        assert_eq!(out, b"{\"a\": \"\\u00e9\", \"b\": [1, true, null]}");
    }
}
