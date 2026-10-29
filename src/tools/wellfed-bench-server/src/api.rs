//! What crosses the socket besides the live state and the job stream
//! (`wellfed_formats::bench::live`) and the device routes
//! (`wellfed_formats::bench::feeder`): a new job's command line, the
//! answer to it, a ledger seed, and the refusals, which a client matches
//! by variant.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use wellfed_formats::bench::banks::Ledger;
use wellfed_formats::bench::live::{JobEnd, JobId};

/// `POST /banks/seed`: the ledger's starting point for one bank, from a
/// dump session's inspection when it names one, else the operator's own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seed {
    pub session: String,
    pub bank: Option<u8>,
    pub build: Option<String>,
}

/// What a seed recorded, and from whose word.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Seeded {
    pub bank: u8,
    pub build: String,
    pub from: SeededFrom,
    pub ledger: Ledger,
}

/// Root-relative paths: the inspection report a seed looked in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SeededFrom {
    Inspection {
        inspection: String,
    },
    /// The inspection named no build, so `--bank` and `--build` did.
    Operator {
        inspection: String,
    },
}

/// Why a seed was refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SeedError {
    /// The inspection names no build, and neither did the operator.
    #[error("{inspection} names no bank and build; give them with --bank N --build HASH")]
    NoBuild { inspection: String },
    #[error("Bank {bank} is no bank; the feeder has 0 and 1")]
    NoBank { bank: u8 },
    #[error("Session name {name:?} must be a single plain name")]
    BadName { name: String },
}

/// Root-relative: the control socket, mode 0600. Only the owner of the
/// checkout can reach it, so it carries no token.
pub const SOCKET: &str = "private/bench.sock";

/// `POST /jobs`: a job as the words of its command line, such as
/// `["dump", "unit-01"]`, without `--root`, and the client's checkout. The
/// server parses the words as the CLI does, so every client is held to the
/// same checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRequest {
    /// Canonical. A checkout whose `private/` is a link to another's
    /// reaches that one's server; the server refuses it, rather than run
    /// the other checkout's sources under this one's session names.
    pub root: PathBuf,
    pub argv: Vec<String>,
}

/// `POST /jobs/{id}/cancel`: whether a cancel may stop a UART write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CancelRequest {
    pub during_write: DuringWrite,
}

/// What a cancel does to a job that is writing flash over UART. A
/// cancelled write stops terminally and leaves its bank half-written until
/// another write, so it is asked for on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DuringWrite {
    /// Refused while the job writes; any other job is cancelled.
    #[default]
    Refuse,
    /// Cancelled, write and all.
    Cancel,
}

/// `POST /jobs` took the job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Accepted {
    pub id: JobId,
}

/// Why the server did not do what a client asked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "kebab-case")]
pub enum Refusal {
    /// One job runs at a time, and a second is refused, never queued: a
    /// queued job would run on a bench nobody looked at since.
    #[error("The bench is busy with job {id} ({label}); wait for it, or cancel it first")]
    Busy { id: JobId, label: String },
    #[error("The bench server is stopping")]
    Stopping,
    /// A stop came during a UART write and waits for its job: nothing new
    /// starts on a bench about to be released.
    #[error("The bench server stops once {}; it takes no new job", ended(*.job))]
    StoppingAfterWrite { job: Option<JobId> },
    /// A cancel would stop the write terminally; [`DuringWrite::Cancel`]
    /// asks for that.
    #[error(
        "Job {id} is writing flash over UART: a cancel stops the write for good and leaves the bank half-written until another UART write; ask again to mean it"
    )]
    CancelDuringWrite { id: JobId },
    /// The server serves another checkout, whose `private/` this one's
    /// links to.
    #[error("The bench server runs for {server}, not {client}; run the command from that checkout")]
    OtherCheckout { server: String, client: String },
    #[error("No job {id} on this server")]
    NoSuchJob { id: JobId },
    #[error("Job {id} has already finished")]
    Finished { id: JobId },
    /// The words do not parse as a job; `message` is clap's, with usage.
    #[error("{message}")]
    Unparsed { message: String },
    /// A command that parses but that the client answers itself.
    #[error("`{what}` is answered by the client and is no job")]
    NotAJob { what: String },
    #[error("Session name {name:?} must be a single plain name")]
    BadName { name: String },
    #[error("--off-seconds {value} must be a finite, non-negative number, at most a week")]
    BadLength { value: f64 },
    #[error("{error}")]
    Seed { error: SeedError },
    #[error("{error}")]
    Note { error: NoteError },
    #[error("{error}")]
    Wait { error: WaitError },
}

/// What a deferred stop waits for, in words.
fn ended(job: Option<JobId>) -> String {
    match job {
        Some(job) => format!("job {job} has ended"),
        None => "the UART write has ended".into(),
    }
}

