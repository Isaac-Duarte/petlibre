//! A bench command's progress: its steps, the counts its tools print, its
//! relay changes and its active capture.
//!
//! [`Status`] keeps the running command's record ([`Current`]) in memory and
//! hands each change to a [`ProgressSink`]. The bench server's sink
//! publishes it as live state, which the page and the clients follow, and
//! keeps each job's output in a file of its own. `private/bench-status/`
//! keeps the history, `history.jsonl`, one line per command.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use super::{now, relative};
use crate::Result;

pub const HISTORY: &str = "history.jsonl";
const KEPT_LINES: usize = 40;

/// How a command went, as its record and the history keep it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    #[default]
    Running,
    Pass,
    Stopped,
    /// Never written: the page's reading of a `running` record whose
    /// process exited without finishing it.
    Ended,
}

/// The bench command a progress record belongs to, by its CLI name. A
/// history line naming a command this bench no longer has does not read,
/// and the page skips it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "kebab-case")]
pub enum BenchCommand {
    Status,
    Off,
    On,
    Capture,
    Boot,
    Loader,
    Transients,
    Dump,
    FlashPrepare,
    FlashExecute,
    GiveWayPrepare,
    GiveWayExecute,
    StockPrepare,
    StockExecute,
    Ota,
    Itest,
    StockOta,
}

impl BenchCommand {
    /// The command's CLI name, as the history records it.
    pub fn word(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Off => "off",
            Self::On => "on",
            Self::Capture => "capture",
            Self::Boot => "boot",
            Self::Loader => "loader",
            Self::Transients => "transients",
            Self::Dump => "dump",
            Self::FlashPrepare => "flash-prepare",
            Self::FlashExecute => "flash-execute",
            Self::GiveWayPrepare => "give-way-prepare",
            Self::GiveWayExecute => "give-way-execute",
            Self::StockPrepare => "stock-prepare",
            Self::StockExecute => "stock-execute",
            Self::Ota => "ota",
            Self::Itest => "itest",
            Self::StockOta => "stock-ota",
        }
    }
}

/// What a progress count counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    Bytes,
    Sectors,
}

/// Where one of a command's steps stands; the page colours its rows by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum StepState {
    Pending,
    Running,
    Done,
    Failed,
    Skipped,
}

