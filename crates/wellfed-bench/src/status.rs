//! Progress for the `--ui` page, in `private/bench-status/`:
//! - `current.json`: rewritten atomically, at most every 0.2 s while output streams
//! - `current.log`: the full output of the running command
//! - `history.jsonl`: one line per command
//! - `relays.json`: the last state each relay was set to or read as
//!
//! The JSON fields are the ones `bench.py` wrote, so older history still
//! reads. The page only reads these files; it never queries or drives the bench.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::{Result, now};

pub const CURRENT: &str = "current.json";
pub const LOG: &str = "current.log";
pub const HISTORY: &str = "history.jsonl";
pub const RELAYS: &str = "relays.json";
pub const SCRIPT: &str = "script.json";
const KEPT_LINES: usize = 40;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Current {
    #[serde(default)]
    pub pid: u32,
    pub command: String,
    #[serde(default)]
    pub name: Option<String>,
    pub started: f64,
    #[serde(default)]
    pub finished: Option<f64>,
    pub outcome: String,
    #[serde(default)]
    pub error: Option<String>,

    #[serde(default)]
    pub steps: Vec<Step>,
    #[serde(default)]
    pub progress: Option<Progress>,
    #[serde(default)]
    pub capture: Option<Capture>,
    #[serde(default)]
    pub transfer: Option<Transfer>,

    #[serde(default)]
    pub relay: BTreeMap<String, bool>,
    #[serde(default)]
    pub lines: Vec<String>,

    /// The command to run after review, such as a prepared plan's execute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flash_tool: Option<FlashTool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub label: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
    pub unit: String,
    /// Rate baseline: when and at which count this count series began.
    pub since: f64,
    pub since_done: u64,
    pub at: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capture {
    /// Root-relative capture directory.
    pub path: String,
    pub seconds: f64,
    pub started: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transfer {
    /// Root-relative session directory.
    pub path: String,
    pub tx: u64,
    pub rx: u64,
    #[serde(default)]
    pub started: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlashTool {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub command: String,
    #[serde(default)]
    pub name: Option<String>,
    pub started: f64,
    #[serde(default)]
    pub finished: Option<f64>,
    pub outcome: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayState {
    pub on: bool,
    pub at: f64,
}

/// What this process did. Disabled (no directory) in tests and for `status`.
pub struct Status {
    directory: Option<PathBuf>,
    root: PathBuf,
    pub state: Option<Current>,
    written: Option<Instant>,
    log: Option<File>,
}

impl Status {
    pub fn disabled() -> Self {
        Self {
            directory: None,
            root: PathBuf::new(),
            state: None,
            written: None,
            log: None,
        }
    }

    pub fn new(root: &Path, directory: PathBuf) -> Self {
        Self {
            directory: Some(directory),
            root: root.to_path_buf(),
            ..Self::disabled()
        }
    }

    pub fn directory(&self) -> Option<&Path> {
        self.directory.as_deref()
    }

    pub fn begin(&mut self, command: &str, name: Option<&str>, steps: &[&str]) -> Result<()> {
        self.state = Some(Current {
            pid: std::process::id(),
            command: command.into(),
            name: name.map(Into::into),
            started: now(),
            outcome: "running".into(),
            steps: steps
                .iter()
                .map(|s| Step {
                    label: (*s).into(),
                    state: "pending".into(),
                })
                .collect(),
            ..Current::default()
        });

        if let Some(directory) = &self.directory {
            fs::create_dir_all(directory)?;
            self.log = Some(File::create(directory.join(LOG))?);
        }
        self.write(true)
    }

    /// Mark `label` running and any running step done.
    pub fn step(&mut self, label: &str) {
        let Some(state) = &mut self.state else { return };
        for s in &mut state.steps {
            if s.state == "running" {
                s.state = "done".into();
            }
        }

        let index = match state
            .steps
            .iter()
            .position(|s| s.label == label && s.state == "pending")
        {
            Some(index) => index,
            None => {
                state.steps.push(Step {
                    label: label.into(),
                    state: "pending".into(),
                });
                state.steps.len() - 1
            }
        };
        state.steps[index].state = "running".into();
        state.progress = None;
        self.write_quietly(true);
    }

    /// One line of tool output: kept, logged, and scanned for progress counts.
    pub fn line(&mut self, text: &str) {
        let Some(state) = &mut self.state else { return };
        let text = text.trim_end();
        state.lines.push(text.into());
        let excess = state.lines.len().saturating_sub(KEPT_LINES);
        state.lines.drain(..excess);
        if let Some(log) = &mut self.log {
            let _ = writeln!(log, "{text}");
        }

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
                unit: unit.into(),
                since,
                since_done,
                at,
            });
        }
        self.write_quietly(false);
    }

    pub fn capture(&mut self, output: Option<&Path>, seconds: f64) {
        let root = self.root.clone();
        let Some(state) = &mut self.state else { return };
        state.capture = output.map(|path| Capture {
            path: relative(&root, path),
            seconds,
            started: now(),
        });
        self.write_quietly(true);
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
        self.write_quietly(true);
    }

    pub fn relay(&mut self, channel: u8, on: bool) {
        if let Some(state) = &mut self.state {
            state.relay.insert(channel.to_string(), on);
        }

        if let Some(directory) = &self.directory {
            let path = directory.join(RELAYS);
            let mut relays: BTreeMap<String, RelayState> = fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or_default();
            relays.insert(channel.to_string(), RelayState { on, at: now() });
            if let Ok(text) = serde_json::to_string(&relays) {
                let _ = replace(&path, text.as_bytes());
            }
        }
        self.write_quietly(true);
    }

    pub fn flash_tool(&mut self, tool: FlashTool) {
        if let Some(state) = &mut self.state {
            state.flash_tool = Some(tool);
        }
        self.write_quietly(true);
    }

    pub fn next(&mut self, line: String) {
        if let Some(state) = &mut self.state {
            state.next = Some(line);
        }
        self.write_quietly(true);
    }

    pub fn finish(&mut self, code: u8, error: Option<String>) {
        let Some(state) = &mut self.state else { return };
        let pass = code == 0;
        for s in &mut state.steps {
            if s.state == "running" {
                s.state = if pass { "done" } else { "failed" }.into();
            }
        }

        state.finished = Some(now());
        state.outcome = if pass { "pass" } else { "stopped" }.into();
        state.error = error;
        state.capture = None;
        state.transfer = None;

        let entry = HistoryEntry {
            command: state.command.clone(),
            name: state.name.clone(),
            started: state.started,
            finished: state.finished,
            outcome: state.outcome.clone(),
            error: state.error.clone(),
            next: state.next.clone(),
        };
        self.write_quietly(true);

        if let Some(directory) = &self.directory {
            let appended = OpenOptions::new()
                .create(true)
                .append(true)
                .open(directory.join(HISTORY))
                .and_then(|mut stream| {
                    writeln!(stream, "{}", serde_json::to_string(&entry).unwrap())
                });
            if let Err(error) = appended {
                eprintln!("Could not record bench history: {error}");
            }
        }
    }

    /// Progress is advisory: a failed status write never stops a session.
    fn write_quietly(&mut self, force: bool) {
        if let Err(error) = self.write(force) {
            eprintln!("Could not record bench progress: {error}");
        }
    }

    pub fn write(&mut self, force: bool) -> Result<()> {
        let Some(directory) = &self.directory else {
            return Ok(());
        };
        if !force
            && self
                .written
                .is_some_and(|at| at.elapsed().as_secs_f64() < 0.2)
        {
            return Ok(());
        }

        self.written = Some(Instant::now());
        fs::create_dir_all(directory)?;
        replace(&directory.join(CURRENT), &serde_json::to_vec(&self.state)?)?;
        Ok(())
    }
}

/// Whether a recorded process still exists (one owned by another user counts).
pub fn alive(pid: u32) -> bool {
    let Some(pid) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    else {
        return false;
    };
    match rustix::process::test_kill_process(pid) {
        Ok(()) => true,
        Err(error) => error == rustix::io::Errno::PERM,
    }
}

/// The command a live bench process is running, from its status file.
pub fn busy(directory: &Path) -> Option<String> {
    let text = fs::read_to_string(directory.join(CURRENT)).ok()?;
    let current: Current = serde_json::from_str(&text).ok()?;
    (current.outcome == "running" && alive(current.pid)).then_some(current.command)
}

/// Write beside the target, then rename over it, so a reader never sees a
/// half-written file.
fn replace(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, data)?;
    fs::rename(temporary, path)
}

