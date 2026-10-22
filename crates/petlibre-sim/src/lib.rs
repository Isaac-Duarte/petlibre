//! Deterministic replacement-feeder scenarios on synthetic hardware; no
//! device or network access.
//!
//! The harness boots `virtual-feeder-worker` (the real owner on the fake SDK
//! with a file-backed journal), steps it one millisecond at a time through a
//! scenario's actions, and checks every event against the safety invariants:
//! a durable reservation before any drive, no invalid motor pair, and outputs
//! inactive whenever a result is durable. Power cuts and watchdog resets
//! restart the worker on the same journal image.

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

// Name tables for the enumerated status fields; the worker reports indexes
// into them, and `expect` actions and the report use the names.
pub const STATES: [&str; 9] = [
    "starting",
    "idle",
    "queued",
    "preparing",
    "feeding",
    "reporting",
    "terminal",
    "fault",
    "stopped",
];

pub const FAULTS: [&str; 7] = [
    "ok", "setup", "recovery", "storage", "hardware", "watchdog", "flash",
];

pub const OUTCOMES: [&str; 5] = ["pending", "done", "cancelled", "expired", "failed"];

pub const REASONS: [&str; 13] = [
    "none",
    "done",
    "cancelled",
    "clock",
    "observation_gap",
    "total_timeout",
    "feedback_timeout",
    "power",
    "sensor",
    "outlet",
    "overcurrent",
    "feedback_stream",
    "output",
];

pub const SUBMISSIONS: [&str; 6] = [
    "enqueued",
    "reject_busy",
    "reject_id",
    "reject_request",
    "reject_phase",
    "reject_stopped",
];

// The status fields an `expect` action may name, and the booleans among them.
const STATUS_FIELDS: [&str; 23] = [
    "device_ms",
    "response",
    "state",
    "fault",
    "supervisor_fault",
    "watchdog_fault",
    "id",
    "completed",
    "outcome",
    "reason",
    "durable",
    "stopped",
    "optics_off",
    "phase_known",
    "high_water_id",
    "pending",
    "journal_error",
    "motor",
    "optics",
    "watchdog_resets",
    "online",
    "boots",
    "submission",
];

const BOOLEAN_FIELDS: [&str; 6] = [
    "durable",
    "stopped",
    "optics_off",
    "phase_known",
    "pending",
    "online",
];

// Every event kind the worker may emit; anything else is a protocol error.
const EVENTS: [&str; 7] = [
    "status",
    "outputs",
    "storage_begin",
    "storage_end",
    "feedback",
    "power_cut",
    "journal_io",
];

/// How long the worker may take to answer one command.
const RESPONSE: Duration = Duration::from_secs(5);
const MAX_RESPONSE_BYTES: usize = 65536;
const MAX_EVENTS: usize = 10000;

/// An invalid scenario, a worker failure, or a malformed protocol.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct HarnessError(pub String);

pub type Result<T> = std::result::Result<T, HarnessError>;

fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(HarnessError(message.into()))
}

fn enum_names(field: &str) -> Option<&'static [&'static str]> {
    Some(match field {
        "state" => &STATES,
        "fault" => &FAULTS,
        "outcome" => &OUTCOMES,
        "reason" => &REASONS,
        "submission" => &SUBMISSIONS,
        _ => return None,
    })
}

/// Fixed unsigned integer arguments and their inclusive bounds; `None` for `expect`.
fn command_bounds(op: &str) -> Option<Option<&'static [(u64, u64)]>> {
    const ID: (u64, u64) = (0, u64::MAX);
    Some(Some(match op {
        "submit" => &[ID, (0, 65535)],
        "cancel" | "ack" => &[ID],
        "auto" => &[(0, 100000)],
        "edge" | "owner" | "timer" | "adc_stuck" => &[(0, 1)],
        "current" | "battery" => &[(0, 4095)],
        "inputs" => &[(0, 7)],
        "cut" => &[(1, 2), (0, 100000)],
        "status" | "stop" | "power_cut" | "boot" => &[],
        "expect" => return Some(None),
        _ => return None,
    }))
}

/// An integer (never a bool or float) in `low..=high`.
fn integer(value: &Value, low: u64, high: u64) -> bool {
    value.as_u64().is_some_and(|v| (low..=high).contains(&v))
}

fn keys(value: &Map<String, Value>, allowed: &[&str]) -> bool {
    value.keys().all(|k| allowed.contains(&k.as_str()))
}

