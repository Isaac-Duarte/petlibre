//! Deterministic replacement-feeder scenarios on synthetic hardware; no
//! device or network access.
//!
//! The harness boots `virtual-feeder-worker` (the real owner on the fake SDK
//! with a file-backed journal, under the PLAF103 firmware's feeding limits,
//! against the measured mechanism in [`plant`]), steps it through a
//! scenario's actions millisecond by millisecond, batching the milliseconds
//! between actions into `run`s, and checks every event against the safety
//! invariants: a durable reservation before any drive, no invalid motor
//! pair, outputs inactive whenever a result is durable, and no flash lease
//! while the motor drives. Power cuts and watchdog resets restart the worker
//! on the same journal image, with the rotor where it stopped.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use wellfed_formats::canonical::{canonical, sha256};

pub mod plaf103;
pub mod plant;
pub mod protocol;
mod report;
pub mod scenario;
mod worker;

use protocol::{
    Bounded, Code, Command, Event, Fault, Journal, MOTORS, Outcome, Plant, RUN_MAX_MS, Reason,
    State, Status, StoragePhase, Submission,
};
pub use report::Report;
use report::{Assertion, Entry, Enums, Limits, Logged, Note, identities};
use scenario::{Action, Expectation, Scenario};
use worker::RESPONSE;
pub use worker::Worker;

/// The worker's exit status after it persists a power cut.
const POWER_CUT_EXIT: i32 = 75;

/// An invalid scenario, a worker failure, or a malformed protocol.
#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("Invalid scenario: {0}")]
    Scenario(serde_json::Error),
    #[error("Scenario cannot be hashed: {0}")]
    Hash(wellfed_formats::Error),
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