impl StepState {
    fn unfinished(self) -> bool {
        matches!(self, Self::Pending | Self::Running)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Current {
    pub pid: u32,
    pub command: BenchCommand,
    pub name: Option<String>,
    pub started: f64,
    pub finished: Option<f64>,
    pub outcome: Outcome,
    pub error: Option<String>,

    pub steps: Vec<Step>,
    pub progress: Option<Progress>,
    pub capture: Option<Capture>,
    pub transfer: Option<Transfer>,

    pub relay: BTreeMap<String, bool>,
    pub lines: Vec<String>,

    /// The command to run after review, such as a prepared plan's execute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flash_tool: Option<FlashTool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Step {
    pub label: String,
    pub state: StepState,
    /// When it began running and when it finished, for the page's timers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished: Option<f64>,
}

impl Step {
    fn pending(label: &str) -> Self {
        Self {
            label: label.into(),
            state: StepState::Pending,
            started: None,
            finished: None,
        }
    }

    /// Move to `state`, stamping the start or finish it implies.
    fn set(&mut self, state: StepState) {
        let at = now();
        match state {
            StepState::Pending => {}
            StepState::Running => self.started = Some(at),
            StepState::Done | StepState::Failed | StepState::Skipped => {
                self.finished = Some(at);
            }
        }
        self.state = state;
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Progress {
    pub done: u64,
    pub total: u64,
    pub unit: Unit,
    /// Rate baseline: when and at which count this count series began.
    pub since: f64,
    pub since_done: u64,
    pub at: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Capture {
    /// Root-relative capture directory.
    pub path: String,
    pub seconds: f64,
    pub started: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Transfer {
    /// Root-relative session directory.
    pub path: String,
    pub tx: u64,
    pub rx: u64,
    pub started: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct FlashTool {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HistoryEntry {
    pub command: BenchCommand,
    pub name: Option<String>,
    pub started: f64,
    pub finished: Option<f64>,
    pub outcome: Outcome,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

/// A relay's state as the board last acknowledged or reported it, and when.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RelayState {
    pub on: bool,
    pub at: f64,
}

/// When a change reaches the sink: at once, for a change the page must show
/// (a step, an outcome), or at most every 0.2 s, for the stream of tool
/// output lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flush {
    Now,
    Throttled,
}

/// Where a command's record goes each time it changes. It is called on the
/// command's own thread, between its steps, so it must return promptly and
/// never wait on the command.
pub trait ProgressSink: Send {
    fn publish(&mut self, current: &Current);
}

/// What this process did. Disabled (no directory, no sink) in tests.
pub struct Status {
    directory: Option<PathBuf>,
    root: PathBuf,
    state: Option<Current>,
    written: Option<Instant>,
    sink: Option<Box<dyn ProgressSink>>,
    /// The steps are the command's own (the suite's scenarios); a child's
    /// step, such as a build inside one, does not add to them.
    fixed: bool,
}

impl Status {
    pub fn disabled() -> Self {
        Self {
            directory: None,
            root: PathBuf::new(),
            state: None,
            written: None,
            sink: None,
            fixed: false,
        }
    }

    /// Keeps the history in `directory`. The record itself goes
    /// only to a sink given with [`Self::with_sink`].
    pub fn new(root: &Path, directory: PathBuf) -> Self {
        Self {
            directory: Some(directory),
            root: root.to_path_buf(),
            ..Self::disabled()
        }
    }

    pub fn with_sink(mut self, sink: Box<dyn ProgressSink>) -> Self {
        self.sink = Some(sink);
        self
    }

    pub fn begin(
        &mut self,
        command: BenchCommand,
        name: Option<&str>,
        steps: &[&str],
    ) -> Result<()> {
        self.state = Some(Current {
            pid: std::process::id(),
            command,
            name: name.map(Into::into),
            started: now(),
            finished: None,
            outcome: Outcome::Running,
            error: None,
            steps: steps.iter().map(|label| Step::pending(label)).collect(),
            progress: None,
            capture: None,
            transfer: None,
            relay: BTreeMap::new(),
            lines: Vec::new(),
            next: None,
            flash_tool: None,
        });

        if let Some(directory) = &self.directory {
            fs::create_dir_all(directory)?;
        }
        self.write(Flush::Now)
    }

    /// Mark `label` running and any running step done.
    pub fn step(&mut self, label: &str) {
        let Some(state) = &mut self.state else { return };
        if self.fixed {
            state.progress = None;
            self.write_quietly(Flush::Now);
            return;
        }
        for s in &mut state.steps {
            if s.state == StepState::Running {
                s.set(StepState::Done);
            }
        }

        let index = match state
            .steps
            .iter()
            .position(|s| s.label == label && s.state == StepState::Pending)
        {
            Some(index) => index,
            None => {
                state.steps.push(Step::pending(label));
                state.steps.len() - 1
            }
        };
        state.steps[index].set(StepState::Running);
        state.progress = None;
        self.write_quietly(Flush::Now);
    }

    /// Keep the steps [`Self::begin`] gave: children's steps no longer add
    /// rows, and the command marks its own with [`Self::mark`].
    pub fn fix_steps(&mut self) {
        self.fixed = true;
    }

    /// Set the first unfinished step named `label` to `state`.
    pub fn mark(&mut self, label: &str, state: StepState) {
        let Some(current) = &mut self.state else {
            return;
        };
        if let Some(step) = current
            .steps
            .iter_mut()
            .find(|s| s.label == label && s.state.unfinished())
        {
            step.set(state);
        }
        self.write_quietly(Flush::Now);
    }

    /// One line of tool output: kept and scanned for progress counts.
    pub fn line(&mut self, text: &str) {
        let Some(state) = &mut self.state else { return };
        let text = text.trim_end();
        state.lines.push(text.into());
        let excess = state.lines.len().saturating_sub(KEPT_LINES);
        state.lines.drain(..excess);

        // A sector count comes at most once per sector and drives the
        // write banner, so it is published at once: a throttled one is
        // dropped, and a writer that goes quiet after it would leave the
        // count behind. Byte counts come fast and stay throttled.
        let mut when = Flush::Throttled;

        if let Some((done, total, unit)) = progress_count(text) {
            let at = now();
            // The first report of a count sets the rate baseline for the ETA.
            let (since, since_done) = match &state.progress {
                Some(p) if p.unit == unit && p.total == total && done >= p.done => {
                    (p.since, p.since_done)
                }
                _ => (at, done),
            };
            state.progress = Some(Progress {
                done,
                total,
                unit,
                since,
                since_done,
                at,
            });
            if unit == Unit::Sectors {
                when = Flush::Now;
            }
        }
        self.write_quietly(when);
    }

    pub fn capture(&mut self, output: Option<&Path>, seconds: f64) {
        let root = self.root.clone();
        let Some(state) = &mut self.state else { return };
        state.capture = output.map(|path| Capture {
            path: relative(&root, path),
            seconds,
            started: now(),
        });
        self.write_quietly(Flush::Now);
    }

    /// Expected serial traffic of an update session, for an estimated bar.
    pub fn transfer(&mut self, directory: Option<&Path>, tx: u64, rx: u64) {
        let root = self.root.clone();
        let Some(state) = &mut self.state else { return };
        state.transfer = directory.map(|path| Transfer {
            path: relative(&root, path),
            tx,
            rx,
            started: Some(now()),
        });
        self.write_quietly(Flush::Now);
    }

    /// A relay this command set or read. The server's live state carries the
    /// board's own view of all four.
    pub fn relay(&mut self, channel: u8, on: bool) {
        if let Some(state) = &mut self.state {
            state.relay.insert(channel.to_string(), on);
        }
        self.write_quietly(Flush::Now);
    }

    /// The record as last written; `None` before [`Self::begin`].
    pub fn state(&self) -> Option<&Current> {
        self.state.as_ref()
    }

    pub fn flash_tool(&mut self, tool: FlashTool) {
        if let Some(state) = &mut self.state {
            state.flash_tool = Some(tool);
        }
        self.write_quietly(Flush::Now);
    }

    pub fn next(&mut self, line: String) {
        if let Some(state) = &mut self.state {
            state.next = Some(line);
        }
        self.write_quietly(Flush::Now);
    }

    pub fn finish(&mut self, code: u8, error: Option<String>) {
        let Some(state) = &mut self.state else { return };
        let pass = code == 0;
        for s in &mut state.steps {
            if s.state == StepState::Running {
                s.set(if pass {
                    StepState::Done
                } else {
                    StepState::Failed
                });
            }
        }

        state.finished = Some(now());
        state.outcome = if pass {
            Outcome::Pass
        } else {
            Outcome::Stopped
        };
        state.error = error;
        state.capture = None;
        state.transfer = None;

        let entry = HistoryEntry {
            command: state.command,
            name: state.name.clone(),
            started: state.started,
            finished: state.finished,
            outcome: state.outcome,
            error: state.error.clone(),
            next: state.next.clone(),
        };
        self.write_quietly(Flush::Now);

        if let Some(directory) = &self.directory
            && let Err(error) = append_history(directory, &entry)
        {
            eprintln!("Could not record bench history: {error}");
        }
    }

    /// Progress is advisory: a failed status write never stops a session.
    fn write_quietly(&mut self, when: Flush) {
        if let Err(error) = self.write(when) {
            eprintln!("Could not record bench progress: {error}");
        }
    }

    pub fn write(&mut self, when: Flush) -> Result<()> {
        let (Some(sink), Some(state)) = (&mut self.sink, &self.state) else {
            return Ok(());
        };
        if when == Flush::Throttled
            && self
                .written
                .is_some_and(|at| at.elapsed().as_secs_f64() < 0.2)
        {
            return Ok(());
        }

        self.written = Some(Instant::now());
        sink.publish(state);
        Ok(())
    }
}

fn append_history(directory: &Path, entry: &HistoryEntry) -> Result<()> {
    let line = serde_json::to_string(entry)?;
    let mut stream = OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join(HISTORY))?;
    writeln!(stream, "{line}")?;
    Ok(())
}

/// `Read N/M bytes`, `Verified changed sectors: N/M` or the over-the-air
/// loop's `Written sectors: N/M` in a tool's output.
fn progress_count(text: &str) -> Option<(u64, u64, Unit)> {
    fn pair(text: &str) -> Option<(u64, u64)> {
        let (done, rest) = text.split_once('/')?;
        let digits = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        Some((done.trim().parse().ok()?, rest[..digits].parse().ok()?))
    }

    if let Some(at) = text.find("Read ") {
        let rest = &text[at + 5..];
        let (counts, tail) = rest.split_once(' ')?;
        if tail.starts_with("bytes") {
            return pair(counts).map(|(d, t)| (d, t, Unit::Bytes));
        }
    }
    for prefix in ["Verified changed sectors: ", "Written sectors: "] {
        if let Some(at) = text.find(prefix) {
            return pair(&text[at + prefix.len()..]).map(|(d, t)| (d, t, Unit::Sectors));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keeps every record it is given, as JSON, for the test to read back.
    #[derive(Clone, Default)]
    struct Recorded(std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>);

    impl ProgressSink for Recorded {
        fn publish(&mut self, current: &Current) {
            self.0
                .lock()
                .unwrap()
                .push(serde_json::to_value(current).unwrap());
        }
    }

    impl Recorded {
        fn last(&self) -> serde_json::Value {
            self.0.lock().unwrap().last().cloned().unwrap()
        }
    }

    #[test]
    fn records_steps_progress_relays_and_history() {
        let tmp = tempfile::tempdir().unwrap();
        let sink = Recorded::default();
        let mut status =
            Status::new(tmp.path(), tmp.path().to_path_buf()).with_sink(Box::new(sink.clone()));
        status
            .begin(
                BenchCommand::Dump,
                Some("unit"),
                &["loader entry", "wellfed-flash read full"],
            )
            .unwrap();

        status.relay(1, true);
        status.step("loader entry");
        status.step("wellfed-flash read full");
        status.line("Read 4096/8388608 bytes\n");
        status.write(Flush::Now).unwrap();

        let current = sink.last();
        assert_eq!(current["steps"][0]["state"], "done");
        assert_eq!(current["steps"][1]["state"], "running");
        assert_eq!(current["outcome"], "running");
        assert_eq!(current["progress"]["done"], 4096);
        assert_eq!(current["progress"]["unit"], "bytes");
        assert_eq!(current["relay"], serde_json::json!({"1": true}));
        // The record lives only in the sink.
        assert!(!tmp.path().join("current.json").exists());

        let first = status.state.as_ref().unwrap().progress.clone().unwrap();
        status.line("Read 8192/8388608 bytes");
        let later = status.state.as_ref().unwrap().progress.clone().unwrap();
        // The rate baseline stays at the first report of this count.
        assert_eq!(
            (later.done, later.since, later.since_done),
            (8192, first.since, 4096)
        );

        status.finish(1, Some("Observer failed".into()));
        let current = sink.last();
        assert_eq!(current["outcome"], "stopped");
        assert_eq!(current["steps"][1]["state"], "failed");

        let history = fs::read_to_string(tmp.path().join(HISTORY)).unwrap();
        let entry: HistoryEntry = serde_json::from_str(history.trim()).unwrap();
        assert_eq!(
            (entry.command, entry.outcome),
            (BenchCommand::Dump, Outcome::Stopped)
        );
        assert_eq!(entry.error.as_deref(), Some("Observer failed"));
    }

    #[test]
    fn a_sector_count_is_published_at_once_and_a_byte_count_is_throttled() {
        let tmp = tempfile::tempdir().unwrap();
        let sink = Recorded::default();
        let mut status =
            Status::new(tmp.path(), tmp.path().to_path_buf()).with_sink(Box::new(sink.clone()));
        status
            .begin(BenchCommand::FlashExecute, Some("unit"), &[])
            .unwrap();

        status.line("Loader answered");
        status.line("Read 4096/8388608 bytes");
        assert!(sink.last()["progress"].is_null());

        status.line("Verified changed sectors: 3/19");
        assert_eq!(sink.last()["progress"]["done"], 3);
        assert_eq!(sink.last()["progress"]["unit"], "sectors");
    }

    #[test]
    fn disabled_status_writes_nothing() {
        let mut status = Status::disabled();
        status.begin(BenchCommand::Boot, Some("unit"), &[]).unwrap();
        status.line("Read 1/2 bytes");
        status.finish(0, None);

        assert_eq!(status.state.unwrap().outcome, Outcome::Pass);
    }

    #[test]
    fn progress_counts_parse_both_tools_forms() {
        assert_eq!(
            progress_count("Read 4096/8388608 bytes"),
            Some((4096, 8388608, Unit::Bytes))
        );
        assert_eq!(
            progress_count("Verified changed sectors: 3/19"),
            Some((3, 19, Unit::Sectors))
        );
        assert_eq!(
            progress_count("Written sectors: 16/164"),
            Some((16, 164, Unit::Sectors))
        );
        assert_eq!(progress_count("Read the ROM"), None);
        assert_eq!(progress_count("Region acquisition 1 complete"), None);
    }

    #[test]
    fn fixed_steps_ignore_child_steps_and_take_marks() {
        let tmp = tempfile::tempdir().unwrap();
        let mut status = Status::new(tmp.path(), tmp.path().join("status"));
        status
            .begin(
                BenchCommand::Itest,
                Some("run1"),
                &["ota-update", "requests"],
            )
            .unwrap();
        status.fix_steps();

        status.mark("ota-update", StepState::Running);
        status.step("firmware build");
        status.mark("ota-update", StepState::Done);
        status.mark("requests", StepState::Failed);

        let steps: Vec<_> = status
            .state
            .as_ref()
            .unwrap()
            .steps
            .iter()
            .map(|s| (s.label.as_str(), s.state))
            .collect();
        assert_eq!(
            steps,
            [
                ("ota-update", StepState::Done),
                ("requests", StepState::Failed)
            ]
        );
    }

    #[test]
    fn a_command_s_word_is_its_history_name() {
        for command in [
            BenchCommand::Status,
            BenchCommand::Capture,
            BenchCommand::FlashExecute,
            BenchCommand::GiveWayPrepare,
            BenchCommand::StockOta,
        ] {
            let named = serde_json::to_value(command).unwrap();
            assert_eq!(named, command.word(), "{command:?}");
        }
    }
}
