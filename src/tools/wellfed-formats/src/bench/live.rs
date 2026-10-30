//! The bench server's live state, which `GET /state` answers and
//! `GET /events` pushes on every change, and the lines of a job's output
//! stream (`GET /jobs/{id}/stream`).
//!
//! It is held in memory only. The server writes nothing while a job runs
//! beyond what the job itself writes, so a UART write is never slowed by a
//! state file. The page and the clients read these shapes; the bench server
//! builds them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::banks::Ledger;
use super::notes::Operator;
use super::progress::{Current, RelayState, StepState, Unit};
use super::prompt::{Answer, Expects, Prompt};

/// A job's number, counting up across servers (see [`JOBS`]).
pub type JobId = u64;

/// Root-relative: each job's output, as `<id>-<name>.log`. Job numbers
/// carry on from the highest file here, so a new server never reuses one.
pub const JOBS: &str = "private/bench-status/jobs";

/// How many of a job's pending steps the state lists as coming next.
const UPCOMING: usize = 3;

/// Everything a reader shows of the bench right now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct BenchState {
    /// The bench host's clock when the state was read, so a reader's
    /// timers agree with it.
    pub now: f64,
    pub server: ServerView,
    /// The running job, or else the last one to finish.
    pub job: Option<JobView>,
    /// What the running job asks the person at the bench, until it is
    /// answered, cancelled or the job ends.
    pub prompt: Option<PendingPrompt>,
    /// Every child process a job has running, as it starts and exits.
    pub activity: Vec<Activity>,
    pub uart: Uart,
    /// By channel, `"1"` to `"4"`: what the board last answered or
    /// acknowledged. The server holds the board, so nothing else switches
    /// a relay between reads, except the board's own front buttons.
    pub relays: BTreeMap<String, RelayState>,
    /// The owner's device API session and what it last read.
    pub feeder: FeederView,
    /// The receive-only capture's newest lines and the records parsed
    /// from them.
    pub capture: CaptureView,
    /// Which build the bench believes each bank holds.
    pub banks: Ledger,
    /// What the operator's notes say now.
    pub operator: Operator,
}

/// The server process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ServerView {
    pub pid: u32,
    pub started: f64,
    pub phase: Phase,
    /// Whether the configured UART adapter's port exists, as the server
    /// last looked: at its start, after every job and with every idle
    /// relay read. The relay board's port is the server's own.
    pub uart_port: bool,
    /// A stop asked for during a UART write, held until the write's job
    /// ends; meanwhile the server takes no new job.
    pub stopping: Option<DeferredStop>,
}

/// A SIGTERM or hangup that came during a UART write: the server stops
/// once `job` has ended.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DeferredStop {
    /// The job that was writing.
    pub job: Option<JobId>,
    /// When the stop was asked for.
    pub since: f64,
}

/// Whether the server takes jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Serving,
    /// Quitting: the running job is cancelled and its cleanup awaited
    /// before the lease is released; no new job starts.
    Stopping,
}

/// One job: what was asked, how far it is, and how it ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct JobView {
    pub id: JobId,
    /// The command as the client asked for it, such as `dump unit-01`.
    pub label: String,
    /// Root-relative: the file its output is kept in.
    pub log: String,
    pub accepted: f64,
    /// The command's own record, from its first step on.
    pub record: Option<Current>,
    /// The labels of the next few steps still pending.
    pub upcoming: Vec<String>,
    pub cancel: CancelState,
    pub end: Option<JobEnd>,
}

impl JobView {
    /// Take a new record from the job, with the steps it says come next.
    pub fn update(&mut self, record: &Current) {
        self.upcoming = record
            .steps
            .iter()
            .filter(|step| step.state == StepState::Pending)
            .take(UPCOMING)
            .map(|step| step.label.clone())
            .collect();
        self.record = Some(record.clone());
    }
}

/// A question a job waits on: `wellfed install` asks it, and whoever
/// answers (the page, `wellfed-bench answer`, an agent) names
/// its `seq`. The number is the job's own count of its questions, from 1,
/// so an answer meant for one question never lands on the next.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PendingPrompt {
    pub job: JobId,
    pub seq: u32,
    pub prompt: Prompt,
    /// The prompt's words, as the installer's terminal shows them.
    pub text: String,
    pub expects: Expects,
}

impl PendingPrompt {
    /// The answers it takes, as `wellfed-bench answer` spells them.
    pub fn answer_words(&self) -> &'static str {
        match self.expects {
            Expects::Done => "done",
            Expects::YesNo => "yes|no",
        }
    }

    pub fn new(job: JobId, seq: u32, prompt: Prompt) -> Self {
        Self {
            job,
            seq,
            text: prompt.text(),
            expects: prompt.expects(),
            prompt,
        }
    }
}