/// Check a scenario fully before any worker starts.
pub fn validate(scenario: &Value) -> Result<&Value> {
    let Some(s) = scenario.as_object().filter(|s| {
        keys(
            s,
            &[
                "version",
                "name",
                "profile",
                "feedback_period_ms",
                "end_ms",
                "actions",
            ],
        )
    }) else {
        return fail("Unknown scenario fields");
    };

    if !integer(&s.get("version").cloned().unwrap_or(Value::Null), 1, 1) {
        return fail("Scenario version must be 1");
    }
    if !s
        .get("name")
        .and_then(Value::as_str)
        .is_some_and(|n| (1..=100).contains(&n.chars().count()))
    {
        return fail("Scenario needs a name of 1..100 characters");
    }

    if !matches!(
        s.get("profile").map_or(Some("single"), Value::as_str),
        Some("single" | "double")
    ) {
        return fail("Profile must be single or double");
    }
    if !integer(s.get("feedback_period_ms").unwrap_or(&json!(10)), 0, 100000) {
        return fail("Invalid feedback period");
    }

    let end = match s.get("end_ms") {
        Some(end) if integer(end, 0, 10000) => end.as_u64().unwrap(),
        _ => return fail("end_ms must be 0..10000"),
    };
    let Some(actions) = s
        .get("actions")
        .and_then(Value::as_array)
        .filter(|a| a.len() <= 10000)
    else {
        return fail("actions must be a list with at most 10000 entries");
    };

    let mut previous = 0;
    let mut expectations = 0;
    for action in actions {
        let Some(a) = action
            .as_object()
            .filter(|a| keys(a, &["at_ms", "op", "args", "values"]))
        else {
            return fail("Invalid action fields");
        };

        match a.get("at_ms") {
            Some(at) if integer(at, previous, end) => previous = at.as_u64().unwrap(),
            _ => return fail("Actions must have nondecreasing timestamps within end_ms"),
        }

        let Some((op, bounds)) = a
            .get("op")
            .and_then(Value::as_str)
            .and_then(|op| Some((op, command_bounds(op)?)))
        else {
            return fail("Unknown action");
        };

        match bounds {
            None => {
                let values = a.get("values").and_then(Value::as_object);
                let Some(values) = values.filter(|v| {
                    !a.contains_key("args") && !v.is_empty() && keys(v, &STATUS_FIELDS)
                }) else {
                    return fail("expect requires known, nonempty status values");
                };

                for (key, value) in values {
                    let valid = if BOOLEAN_FIELDS.contains(&key.as_str()) {
                        value.is_boolean()
                    } else if let Some(names) = enum_names(key) {
                        value.as_str().is_some_and(|v| names.contains(&v))
                    } else {
                        integer(value, 0, u64::MAX)
                    };
                    if !valid {
                        return fail(format!("Invalid expectation for {key}"));
                    }
                }

                expectations += 1;
            }

            Some(bounds) => {
                let empty = json!([]);
                let args = a.get("args").unwrap_or(&empty).as_array();
                let Some(args) =
                    args.filter(|args| !a.contains_key("values") && args.len() == bounds.len())
                else {
                    return fail(format!("Invalid arguments for {op}"));
                };
                if args
                    .iter()
                    .zip(bounds)
                    .any(|(value, (low, high))| !integer(value, *low, *high))
                {
                    return fail(format!("Arguments out of range for {op}"));
                }
            }
        }
    }

    if expectations == 0 {
        return fail("Scenario needs at least one expectation");
    }
    Ok(scenario)
}

/// One worker process: a bounded request/response transport, with stderr
/// kept for diagnostics.
pub struct Worker {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<std::io::Result<String>>,
    errors: fs::File,
}