/// One scenario run: the worker for the current boot, the latest status, and
/// the harness's own view of storage that the invariants are checked against.
pub struct Simulation<'a> {
    raw: &'a Value,
    scenario: Scenario,
    root: PathBuf,
    executable: PathBuf,
    image: PathBuf,
    worker: Option<Worker>,

    /// The harness's clock: the scenario's milliseconds.
    now: u64,
    boots: u64,
    /// The harness's clock when this boot's worker started at its 0.
    base_ms: u64,
    /// The rotor's position, which outlives every worker.
    rotor_ms: i64,

    latest: Status,
    last_submission: Option<Submission>,

    events: Vec<Entry>,
    assertions: Vec<Assertion>,

    // `armed`: a `cut` or `power_cut` was sent, so a worker power cut is expected.
    // `reserved`: a reservation write ended ok and no finish write has ended since.
    // `stopped`: this boot has requested STOP. Startup clears the motor latches
    // while the pins are still inputs, so LOW/LOW before the first STOP is the
    // driver's standby, not an invalid request.
    // `driving` and `lease`: the motor drives; a store write holds the flash.
    armed: bool,
    reserved: bool,
    stopped: bool,
    driving: bool,
    outputs_off: bool,
    lease: bool,
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
            base_ms: 0,
            rotor_ms: 0,
            latest: Status::default(),
            last_submission: None,
            events: Vec::new(),
            assertions: Vec::new(),
            armed: false,
            reserved: false,
            stopped: false,
            driving: false,
            outputs_off: true,
            lease: false,
            phase: StoragePhase::None,
        })
    }

    fn record(&mut self, at_ms: u64, event: Logged) {
        self.events.push(Entry {
            at_ms,
            boot: self.boots,
            event,
        });
    }

    fn note(&mut self, note: Note) {
        self.record(self.now, Logged::Harness(note));
    }

    /// The worker's current millisecond on the harness's clock, while it
    /// runs.
    fn clock(&self) -> Option<u64> {
        self.worker
            .as_ref()
            .map(|_| self.base_ms + self.latest.device_ms)
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
            self.record(self.base_ms + event.device_ms(), Logged::Worker(event));
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

                Event::Outputs { motor, optics, .. } => {
                    self.driving = matches!(motor, 1 | 2);
                    self.outputs_off = motor == 0 && optics == 0;
                    if self.driving {
                        self.invariant("reservation_before_drive", self.reserved);
                        self.invariant("no_lease_while_driving", !self.lease);
                    }
                    self.stopped |= motor == 0;
                    self.invariant("valid_motor_pair", motor != 3 || !self.stopped);
                }

                Event::Lease { active, .. } => {
                    self.lease = active;
                    if active {
                        self.invariant("no_lease_while_driving", self.outputs_off);
                    }
                }

                Event::Status(status) | Event::Changed(status) => {
                    self.latest = status;
                    if status.durable {
                        let inactive = status.motor == 0
                            && status.optics == 0
                            && status.stopped
                            && status.optics_off
                            && !status.pending;
                        self.invariant("durable_result_after_inactive_outputs", inactive);
                    }
                    self.rotor_ms = status.rotor_ms;
                    // Applied after this batch, as a reboot on the same journal.
                    reset = status.watchdog_resets > 0;
                }

                Event::PowerCut { rotor_ms, .. } => {
                    self.rotor_ms = rotor_ms;
                    self.power_cut()?;
                }
                Event::Feedback { .. } | Event::JournalIo { .. } => {}
            }
        }

        if reset && let Some(clock) = self.clock() {
            self.now = clock;
            self.note(Note::WatchdogResetRequested);
            self.offline();
            self.boot()?;
        }

        Ok(())
    }

    fn power_cut(&mut self) -> Result<()> {
        if let Some(clock) = self.clock() {
            self.now = self.now.max(clock);
        }
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
        self.stopped = false;
        self.driving = false;
        self.outputs_off = true;
        self.lease = false;
        self.phase = StoragePhase::None;
    }

    fn boot(&mut self) -> Result<()> {
        require(
            self.worker.is_none(),
            "boot requires the simulated power to be off",
        )?;

        self.boots += 1;
        self.last_submission = None;
        self.base_ms = self.now;
        self.latest = Status::default();

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
        // The new worker's rotor is a default one until it is placed where
        // the last boot left it.
        let rotor = self.rotor_ms;
        self.consume(first?)?;
        self.rotor_ms = rotor;

        let plant = Plant(self.scenario.plant);
        self.send(&Command::Plant {
            args: (plant, rotor),
        })
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
            at_mark: status.at_mark,
            homed: status.homed,
            reason: status.reason,
            response: status.response,
            state: status.state,
            stopped: status.stopped,
            submission: submission,
            supervisor_fault: status.supervisor_fault,
            supply_low: status.supply_low,
            watchdog_fault: status.watchdog_fault,
            watchdog_resets: status.watchdog_resets,
            writes_done: status.writes_done,
            writes_waiting: status.writes_waiting,
        );

        Ok(())
    }

    pub fn run(mut self) -> Result<Report> {
        // Power off before surfacing a step's error, so no worker outlives the run.
        let result = self.steps();
        self.offline();
        result?;

        let scenario_text = canonical(self.raw).map_err(HarnessError::Hash)?;
        let passed = self.assertions.iter().all(|a| a.passed);

        Ok(Report {
            version: 2,
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
                synthetic_hardware: true,
                plant_from_bench_measurements: true,
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

    /// Bring the worker to millisecond `at`, ticked but not yet polled, for
    /// that millisecond's actions: every millisecond before it is ticked and
    /// polled. A worker already past it (a feed's claim waited for a write)
    /// stays where it is; time passes without one while the power is off.
    fn advance(&mut self, at: u64) -> Result<()> {
        loop {
            let Some(clock) = self.clock() else {
                self.now = self.now.max(at);
                return Ok(());
            };
            self.now = clock;
            if clock >= at {
                return Ok(());
            }
            if clock + 1 == at {
                self.send(&Command::TICK)?;
                continue;
            }

            let ms = (at - clock - 1).min(u64::from(RUN_MAX_MS));
            let ms = u32::try_from(ms).expect("at most RUN_MAX_MS");
            let boots = self.boots;
            self.send(&Command::Run {
                args: [Bounded(ms)],
            })?;
            // A watchdog reset restarted the worker on the reset's own
            // millisecond, which still needs its poll.
            if self.boots != boots && self.clock().is_some_and(|clock| clock < at) {
                self.send(&Command::POLL)?;
            }
        }
    }

    fn steps(&mut self) -> Result<()> {
        self.boot()?;

        let steps = std::mem::take(&mut self.scenario.steps);
        let mut next = steps.into_iter().peekable();

        // Each millisecond: tick, apply that millisecond's actions, then poll.
        while let Some(at) = next.peek().map(|step| step.at_ms) {
            self.advance(at)?;
            while let Some(step) = next.next_if(|s| s.at_ms == at) {
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

        let end = self.scenario.end_ms;
        if self.clock().is_some_and(|clock| clock < end) {
            self.advance(end)?;
            if self.worker.is_some() {
                self.send(&Command::POLL)?;
            }
        }
        self.now = self.clock().unwrap_or(self.now).max(end);
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