/// `POST /jobs/{id}/answer`: the answer to the job's question `seq`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AnswerRequest {
    pub seq: u32,
    pub answer: Answer,
}

/// Why an answer was not passed to the job. Each is refused before
/// anything reaches it, and a refused answer is never held for later: the
/// installer reads answers in order, with no number of their own, so one
/// sent ahead of its question would answer that question unseen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "refused", rename_all = "kebab-case")]
pub enum AnswerError {
    /// No such job, or it has ended.
    #[error("Job {job} is not running")]
    NotRunning { job: JobId },
    /// Nothing is asked now: answered already, or not yet asked.
    #[error("Job {job} asks nothing now")]
    NoPrompt { job: JobId },
    #[error("Job {job} asks question {pending}, not {seq}")]
    WrongSeq { job: JobId, seq: u32, pending: u32 },
    /// A yes to an instruction, or done to a question.
    #[error("The {name} prompt takes {expects}, not {answer}")]
    Mismatched {
        name: String,
        expects: Expects,
        answer: Answer,
    },
}

/// Whether anyone asked the job to stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum CancelState {
    None,
    /// Asked: its children were signalled and its sequence is running its
    /// cleanup.
    Requested,
}

/// How a job ended: its exit code (0 complete, 1 stopped, 2 a suite's
/// silent feeder) and the error it stopped on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct JobEnd {
    pub code: u8,
    pub error: Option<String>,
    pub finished: f64,
}

/// One child process a job has running.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Activity {
    pub kind: ActivityKind,
    pub pid: u32,
    /// Its subcommand words, such as `update execute` or `serve`.
    pub label: String,
    pub started: f64,
}

/// What a child is, which decides how a cancel stops it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum ActivityKind {
    /// `wellfed-flash`, which a cancel only ever interrupts: SIGINT makes
    /// it stop its session terminally and record it.
    Flash,
    /// `wellfed-build`.
    Build,
    /// `wellfed-stock`: a server, an image server or a request.
    Stock,
    /// `wellfed install`, which stops its own stock server on SIGTERM, or
    /// `wellfed restore-stock`.
    Install,
    /// `sigrok-cli`.
    Analyzer,
}

/// What the feeder's UART is doing, for the banner that says not to touch
/// the bench during a write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Uart {
    Idle,
    /// A receive-only capture holds the port.
    Capturing {
        path: Option<String>,
    },
    /// The flash client has the ROM loader: a read, an inspection or a plan.
    Loader,
    /// The flash client is writing flash; `done` of `total` sectors are
    /// verified so far (both 0 until the first count).
    Writing {
        done: u64,
        total: u64,
        /// Whether this write reports a sector count at all: a give-way
        /// and a stock recovery print none, so theirs stays 0 of 0.
        counted: bool,
    },
}

/// The flash client's subcommands that write flash, and whether each
/// prints a sector count as it verifies.
const WRITES: [(&str, bool); 3] = [
    ("update execute", true),
    ("give-way execute", false),
    ("stock-recovery execute", false),
];
const OBSERVE: &str = "read observe";

impl Uart {
    /// The UART's state from the job's record and its running children:
    /// a writing flash client outranks a capture, which outranks a loader
    /// session.
    pub fn of(record: Option<&Current>, activity: &[Activity]) -> Self {
        let flash: Vec<&str> = activity
            .iter()
            .filter(|child| child.kind == ActivityKind::Flash)
            .map(|child| child.label.as_str())
            .collect();

        let write = WRITES.iter().find(|(write, _)| flash.contains(write));
        if let Some(&(_, counted)) = write {
            let (done, total) = record
                .and_then(|record| record.progress.as_ref())
                .filter(|progress| progress.unit == Unit::Sectors)
                .map_or((0, 0), |progress| (progress.done, progress.total));
            return Self::Writing {
                done,
                total,
                counted,
            };
        }
        if flash.contains(&OBSERVE) {
            let path = record
                .and_then(|record| record.capture.as_ref())
                .map(|capture| capture.path.clone());
            return Self::Capturing { path };
        }
        if flash.is_empty() {
            Self::Idle
        } else {
            Self::Loader
        }
    }
}

/// Root-relative: the feeder's debug log as the server's session follows
/// it, one file a UTC day (`<YYYY-MM-DD>.log`).
pub const DEVICE_LOG: &str = "private/bench-status/device-log";
/// Root-relative: the newest event the server's session has taken, which
/// a new server subscribes after.
pub const DEVICE_CURSOR: &str = "private/bench-status/device-cursor.json";

/// How many of the feeder's log lines and the capture's UART lines the
/// state keeps.
pub const TAIL: usize = 200;