/// Why a note was refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "refused", rename_all = "kebab-case")]
pub enum NoteError {
    /// The bench keeps its evidence in `private/`, which no tracked file
    /// may name.
    #[error("Evidence path {path:?} is not inside private/")]
    OutsidePrivate { path: String },
    #[error("A timer needs a label")]
    NoLabel,
    #[error("Timer {label:?}: its window must end at or after its due time")]
    Window { label: String },
    #[error("Timer {label:?}: an empty expectation would match every line")]
    EmptyExpect { label: String },
    #[error("{kind:?} is no device event kind; the device API has {known}")]
    UnknownEvent { kind: String, known: String },
    /// Only the server settles a timer, as it sees the match or the miss.
    #[error("A `{kind}` note is the bench server's own")]
    ServerOnly { kind: String },
    /// The disk refused the note. The error is its text, not the
    /// `io::Error`: a refusal crosses the socket as JSON, which an
    /// `io::Error` has no form in.
    #[error("{path}: {error}")]
    Write { path: String, error: String },
}

/// `GET /wait`: what to wait for, as query parameters. `what` picks the
/// condition; the fields it names must be given, and no others matter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wait {
    #[serde(rename = "for")]
    pub what: WaitFor,
    /// [`WaitFor::Job`]: the job whose end to wait for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<JobId>,
    /// [`WaitFor::Line`]: text a UART, device log or job output line
    /// contains, such as `PLFW1 start`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// [`WaitFor::Event`]: a device event kind, as the device API names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// [`WaitFor::Status`]: the start of the build's startup identity, hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    /// [`WaitFor::Status`]: the bank it runs from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bank: Option<u8>,
    /// [`WaitFor::Timer`]: the operator's timer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// How long to wait, in milliseconds, at most [`WAIT_MAX_MS`].
    pub timeout_ms: u64,
}

/// The longest one `GET /wait` holds its connection.
pub const WAIT_MAX_MS: u64 = 600_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WaitFor {
    /// The job ended (or a later one started, so it had).
    Job,
    /// A line containing `text` arrived after the wait began.
    Line,
    /// A device event of `kind` arrived after the wait began.
    Event,
    /// A status read after the wait began names `build` (a prefix) and
    /// `bank`.
    Status,
    /// The operator's timer is due, seen or missed.
    Timer,
    /// No job runs and the flash client holds no UART.
    Idle,
}

/// What came of a wait.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum Waited {
    /// The condition held; `detail` says how (the line, the job's end).
    Met {
        detail: String,
    },
    TimedOut {
        waited_ms: u64,
    },
}

/// Why a wait was refused before it began.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "refused", rename_all = "kebab-case")]
pub enum WaitError {
    #[error("Waiting for {what} needs --{field}")]
    Missing { what: String, field: String },
    #[error("A wait lasts at most {max_ms} ms, not {asked_ms}")]
    TooLong { asked_ms: u64, max_ms: u64 },
    #[error("A line wait needs text to look for")]
    EmptyText,
    #[error("{kind:?} is no device event kind")]
    UnknownEvent { kind: String },
    /// A wait for it would only ever time out.
    #[error("No timer {label:?} is set; `note timer` sets one")]
    UnknownTimer { label: String },
    #[error("No job {id} yet: the next job is {next}")]
    NoSuchJob { id: JobId, next: JobId },
}

/// `GET /jobs/{id}/output?after=N`: a page of a job's kept output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputPage {
    /// The line number after which these follow; lines dropped from the
    /// front of a long job's output move it on.
    pub from: u64,
    pub lines: Vec<String>,
    /// Where the next page starts.
    pub next: u64,
    /// The job's end, once every line has been read.
    pub end: Option<JobEnd>,
}

/// The longest page `GET /jobs/{id}/output` answers.
pub const OUTPUT_PAGE: usize = 500;

/// A device API request a client that may not change the feeder can make:
/// reads, and setting the clock as the session itself would. Never a feed,
/// a cancel, a plan, a setting or an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ReadOnly {
    Status,
    Settings,
    Time,
    Plans,
    Clients,
    UpdateState,
    Calibration,
    Call,
    Stats {
        section: wellfed_model::StatsSection,
    },
    /// This host's clock and zone, as `wellfed time` sets them.
    ClockSync,
}

impl ReadOnly {
    /// The request, as of `now` for a clock sync.
    pub fn op(self, now: jiff::Timestamp) -> wellfed_model::Op {
        use wellfed_model::Op;

        match self {
            Self::Status => Op::Status,
            Self::Settings => Op::GetSettings,
            Self::Time => Op::GetTime,
            Self::Plans => Op::GetPlans,
            Self::Clients => Op::ListClients,
            Self::UpdateState => Op::UpdateState,
            Self::Calibration => Op::GetCalibration,
            Self::Call => Op::GetCall,
            Self::Stats { section } => Op::Stats { section },
            Self::ClockSync => Op::TimeSync(wellfed_client::time::sync(
                now,
                &jiff::tz::TimeZone::system(),
            )),
        }
    }
}