impl Worker {
    pub fn start(executable: &Path, image: &Path, mode: u8, provision: bool) -> Result<Self> {
        let errors = tempfile::tempfile().map_err(|e| HarnessError(e.to_string()))?;
        let mut child = Command::new(executable)
            .arg(image)
            .arg(mode.to_string())
            .arg(u8::from(provision).to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(
                errors
                    .try_clone()
                    .map_err(|e| HarnessError(e.to_string()))?,
            )
            .spawn()
            .map_err(|e| HarnessError(format!("Worker start failed: {e}")))?;

        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
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
    pub fn read(&mut self) -> Result<Vec<Value>> {
        let deadline = Instant::now() + RESPONSE;
        let (mut events, mut bytes) = (Vec::new(), 0);

        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = match self.lines.recv_timeout(left) {
                Ok(Ok(line)) => line,
                Ok(Err(_)) | Err(RecvTimeoutError::Disconnected) => return Err(self.exited()),
                Err(RecvTimeoutError::Timeout) => return fail("Worker response timed out"),
            };

            bytes += line.len() + 1;
            let event: Value = serde_json::from_str(&line)
                .map_err(|_| HarnessError("Malformed worker JSON".into()))?;
            if !event["event"].as_str().is_some_and(|e| EVENTS.contains(&e)) {
                return fail("Unknown worker event");
            }

            let done = matches!(event["event"].as_str(), Some("status" | "power_cut"));
            events.push(event);
            if done {
                return Ok(events);
            }
            if bytes > MAX_RESPONSE_BYTES || events.len() > MAX_EVENTS {
                return fail("Worker response exceeded bounds");
            }
        }
    }

    fn exited(&mut self) -> HarnessError {
        let code = self.wait(RESPONSE).ok().flatten();
        let mut detail = String::new();
        let _ = self.errors.rewind();
        let _ = (&self.errors).take(4096).read_to_string(&mut detail);
        let code = code.map_or_else(|| "None".into(), |c| c.to_string());
        HarnessError(format!("Worker exited {code}: {detail}"))
    }

    pub fn command(&mut self, op: &str, args: &[u64]) -> Result<Vec<Value>> {
        let mut line = op.to_string();
        for arg in args {
            line += &format!(" {arg}");
        }
        writeln!(self.stdin, "{line}")
            .and_then(|()| self.stdin.flush())
            .map_err(|_| HarnessError("Worker command pipe failed".into()))?;
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
/// the earlier reports carry.
fn python_dumps(value: &Value, out: &mut String) {
    match value {
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                python_dumps(item, out);
            }
            out.push(']');
        }

        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                python_dumps(&Value::String(key.clone()), out);
                out.push_str(": ");
                python_dumps(&map[key], out);
            }
            out.push('}');
        }

        Value::String(text) => {
            out.push('"');
            for c in text.chars() {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    '\u{8}' => out.push_str("\\b"),
                    '\u{c}' => out.push_str("\\f"),
                    ' '..='~' => out.push(c),
                    _ => {
                        let mut units = [0u16; 2];
                        for unit in c.encode_utf16(&mut units) {
                            out.push_str(&format!("\\u{unit:04x}"));
                        }
                    }
                }
            }
            out.push('"');
        }

        other => out.push_str(&other.to_string()),
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Hashes of the worker and of every source that shapes it.
pub fn identities(root: &Path, executable: &Path) -> Value {
    let mut paths: BTreeSet<PathBuf> = BTreeSet::new();

    fn walk(dir: &Path, out: &mut BTreeSet<PathBuf>) {
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
                out.insert(path);
            }
        }
    }

    for krate in ["core", "amebad", "testkit", "sim"] {
        walk(&root.join(format!("crates/petlibre-{krate}")), &mut paths);
    }
    if let Ok(entries) = fs::read_dir(root.join("firmware/sim")) {
        paths.extend(entries.flatten().map(|e| e.path()).filter(|p| p.is_file()));
    }
    paths.insert(root.join("Cargo.lock"));

    let sources: Map<String, Value> = paths
        .iter()
        .filter_map(|p| {
            let data = fs::read(p).ok()?;
            Some((
                p.strip_prefix(root).unwrap_or(p).display().to_string(),
                json!(sha256_hex(&data)),
            ))
        })
        .collect();

    let worker = fs::read(executable)
        .map(|d| sha256_hex(&d))
        .unwrap_or_default();
    json!({"worker_sha256": worker, "sources": sources})
}

/// One scenario run: the worker for the current boot, the latest status, and
/// the harness's own view of storage that the invariants are checked against.
pub struct Simulation<'a> {
    scenario: &'a Value,
    root: PathBuf,
    executable: PathBuf,
    image: PathBuf,
    worker: Option<Worker>,

    now: u64,
    boots: u64,

    latest: Map<String, Value>,
    last_submission: Option<&'static str>,

    events: Vec<Value>,
    assertions: Vec<Value>,

    // `armed`: a `cut` or `power_cut` was sent, so a worker power cut is expected.
    // `reserved`: a reservation write ended ok and no finish write has ended since.
    armed: bool,
    reserved: bool,
    phase: u64,
}

impl<'a> Simulation<'a> {
    pub fn new(
        scenario: &'a Value,
        root: &Path,
        executable: &Path,
        directory: &Path,
    ) -> Result<Self> {
        validate(scenario)?;

        Ok(Self {
            scenario,
            root: root.to_path_buf(),
            executable: executable.to_path_buf(),
            image: directory.join("journal.bin"),
            worker: None,
            now: 0,
            boots: 0,
            latest: Map::new(),
            last_submission: None,
            events: Vec::new(),
            assertions: Vec::new(),
            armed: false,
            reserved: false,
            phase: 0,
        })
    }

