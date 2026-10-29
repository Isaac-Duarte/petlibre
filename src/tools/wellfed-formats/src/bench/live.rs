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
    /// Every child process a job has running, as it starts and exits.
    pub activity: Vec<Activity>,
    pub uart: Uart,
    /// By channel, `"1"` to `"4"`: what the board last answered or
    /// acknowledged. The server holds the board, so nothing else switches
    /// a relay between reads, except the board's own front buttons.
    pub relays: BTreeMap<String, RelayState>,
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
    },
}

/// The flash client's subcommands that write flash.
const WRITES: [&str; 3] = [
    "update execute",
    "give-way execute",
    "stock-recovery execute",
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

        if flash.iter().any(|label| WRITES.contains(label)) {
            let (done, total) = record
                .and_then(|record| record.progress.as_ref())
                .filter(|progress| progress.unit == Unit::Sectors)
                .map_or((0, 0), |progress| (progress.done, progress.total));
            return Self::Writing { done, total };
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
            Uart::Writing { done: 3, total: 19 }
        );
        assert_eq!(
            Uart::of(None, &[flash("read observe"), flash("give-way execute")]),
            Uart::Writing { done: 0, total: 0 }
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