/// The owner's device API session, as the server holds it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct FeederView {
    pub session: Session,
    /// The last status read, and when.
    pub status: Option<FeederStatus>,
    pub status_at: Option<f64>,
    pub clock: ClockView,
    /// The newest event the feeder has recorded, as its status or the
    /// last pushed event said.
    pub journal: Option<Journal>,
    /// Whether the bench hotspot's connection is up, read with nmcli;
    /// `None` without a `private/hotspot.json`, or when nmcli failed.
    pub hotspot: Option<bool>,
    /// The feeder's newest debug log lines.
    pub log: Vec<String>,
    /// Device API sessions open on the feeder besides the server's, at the
    /// last status: anything above 0 is a session the bench did not open
    /// through the server, which the page and the console warn of.
    pub other_sessions: u8,
}

/// Where the session stands.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Session {
    /// The server has not tried yet.
    #[default]
    Idle,
    /// No owner key file or server config to open one with.
    Unconfigured { error: String },
    /// Closed on purpose until the pause ends.
    Paused { pause: Pause },
    /// Opening, after `attempt` failures: the last one's error, and when
    /// the next try is due.
    Connecting {
        attempt: u32,
        error: Option<String>,
        retry: Option<f64>,
    },
    Open {
        address: String,
        build: String,
        since: f64,
    },
}

/// Why the server closed its session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Pause {
    /// A job that holds the UART runs: session traffic has no place beside
    /// a loader session or a write, and the feeder is about to lose power
    /// or its application anyway.
    UartJob { job: JobId, label: String },
    /// A job's install push holds a session of its own: the feeder's owner
    /// sessions are the push's until it ends.
    Install,
    /// `stock-ota` runs, with sessions of its own: the feeder runs stock,
    /// which serves no device API, until the build it installs answers.
    StockOta { job: JobId },
    /// `wellfed install` runs as job `job`, with sessions of its own: the
    /// feeder runs stock, and then the replacement it pairs with.
    StockInstall { job: JobId },
    /// Relay 1 is off: there is no feeder to talk to.
    PowerOff,
    /// The server is quitting.
    Stopping,
}

impl Pause {
    /// The job this pause is for when that job opens feeder sessions of
    /// its own. Only such a job may run a command that opens its own
    /// sessions beside the server; a UART job, a power-off or the server's
    /// own install push leaves no room for one.
    pub const fn own_sessions_job(&self) -> Option<JobId> {
        match self {
            Self::StockOta { job } | Self::StockInstall { job } => Some(*job),
            Self::UartJob { .. } | Self::Install | Self::PowerOff | Self::Stopping => None,
        }
    }
}

impl BenchState {
    /// The running job the feeder session is paused for, while that job
    /// opens sessions of its own.
    pub fn own_sessions_job(&self) -> Option<JobId> {
        let Session::Paused { pause } = &self.feeder.session else {
            return None;
        };
        let paused_for = pause.own_sessions_job()?;
        let running = self.job.as_ref().filter(|job| job.end.is_none())?;
        (running.id == paused_for).then_some(paused_for)
    }
}

impl std::fmt::Display for Pause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UartJob { job, label } => write!(f, "job {job} ({label}) holds the UART"),
            Self::Install => f.write_str("an install push holds the feeder's session"),
            Self::StockOta { job } => write!(f, "job {job} (stock-ota) holds its own sessions"),
            Self::StockInstall { job } => write!(f, "job {job} (install) holds its own sessions"),
            Self::PowerOff => f.write_str("relay 1 is off"),
            Self::Stopping => f.write_str("the bench server is stopping"),
        }
    }
}

/// Why the server's session cannot carry a request now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Unavailable {
    #[error("the bench server's feeder session is paused: {pause}")]
    Paused { pause: Pause },
    #[error("the bench server has no feeder session: {error}")]
    Unconfigured { error: String },
    /// It did not open within the time a request waits for it.
    #[error("the bench server's feeder session is not open{}", not_open(.error.as_deref()))]
    NotOpen { error: Option<String> },
    #[error("the bench server's feeder session has ended")]
    Ended,
    /// A request under this id is still out; `id` is hex.
    #[error("request {id} is already out on the bench server's feeder session")]
    DuplicateId { id: String },
}

fn not_open(error: Option<&str>) -> String {
    error.map(|error| format!(" ({error})")).unwrap_or_default()
}

/// The parts of the feeder's status the page shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct FeederStatus {
    /// The startup identity, lowercase hex.
    pub build: String,
    pub bank: u8,
    pub uptime_ms: u32,
    pub boot_count: u32,
    pub rssi: i32,
    /// Unix milliseconds; `None` while the clock is invalid.
    pub clock_ms: Option<u64>,
    pub locked: Option<bool>,
    pub last_event: Option<u64>,
    pub sessions: u8,
}

/// The feeder's clock against the bench host's.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ClockView {
    /// The feeder's clock minus the host's at the last status, in
    /// milliseconds; `None` while it is invalid or unread.
    pub skew_ms: Option<i64>,
    /// When the server last set it.
    pub synced_at: Option<f64>,
}