    fn profile(&self) -> &str {
        self.scenario["profile"].as_str().unwrap_or("single")
    }

    fn record(&mut self, event: &Value) {
        let mut entry = Map::new();
        entry.insert("at_ms".into(), json!(self.now));
        entry.insert("boot".into(), json!(self.boots));
        if let Some(map) = event.as_object() {
            entry.extend(map.clone());
        }
        self.events.push(Value::Object(entry));
    }

    fn check(&mut self, label: &str, ok: bool, actual: Value, expected: Value) {
        self.assertions
            .push(json!({"at_ms": self.now, "label": label, "passed": ok,
                                    "actual": actual, "expected": expected}));
    }

    fn consume(&mut self, events: Vec<Value>) -> Result<()> {
        let mut reset = false;

        for event in events {
            self.record(&event);
            match event["event"].as_str().unwrap_or("") {
                "storage_begin" => {
                    self.phase = event["phase"].as_u64().unwrap_or(0);
                    // A new reservation does not count until its write ends ok.
                    if self.phase == 1 {
                        self.reserved = false;
                    }
                }

                "storage_end" => {
                    if self.phase == 1 && event["ok"] == true {
                        self.reserved = true;
                    } else if self.phase == 2 {
                        self.reserved = false;
                    }
                    self.phase = 0;
                }

                "outputs" => {
                    let motor = event["motor"].as_u64();
                    if matches!(motor, Some(1 | 2)) {
                        self.check(
                            "reservation_before_drive",
                            self.reserved,
                            Value::Null,
                            Value::Null,
                        );
                    }
                    self.check(
                        "valid_motor_pair",
                        motor != Some(3),
                        Value::Null,
                        Value::Null,
                    );
                }

                "status" => {
                    self.latest = event.as_object().cloned().unwrap_or_default();
                    if event["durable"] == true {
                        let inactive = event["motor"] == 0
                            && event["optics"] == 0
                            && event["stopped"] == true
                            && event["optics_off"] == true
                            && event["pending"] == false;
                        self.check(
                            "durable_result_after_inactive_outputs",
                            inactive,
                            Value::Null,
                            Value::Null,
                        );
                    }
                    // Applied after this batch, as a reboot on the same journal.
                    reset = event["watchdog_resets"].as_u64().unwrap_or(0) > 0;
                }

                "power_cut" => {
                    if !self.armed {
                        return fail("Unexpected worker power cut");
                    }
                    let worker = self.worker.as_mut().expect("a powered worker");
                    match worker.wait(RESPONSE) {
                        Ok(Some(75)) => {}
                        Ok(None) => return fail("Worker failed to exit after power cut"),
                        _ => return fail("Invalid power-cut worker exit"),
                    }
                    self.record(&json!({"event": "power_removed", "outputs": "unpowered"}));
                    self.offline();
                }
                _ => {}
            }
        }

        if reset && self.worker.is_some() {
            self.record(&json!({"event": "watchdog_reset_requested"}));
            self.offline();
            self.boot()?;
        }

        Ok(())
    }