pub fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// `Read N/M bytes` or `Verified changed sectors: N/M` in a tool's output.
pub fn progress_count(text: &str) -> Option<(u64, u64, &'static str)> {
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
            return pair(counts).map(|(d, t)| (d, t, "bytes"));
        }
    }
    let at = text.find("Verified changed sectors: ")?;
    pair(&text[at + 26..]).map(|(d, t)| (d, t, "sectors"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_steps_progress_relays_and_history() {
        let tmp = tempfile::tempdir().unwrap();
        let mut status = Status::new(tmp.path(), tmp.path().to_path_buf());
        status
            .begin(
                "dump",
                Some("unit"),
                &["loader entry", "wellfed-flash read full"],
            )
            .unwrap();

        status.relay(1, true);
        status.step("loader entry");
        status.step("wellfed-flash read full");
        status.line("Read 4096/8388608 bytes\n");
        status.write(true).unwrap();

        let read = |name| -> serde_json::Value {
            serde_json::from_str(&fs::read_to_string(tmp.path().join(name)).unwrap()).unwrap()
        };
        let current = read(CURRENT);
        assert_eq!(current["steps"][0]["state"], "done");
        assert_eq!(current["steps"][1]["state"], "running");
        assert_eq!(current["progress"]["done"], 4096);
        assert_eq!(current["progress"]["unit"], "bytes");
        assert_eq!(current["relay"], serde_json::json!({"1": true}));
        assert_eq!(read(RELAYS)["1"]["on"], true);

        let first = status.state.as_ref().unwrap().progress.clone().unwrap();
        status.line("Read 8192/8388608 bytes");
        let later = status.state.as_ref().unwrap().progress.clone().unwrap();
        // The rate baseline stays at the first report of this count.
        assert_eq!(
            (later.done, later.since, later.since_done),
            (8192, first.since, 4096)
        );

        status.finish(1, Some("Observer failed".into()));
        let current = read(CURRENT);
        assert_eq!(current["outcome"], "stopped");
        assert_eq!(current["steps"][1]["state"], "failed");

        let history = fs::read_to_string(tmp.path().join(HISTORY)).unwrap();
        let entry: HistoryEntry = serde_json::from_str(history.trim()).unwrap();
        assert_eq!(
            (entry.command.as_str(), entry.outcome.as_str()),
            ("dump", "stopped")
        );
        assert_eq!(entry.error.as_deref(), Some("Observer failed"));

        let log = fs::read_to_string(tmp.path().join(LOG)).unwrap();
        assert_eq!(log, "Read 4096/8388608 bytes\nRead 8192/8388608 bytes\n");
    }

    #[test]
    fn disabled_status_writes_nothing() {
        let mut status = Status::disabled();
        status.begin("boot", Some("unit"), &[]).unwrap();
        status.line("Read 1/2 bytes");
        status.finish(0, None);

        assert_eq!(status.state.unwrap().outcome, "pass");
    }

    #[test]
    fn progress_counts_parse_both_tools_forms() {
        assert_eq!(
            progress_count("Read 4096/8388608 bytes"),
            Some((4096, 8388608, "bytes"))
        );
        assert_eq!(
            progress_count("Verified changed sectors: 3/19"),
            Some((3, 19, "sectors"))
        );
        assert_eq!(progress_count("Read the ROM"), None);
        assert_eq!(progress_count("Region acquisition 1 complete"), None);
    }

    #[test]
    fn python_written_history_still_reads() {
        let line = r#"{"command": "dump", "name": "x", "started": 1.0, "finished": 2.0, "outcome": "pass", "error": null}"#;
        let entry: HistoryEntry = serde_json::from_str(line).unwrap();
        assert_eq!(entry.outcome, "pass");
    }
}