/// The newest event the feeder holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Journal {
    pub seq: u64,
    /// The history the number counts in, once an event has named it.
    pub epoch: Option<u32>,
    pub from: JournalSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum JournalSource {
    Status,
    Event,
}

/// [`DEVICE_CURSOR`]'s contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCursor {
    pub epoch: u32,
    pub seq: u64,
}

/// The receive-only capture the server holds while the feeder is idle and
/// powered.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CaptureView {
    /// Root-relative: the segment being written, while one is.
    pub segment: Option<String>,
    /// Why the capture stopped on its own, until it starts again.
    pub failure: Option<String>,
    /// The newest UART lines, from this capture or an earlier one.
    pub lines: Vec<String>,
    /// The last `PLFW1 start` seen.
    pub start: Option<StartRecord>,
    /// The last `PLOTA1` record seen.
    pub update: Option<UartRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct StartRecord {
    pub build: String,
    pub bank: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct UartRecord {
    pub line: String,
    pub at: f64,
}

/// Keep the newest [`TAIL`] of `lines`.
pub fn trim_tail(lines: &mut Vec<String>) {
    if lines.len() > TAIL {
        lines.drain(..lines.len() - TAIL);
    }
}

/// One line of a job's output stream, as newline-delimited JSON: each
/// output line as it comes, then one `end`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum JobEvent {
    Line(String),
    End(JobEnd),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::progress::{BenchCommand, Outcome, Progress, Step};

    fn record() -> Current {
        Current {
            pid: 1,
            command: BenchCommand::FlashExecute,
            name: Some("unit".into()),
            started: 1.0,
            finished: None,
            outcome: Outcome::Running,
            error: None,
            steps: ["one", "two", "three", "four", "five"]
                .iter()
                .enumerate()
                .map(|(n, label)| Step {
                    label: (*label).into(),
                    state: if n == 0 {
                        StepState::Running
                    } else {
                        StepState::Pending
                    },
                    started: None,
                    finished: None,
                })
                .collect(),
            progress: Some(Progress {
                done: 3,
                total: 19,
                unit: Unit::Sectors,
                since: 1.0,
                since_done: 0,
                at: 2.0,
            }),
            capture: None,
            transfer: None,
            relay: BTreeMap::new(),
            lines: Vec::new(),
            next: None,
            flash_tool: None,
        }
    }

    fn flash(label: &str) -> Activity {
        Activity {
            kind: ActivityKind::Flash,
            pid: 7,
            label: label.into(),
            started: 1.0,
        }
    }

    #[test]
    fn a_writing_flash_client_shows_its_sector_count() {
        let record = record();

        assert_eq!(
            Uart::of(Some(&record), &[flash("update execute")]),
            Uart::Writing {
                done: 3,
                total: 19,
                counted: true
            }
        );
        assert_eq!(
            Uart::of(None, &[flash("read observe"), flash("give-way execute")]),
            Uart::Writing {
                done: 0,
                total: 0,
                counted: false
            }
        );
    }

    #[test]
    fn captures_loader_sessions_and_idle() {
        let build = Activity {
            kind: ActivityKind::Build,
            label: "update execute".into(),
            ..flash("")
        };

        assert_eq!(
            Uart::of(None, &[flash("read observe")]),
            Uart::Capturing { path: None }
        );
        assert_eq!(Uart::of(None, &[flash("read full")]), Uart::Loader);
        // Only the flash client touches the UART.
        assert_eq!(Uart::of(None, &[build]), Uart::Idle);
    }

    #[test]
    fn a_job_lists_its_next_pending_steps() {
        let mut job = JobView {
            id: 1,
            label: "flash-execute unit".into(),
            log: "private/bench-status/jobs/1-unit.log".into(),
            accepted: 0.0,
            record: None,
            upcoming: Vec::new(),
            cancel: CancelState::None,
            end: None,
        };

        job.update(&record());

        assert_eq!(job.upcoming, ["two", "three", "four"]);
        assert!(job.record.is_some());
    }

    #[test]
    fn stream_lines_are_tagged_by_kind() {
        let line = serde_json::to_string(&JobEvent::Line("Read 1/2 bytes".into())).unwrap();
        let end = JobEvent::End(JobEnd {
            code: 1,
            error: Some("Interrupted by the operator".into()),
            finished: 2.0,
        });

        assert_eq!(line, r#"{"line":"Read 1/2 bytes"}"#);
        let text = serde_json::to_string(&end).unwrap();
        assert!(text.starts_with(r#"{"end":{"code":1,"#), "{text}");
        assert_eq!(serde_json::from_str::<JobEvent>(&text).unwrap(), end);
    }
}