    fn offline(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.close();
        }
        self.armed = false;
        self.reserved = false;
        self.phase = 0;
    }

    fn boot(&mut self) -> Result<()> {
        if self.worker.is_some() {
            return fail("boot requires the simulated power to be off");
        }

        self.boots += 1;
        self.last_submission = None;

        let mode = u8::from(self.profile() == "double");
        // Only the first boot provisions; later boots recover from the journal.
        let mut worker = Worker::start(&self.executable, &self.image, mode, self.boots == 1)?;
        // Keep the worker before checking its boot output, so `offline` stops it.
        let first = worker.read();
        self.worker = Some(worker);
        self.record(&json!({"event": "boot", "profile": self.profile()}));
        self.consume(first?)?;

        let period = self
            .scenario
            .get("feedback_period_ms")
            .and_then(Value::as_u64)
            .unwrap_or(10);
        self.send("auto", &[period])
    }

    fn send(&mut self, op: &str, args: &[u64]) -> Result<()> {
        let Some(worker) = self.worker.as_mut() else {
            return fail(format!("{op} requires simulated power"));
        };

        if op == "cut" || op == "power_cut" {
            self.armed = true;
        }

        let events = worker.command(op, args)?;
        self.consume(events)?;

        if op == "submit" {
            let response = self.latest.get("response").cloned().unwrap_or(Value::Null);
            if !integer(&response, 0, SUBMISSIONS.len() as u64 - 1) {
                return fail("Invalid submission response");
            }
            self.last_submission = Some(SUBMISSIONS[response.as_u64().unwrap() as usize]);
        }

        Ok(())
    }

    fn expect(&mut self, values: &Map<String, Value>) -> Result<()> {
        let mut actual = self.latest.clone();
        actual.insert("online".into(), json!(self.worker.is_some()));
        actual.insert("boots".into(), json!(self.boots));
        actual.insert("submission".into(), json!(self.last_submission));

        // The worker reports enum fields as indexes; scenarios expect names.
        for key in ["state", "fault", "outcome", "reason", "submission"] {
            let names = enum_names(key).unwrap();
            if let Some(index) = actual.get(key).and_then(Value::as_u64) {
                let Some(name) = names.get(index as usize) else {
                    return fail(format!("Invalid worker enum {key}"));
                };
                actual.insert(key.into(), json!(name));
            }
        }

        for (key, value) in values {
            if self.worker.is_none() && key != "online" && key != "boots" {
                return fail("Only online/boots can be asserted while power is off");
            }
            let got = actual.get(key).cloned().unwrap_or(Value::Null);
            self.check(&format!("expect.{key}"), got == *value, got, value.clone());
        }

        Ok(())
    }

    pub fn run(mut self) -> Result<Value> {
        // Power off before surfacing a step's error, so no worker outlives the run.
        let result = self.steps();
        self.offline();
        result?;

        let mut scenario_text = String::new();
        python_dumps(self.scenario, &mut scenario_text);
        let passed = self.assertions.iter().all(|a| a["passed"] == true);

        Ok(json!({
            "version": 1, "name": self.scenario["name"], "profile": self.profile(),
            "scenario_sha256": sha256_hex(scenario_text.as_bytes()),
            "identity": identities(&self.root, &self.executable),
            "enums": {"state": STATES, "fault": FAULTS, "outcome": OUTCOMES, "reason": REASONS,
                      "submission": SUBMISSIONS, "storage_phase": ["none", "reserve", "finish", "recover"],
                      "motor": ["stop", "drive_0", "drive_1", "invalid"]},
            "limits": {"synthetic_hardware_and_timing": true, "physical_reliability_verified": false,
                       "full_boot_or_rtos_emulated": false, "real_flash_provider": false,
                       "network_or_scheduler_implemented": false},
            "events": self.events, "assertions": self.assertions, "passed": passed,
        }))
    }

    fn steps(&mut self) -> Result<()> {
        self.boot()?;
        let actions = self.scenario["actions"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let mut next = actions.iter().peekable();
        let end = self.scenario["end_ms"].as_u64().unwrap_or(0);
        // Each millisecond: tick, apply that millisecond's actions, then poll.
        for now in 0..=end {
            self.now = now;
            if now > 0 && self.worker.is_some() {
                self.send("tick", &[])?;
            }

            while let Some(action) = next.next_if(|a| a["at_ms"].as_u64() == Some(now)) {
                self.record(&json!({"event": "action", "action": action}));
                let op = action["op"].as_str().unwrap_or("");
                match op {
                    "expect" => self.expect(action["values"].as_object().expect("validated"))?,
                    "boot" => self.boot()?,
                    _ => {
                        let args: Vec<u64> = action["args"]
                            .as_array()
                            .map_or(vec![], |a| a.iter().filter_map(Value::as_u64).collect());
                        self.send(op, &args)?;
                    }
                }
            }

            if self.worker.is_some() {
                self.send("poll", &[])?;
            }
        }

        Ok(())
    }
}

/// Run one scenario on a fresh journal under `root/private/verification`.
pub fn run_scenario(scenario: &Value, root: &Path, executable: &Path) -> Result<Value> {
    validate(scenario)?;
    let directory = root.join("private/verification");
    fs::create_dir_all(&directory).map_err(|e| HarnessError(e.to_string()))?;
    let scratch = tempfile::Builder::new()
        .prefix("virtual-feeder-")
        .tempdir_in(&directory)
        .map_err(|e| HarnessError(e.to_string()))?;
    Simulation::new(scenario, root, executable, scratch.path())?.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_dumps_matches_the_default_separators() {
        let mut out = String::new();
        python_dumps(&json!({"b": [1, true, null], "a": "é"}), &mut out);
        assert_eq!(out, "{\"a\": \"\\u00e9\", \"b\": [1, true, null]}");
    }
}
