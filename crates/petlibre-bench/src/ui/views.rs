//! Read-only views of the status files and evidence directories, as the
//! documents `assets/app.js` reads.
//!
//! Each public view backs one of the `--ui` page's API routes in the parent
//! module, which runs it on the blocking pool. Field names and shapes are the
//! page's contract: change one only with the script.
//!
//! Nothing here opens a serial port or writes a file. Every evidence path
//! from a status file must stay inside its private/ folder; others are ignored.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use petlibre_formats::canonical::sha256;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::commands::{Paths, check_name};
use crate::config::{CAPTURES, CONFIG, Config, FLASH_BACKUPS, STATUS_DIR, VERIFICATION};
use crate::now;
use crate::status::{
    self, Current, FlashTool, HISTORY, HistoryEntry, Outcome, Progress, RELAYS, RelayState, Step,
};

/// An operator script the owner writes by hand, in the status directory.
const SCRIPT: &str = "script.json";

const TAIL_BYTES: u64 = 12_000;
const TAIL_LINES: usize = 60;
const HISTORY_SHOWN: usize = 15;
const SESSIONS_SHOWN: usize = 40;

/// Boot captures are small; skip anything larger when looking for a banner.
const BANNER_SCAN_BYTES: u64 = 4 << 20;
const BANNER_SCAN_CAPTURES: usize = 25;

/// Where the views look: the checkout root and the flash client in use.
#[derive(Clone)]
pub(crate) struct Sources {
    pub root: PathBuf,
    pub flash_bin: PathBuf,
}

impl Sources {
    fn status(&self) -> PathBuf {
        self.root.join(STATUS_DIR)
    }
}

pub(super) fn modified(path: &Path) -> Option<f64> {
    let time = fs::metadata(path).ok()?.modified().ok()?;
    Some(time.duration_since(UNIX_EPOCH).ok()?.as_secs_f64())
}

/// `relative` under `base` (both under `root`), with no escape by `..` or links.
pub(super) fn inside(root: &Path, base: &str, relative: &str) -> Option<PathBuf> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return None;
    }

    let path = root.join(relative);
    let base = root.join(base);
    if !path.starts_with(&base) {
        return None;
    }
    match (path.canonicalize(), base.canonicalize()) {
        (Ok(real), Ok(real_base)) => real.starts_with(real_base).then_some(path),
        // Not created yet: the lexical check above is all there is.
        (Err(_), _) => Some(path),
        (Ok(_), Err(_)) => None,
    }
}

fn printable(data: &[u8]) -> String {
    data.iter()
        .filter(|b| **b == b'\n' || (32..127).contains(*b))
        .map(|b| *b as char)
        .collect()
}

/// A JSON file under the root, if it reads as `T`; the views show what is
/// there and skip what is not.
pub(super) fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    petlibre_formats::load(path).ok()
}

/// A file's size, modification time and printable tail: a capture's
/// serial-rx.bin or a run's server log.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct TailView {
    /// Root-relative: the capture directory, or the file itself.
    path: String,
    bytes: u64,
    modified: Option<f64>,
    tail: Vec<String>,
}

/// A capture directory's received bytes.
pub(super) fn rx_view(root: &Path, directory: &Path) -> TailView {
    tail_view(
        &directory.join("serial-rx.bin"),
        status::relative(root, directory),
    )
}

/// The last [`TAIL_LINES`] non-blank printable lines of `file`.
pub(super) fn tail_view(file: &Path, path: String) -> TailView {
    let Ok(mut opened) = fs::File::open(file) else {
        return TailView {
            path,
            bytes: 0,
            modified: None,
            tail: Vec::new(),
        };
    };

    let size = opened.metadata().map_or(0, |m| m.len());
    let mut data = Vec::new();
    if opened
        .seek(SeekFrom::Start(size.saturating_sub(TAIL_BYTES)))
        .is_ok()
    {
        let _ = opened.read_to_end(&mut data);
    }

    let text = printable(&data);
    let lines: Vec<&str> = text.split('\n').filter(|l| !l.trim().is_empty()).collect();
    let tail = lines[lines.len().saturating_sub(TAIL_LINES)..]
        .iter()
        .map(|line| (*line).to_string())
        .collect();
    TailView {
        path,
        bytes: size,
        modified: modified(file),
        tail,
    }
}

fn history(sources: &Sources) -> Vec<HistoryEntry> {
    fs::read_to_string(sources.status().join(HISTORY))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// `/api/status`: everything the "Now" view shows, as one snapshot.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct Snapshot {
    /// The bench host's clock, so the page's timers agree with it.
    now: f64,
    current: Option<CurrentView>,
    /// Newest first.
    history: Vec<HistoryEntry>,
    /// The latest capture's output.
    rx: Option<TailView>,
    script: Option<ScriptView>,
}

impl Snapshot {
    /// The same apart from the clock, which differs on every read.
    pub(crate) fn same_as(&self, other: &Self) -> bool {
        Self {
            now: other.now,
            ..self.clone()
        } == *other
    }
}

pub(crate) fn snapshot(sources: &Sources) -> Snapshot {
    let root = &sources.root;
    let current = status::current(&sources.status()).map(|current| current_view(root, current));

    let mut history = history(sources);
    let shown = history.len().saturating_sub(HISTORY_SHOWN);
    let mut history = history.split_off(shown);
    history.reverse();

    Snapshot {
        now: now(),
        current,
        history,
        rx: latest_capture(root).map(|dir| rx_view(root, &dir)),
        script: script_view(sources),
    }
}

/// `current.json` as the page reads it: a dead process's `running` record
/// shows as `ended`, and capture and transfer carry what has arrived so far.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
struct CurrentView {
    pid: u32,
    command: String,
    name: Option<String>,
    started: f64,
    finished: Option<f64>,
    outcome: Outcome,
    error: Option<String>,

    steps: Vec<Step>,
    progress: Option<Progress>,
    capture: Option<CaptureView>,
    transfer: Option<TransferView>,

    relay: BTreeMap<String, bool>,
    lines: Vec<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    next: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    flash_tool: Option<FlashTool>,
    /// The recording process still exists.
    alive: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
struct CaptureView {
    #[serde(flatten)]
    rx: TailView,
    seconds: f64,
    elapsed: f64,
}

/// Serial bytes so far against the plan's estimate, for the install bar.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
struct TransferView {
    tx: u64,
    rx: u64,
    tx_expected: u64,
    rx_expected: u64,
    started: Option<f64>,
}

fn current_view(root: &Path, current: Current) -> CurrentView {
    let alive = current.live();
    let outcome = match current.outcome {
        // The process exited without recording a finish.
        Outcome::Running if !alive => Outcome::Ended,
        outcome => outcome,
    };

    let capture = current.capture.and_then(|capture| {
        let path = inside(root, CAPTURES, &capture.path)?;
        Some(CaptureView {
            rx: rx_view(root, &path),
            seconds: capture.seconds,
            elapsed: (now() - capture.started).min(capture.seconds),
        })
    });
    let transfer = current.transfer.and_then(|transfer| {
        let path = inside(root, VERIFICATION, &transfer.path)?;
        let (tx, rx) = serial_sizes(&path);
        Some(TransferView {
            tx,
            rx,
            tx_expected: transfer.tx,
            rx_expected: transfer.rx,
            started: transfer.started,
        })
    });

    CurrentView {
        pid: current.pid,
        command: current.command,
        name: current.name,
        started: current.started,
        finished: current.finished,
        outcome,
        error: current.error,
        steps: current.steps,
        progress: current.progress,
        capture,
        transfer,
        relay: current.relay,
        lines: current.lines,
        next: current.next,
        flash_tool: current.flash_tool,
        alive,
    }
}

fn serial_sizes(directory: &Path) -> (u64, u64) {
    let size = |name| fs::metadata(directory.join(name)).map_or(0, |m| m.len());
    (size("serial-tx.bin"), size("serial-rx.bin"))
}

/// Capture directories, newest serial-rx.bin first.
fn captures_by_age(root: &Path) -> Vec<(f64, PathBuf)> {
    let mut found: Vec<(f64, PathBuf)> = fs::read_dir(root.join(CAPTURES))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let dir = entry.path();
            Some((modified(&dir.join("serial-rx.bin"))?, dir))
        })
        .collect();
    found.sort_by(|a, b| b.0.total_cmp(&a.0));
    found
}

fn latest_capture(root: &Path) -> Option<PathBuf> {
    captures_by_age(root).into_iter().next().map(|(_, dir)| dir)
}

/// Arrival time (Unix seconds) of the first `anchor` bytes in a capture, if seen.
fn anchor_time(directory: &Path, anchor: &str) -> Option<f64> {
    if anchor.is_empty() {
        return None;
    }
    let rx = fs::read(directory.join("serial-rx.bin")).ok()?;
    let offset = rx
        .windows(anchor.len())
        .position(|w| w == anchor.as_bytes())? as u64;

    // The receive event whose byte range holds that offset carries its time.
    let events = fs::read_to_string(directory.join("serial-events.jsonl")).ok()?;
    crate::commands::ota::arrival(&events, offset)
}

/// `script.json`: an operator script the owner writes by hand.
#[derive(Deserialize)]
struct Script {
    #[serde(default)]
    title: String,
    /// Root-relative capture the script runs against.
    #[serde(default)]
    capture: String,
    /// Bytes in that capture that start the script's clock.
    #[serde(default)]
    anchor: String,
    #[serde(default)]
    steps: Vec<ScriptStep>,
}

/// One instruction, shown from `at` seconds after the anchor arrived.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
struct ScriptStep {
    at: f64,
    text: String,
}

/// The optional operator script: timed steps from an anchor in a capture.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
struct ScriptView {
    title: String,
    capture: String,
    steps: Vec<ScriptStep>,
    anchor: String,
    /// When the anchor arrived; `None` until it has.
    started: Option<f64>,
    capture_exists: bool,
}

fn script_view(sources: &Sources) -> Option<ScriptView> {
    let script: Script = read(&sources.status().join(SCRIPT))?;
    let path = inside(&sources.root, CAPTURES, &script.capture)?;
    Some(ScriptView {
        started: anchor_time(&path, &script.anchor),
        capture_exists: path.is_dir(),
        title: script.title,
        capture: script.capture,
        steps: script.steps,
        anchor: script.anchor,
    })
}

/// Evidence parts a session name can have, as (kind, suffix, folder).
const PARTS: [(&str, &str, &str); 7] = [
    ("loader", "-loader", CAPTURES),
    ("bounded", "-bounded", FLASH_BACKUPS),
    ("full", "-full", FLASH_BACKUPS),
    ("inspect", "-inspect", VERIFICATION),
    ("plan", "-plan.json", VERIFICATION),
    ("execute", "-execute", VERIFICATION),
    ("boot", "-boot", CAPTURES),
];

fn report_status(path: &Path) -> Option<String> {
    read::<Value>(&path.join("report.json"))?["status"]
        .as_str()
        .map(str::to_string)
}

/// `/api/sessions`: recent session names with the evidence each one has.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct Sessions {
    sessions: Vec<SessionRow>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
struct SessionRow {
    name: String,
    modified: f64,
    /// Evidence kinds found, from [`PARTS`].
    parts: Vec<&'static str>,
    /// The furthest part's state, in the report's own words (`read complete`).
    outcome: String,
}

pub(crate) fn sessions(sources: &Sources) -> Sessions {
    let root = &sources.root;
    let mut found: BTreeMap<String, (f64, Vec<&'static str>)> = BTreeMap::new();
    for (kind, suffix, folder) in PARTS {
        for entry in fs::read_dir(root.join(folder))
            .into_iter()
            .flatten()
            .flatten()
        {
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str().and_then(|n| n.strip_suffix(suffix)) else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            let session = found.entry(name.to_string()).or_default();
            session.0 = session.0.max(modified(&entry.path()).unwrap_or(0.0));
            session.1.push(kind);
        }
    }

    let mut list: Vec<_> = found.into_iter().collect();
    list.sort_by(|a, b| b.1.0.total_cmp(&a.1.0));
    list.truncate(SESSIONS_SHOWN);

    // A report left "running" by a process that exited without finishing it.
    let live = status::current(&sources.status())
        .filter(Current::live)
        .and_then(|current| current.name);
    let sessions = list
        .into_iter()
        .map(|(name, (modified, parts))| {
            let p = Paths::new(root, &name);
            let outcome = if parts.contains(&"execute") {
                match report_status(&p.execute) {
                    Some(status) if status == "running" && live.as_deref() != Some(&name) => {
                        "ended".into()
                    }
                    status => status.unwrap_or_else(|| "unknown".into()),
                }
            } else if parts.contains(&"plan") {
                "prepared".into()
            } else if parts.contains(&"full") {
                report_status(&p.full).map_or_else(|| "unknown".into(), |s| format!("read {s}"))
            } else {
                "started".into()
            };

            SessionRow {
                name,
                modified,
                parts,
                outcome,
            }
        })
        .collect();

    Sessions { sessions }
}

/// Scalar fields of a report's result: the small facts, not bulk data. Any
/// object reads, since each report kind has its own fields.
fn scalars(value: &Value) -> Option<Value> {
    let object = value.as_object()?;
    Some(Value::Object(
        object
            .iter()
            .filter(|(_, v)| !v.is_array() && !v.is_object())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Map<_, _>>(),
    ))
}

/// A `petlibre-flash` report's headline fields and result scalars. The
/// report belongs to the flash client; the page shows its fields as given.
fn report_summary(directory: &Path) -> Option<Value> {
    let report: Value = read(&directory.join("report.json"))?;

    let mut summary = Map::new();
    for key in [
        "status",
        "mode",
        "error",
        "error_type",
        "start_utc",
        "end_utc",
        "tool",
        "transport",
    ] {
        if let Some(value) = report.get(key).filter(|v| !v.is_null()) {
            summary.insert(key.into(), value.clone());
        }
    }

    if let Some(result) = scalars(&report["result"]) {
        summary.insert("result".into(), result);
    }
    Some(Value::Object(summary))
}

fn plan_summary(path: &Path) -> Option<Value> {
    let raw = fs::read(path).ok()?;
    let plan: Value = serde_json::from_slice(&raw).unwrap_or_default();
    let operation = &plan["operation_plan"];
    let mut summary = scalars(operation).unwrap_or_default();
    summary["plan_sha256"] = Value::String(sha256(&raw));
    summary["sectors"] = operation["ordered_sectors"].as_array().map(Vec::len).into();
    Some(summary)
}

/// `/api/sessions/{name}`: one session's evidence, plan and next command.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct SessionView {
    name: String,
    parts: Vec<PartView>,
    /// The reviewed command that follows, if one was recorded.
    next: Option<String>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
struct PartView {
    kind: &'static str,
    path: String,
    modified: Option<f64>,
    #[serde(flatten)]
    detail: PartDetail,
    /// Execute only: the serial bytes it exchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    serial: Option<Serial>,
}

/// A plan's summary, or any other part's report (`null` when it has none).
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
enum PartDetail {
    Plan(Option<Value>),
    Report(Option<Value>),
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
struct Serial {
    tx: u64,
    rx: u64,
}

pub(crate) fn session(sources: &Sources, name: &str) -> Option<SessionView> {
    check_name(name).ok()?;
    let root = &sources.root;
    let p = Paths::new(root, name);

    let parts: Vec<PartView> = [
        ("loader", &p.loader),
        ("bounded", &p.bounded),
        ("full", &p.full),
        ("inspect", &p.inspect),
        ("plan", &p.plan),
        ("execute", &p.execute),
        ("boot", &p.boot),
    ]
    .into_iter()
    .filter(|(_, path)| path.exists())
    .map(|(kind, path)| {
        let detail = if kind == "plan" {
            PartDetail::Plan(plan_summary(path))
        } else {
            PartDetail::Report(report_summary(path))
        };
        let serial = (kind == "execute").then(|| {
            let (tx, rx) = serial_sizes(path);
            Serial { tx, rx }
        });

        PartView {
            kind,
            path: status::relative(root, path),
            modified: modified(path),
            detail,
            serial,
        }
    })
    .collect();

    if parts.is_empty() {
        return None;
    }

    let next = history(sources)
        .into_iter()
        .rev()
        .find(|entry| entry.name.as_deref() == Some(name) && entry.next.is_some())
        .and_then(|entry| entry.next);
    Some(SessionView {
        name: name.into(),
        parts,
        next,
    })
}

/// `/api/health`: ports, last relay states, flash client and the image the
/// feeder last booted.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct HealthView {
    /// Root-relative `bench.json`.
    config: &'static str,
    config_ok: bool,
    ports: Ports,
    relays: Option<BTreeMap<String, RelayState>>,
    flash_tool: FlashToolView,
    installed: Option<Installed>,
    /// The command a live bench process is running.
    busy: Option<String>,
}

/// Whether each configured path exists, or why the config does not read.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(untagged)]
enum Ports {
    Found {
        relay_port: bool,
        uart_port: bool,
        baseline: bool,
        rom_report: bool,
    },
    Unreadable {
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct FlashToolView {
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha256: Option<String>,
    present: bool,
}

/// The newest replacement-firmware start banner in a capture.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
struct Installed {
    capture: String,
    /// When that capture was last written.
    at: f64,
    build: Option<String>,
    bank: Option<String>,
    role: Role,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
enum Role {
    Normal,
    /// Bank 0's recovery role ran in that capture.
    Recovery,
}

pub(crate) fn health(sources: &Sources, flash: &mut FlashCache) -> HealthView {
    let root = &sources.root;
    let config = Config::load(root);
    let ports = match &config {
        Ok(config) => Ports::Found {
            relay_port: Path::new(&config.relay_port).exists(),
            uart_port: Path::new(&config.uart_port).exists(),
            baseline: root.join(&config.baseline).is_file(),
            rom_report: root.join(&config.rom_report).is_file(),
        },
        Err(error) => Ports::Unreadable {
            error: error.to_string(),
        },
    };

    HealthView {
        config: CONFIG,
        config_ok: config.is_ok(),
        ports,
        relays: read(&sources.status().join(RELAYS)),
        flash_tool: flash.get(sources),
        installed: installed(root),
        busy: status::busy(&sources.status()),
    }
}

/// The newest capture that shows a replacement-firmware start banner.
fn installed(root: &Path) -> Option<Installed> {
    const START: &[u8] = b"PLFW1 start build=";
    const RECOVERY: &[u8] = b"PLOTA1 start role=recovery";

    for (at, dir) in captures_by_age(root).into_iter().take(BANNER_SCAN_CAPTURES) {
        let rx = dir.join("serial-rx.bin");
        if fs::metadata(&rx).map_or(true, |m| m.len() > BANNER_SCAN_BYTES) {
            continue;
        }
        let Ok(data) = fs::read(&rx) else { continue };
        let Some(offset) = data.windows(START.len()).rposition(|w| w == START) else {
            continue;
        };

        let line = printable(&data[offset..]);
        let line = line.lines().next().unwrap_or("");
        let field = |key: &str| {
            line.split(' ')
                .find_map(|word| word.strip_prefix(key))
                .map(str::to_string)
        };
        let recovery = data.windows(RECOVERY.len()).any(|w| w == RECOVERY);

        return Some(Installed {
            capture: status::relative(root, &dir),
            at,
            build: field("build="),
            bank: field("bank="),
            role: if recovery {
                Role::Recovery
            } else {
                Role::Normal
            },
        });
    }
    None
}

/// The flash client's hash, recomputed only when the binary changes.
#[derive(Default)]
pub(crate) struct FlashCache {
    key: Option<(f64, u64)>,
    value: Option<FlashToolView>,
}

impl FlashCache {
    fn get(&mut self, sources: &Sources) -> FlashToolView {
        let missing = || FlashToolView {
            path: sources.flash_bin.display().to_string(),
            sha256: None,
            present: false,
        };
        let Ok(metadata) = fs::metadata(&sources.flash_bin) else {
            return missing();
        };

        let key = (modified(&sources.flash_bin).unwrap_or(0.0), metadata.len());
        let cached = self.value.as_ref().filter(|_| self.key == Some(key));
        if let Some(view) = cached {
            return view.clone();
        }

        let view = match crate::tools::flash_tool(&sources.root, &sources.flash_bin) {
            Some(FlashTool { path, sha256 }) => FlashToolView {
                path,
                sha256: Some(sha256),
                present: true,
            },
            None => missing(),
        };
        self.key = Some(key);
        self.value = Some(view.clone());
        view
    }
}

/// This module's page types, for [`super::bindings`].
#[cfg(test)]
pub(super) fn bindings(cfg: &ts_rs::Config) -> Vec<String> {
    use super::bindings::decl;
    vec![
        decl::<TailView>(cfg),
        decl::<Snapshot>(cfg),
        decl::<CurrentView>(cfg),
        decl::<CaptureView>(cfg),
        decl::<TransferView>(cfg),
        decl::<ScriptStep>(cfg),
        decl::<ScriptView>(cfg),
        decl::<Sessions>(cfg),
        decl::<SessionRow>(cfg),
        decl::<SessionView>(cfg),
        decl::<PartView>(cfg),
        decl::<PartDetail>(cfg),
        decl::<Serial>(cfg),
        decl::<HealthView>(cfg),
        decl::<Ports>(cfg),
        decl::<FlashToolView>(cfg),
        decl::<Installed>(cfg),
        decl::<Role>(cfg),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::{CURRENT, Status};
    use serde_json::json;

    fn sources() -> (tempfile::TempDir, Sources) {
        let tmp = tempfile::tempdir().unwrap();
        let sources = Sources {
            root: tmp.path().to_path_buf(),
            flash_bin: tmp.path().join("petlibre-flash"),
        };
        (tmp, sources)
    }

    /// A view as the page receives it.
    fn json(view: impl Serialize) -> Value {
        serde_json::to_value(view).unwrap()
    }

    #[test]
    fn empty() {
        let (_tmp, sources) = sources();

        let view = json(snapshot(&sources));
        assert!(view["current"].is_null());
        assert_eq!(view["history"], json!([]));
        assert!(view["rx"].is_null());
        assert!(view["script"].is_null());
        assert_eq!(json(sessions(&sources))["sessions"], json!([]));
    }

    #[test]
    fn live_capture_tail_is_printable_and_bounded() {
        let (_tmp, sources) = sources();
        let dir = sources.root.join(CAPTURES).join("cap");
        fs::create_dir_all(&dir).unwrap();
        let mut data = b"\x00\x15junk\r\n".to_vec();
        for i in 0..200 {
            data.extend(format!("line {i}\r\n").as_bytes());
        }
        fs::write(dir.join("serial-rx.bin"), &data).unwrap();

        let view = rx_view(&sources.root, &dir);
        assert_eq!(view.tail.len(), TAIL_LINES);
        assert_eq!(view.tail.last().unwrap(), "line 199");
        assert_eq!(view.bytes, data.len() as u64);
        assert_eq!(json(&view)["path"], "private/captures/cap");
    }

    #[test]
    fn capture_outside_private_captures_is_ignored() {
        let (_tmp, sources) = sources();

        for bad in ["../etc", "/etc", "a/../../b"] {
            assert!(inside(&sources.root, CAPTURES, bad).is_none(), "{bad}");
        }

        assert!(inside(&sources.root, CAPTURES, "private/verification/x").is_none());
        assert!(inside(&sources.root, CAPTURES, "private/captures/x").is_some());
    }

    #[test]
    fn a_dead_running_process_reads_as_ended_and_history_is_newest_first() {
        let (_tmp, sources) = sources();
        let mut status = Status::new(&sources.root, sources.root.join(STATUS_DIR));
        for name in ["one", "two"] {
            status.begin("boot", Some(name), &[]).unwrap();
            status.finish(0, None);
        }

        // A pid that cannot exist stands in for a process that died mid-run.
        let mut current = status::current(&sources.status()).unwrap();
        current.outcome = Outcome::Running;
        current.pid = u32::MAX;
        fs::write(
            sources.status().join(CURRENT),
            serde_json::to_string(&current).unwrap(),
        )
        .unwrap();

        let view = json(snapshot(&sources));
        assert_eq!(view["current"]["outcome"], "ended");
        assert_eq!(view["current"]["alive"], false);
        assert_eq!(view["history"][0]["name"], "two");
        assert_eq!(view["history"][0]["outcome"], "pass");
    }

    #[test]
    fn snapshots_compare_without_their_clock() {
        let (_tmp, sources) = sources();
        let first = snapshot(&sources);
        let mut later = first.clone();
        later.now += 1.0;

        assert!(later.same_as(&first));
        later.history.push(HistoryEntry {
            command: "boot".into(),
            name: None,
            started: 1.0,
            finished: None,
            outcome: Outcome::Pass,
            error: None,
            next: None,
        });
        assert!(!later.same_as(&first));
    }

    #[test]
    fn update_transfer_counts_session_bytes() {
        let (_tmp, sources) = sources();
        let execute = sources.root.join(VERIFICATION).join("s-execute");
        fs::create_dir_all(&execute).unwrap();
        fs::write(execute.join("serial-tx.bin"), [0u8; 3]).unwrap();
        fs::write(execute.join("serial-rx.bin"), [0u8; 5]).unwrap();

        let mut status = Status::new(&sources.root, sources.root.join(STATUS_DIR));
        status.begin("flash-execute", Some("s"), &[]).unwrap();
        status.transfer(Some(&execute), 100, 200);

        let view = json(snapshot(&sources));
        let transfer = &view["current"]["transfer"];
        assert_eq!(
            (transfer["tx"].as_u64(), transfer["rx"].as_u64()),
            (Some(3), Some(5))
        );
        assert_eq!(transfer["tx_expected"], 100);
        assert_eq!(view["current"]["outcome"], "running");
        assert_eq!(view["current"]["steps"], json!([]));
    }

    #[test]
    fn a_running_capture_shows_its_tail_and_time() {
        let (_tmp, sources) = sources();
        let dir = sources.root.join(CAPTURES).join("c");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("serial-rx.bin"), b"hello\r\n").unwrap();

        let mut status = Status::new(&sources.root, sources.root.join(STATUS_DIR));
        status.begin("capture", Some("c"), &["capture"]).unwrap();
        status.step("capture");
        status.capture(Some(&dir), 60.0);

        let view = json(snapshot(&sources));
        let capture = &view["current"]["capture"];
        assert_eq!(capture["path"], "private/captures/c");
        assert_eq!(capture["bytes"], 7);
        assert_eq!(capture["tail"], json!(["hello"]));
        assert_eq!(capture["seconds"], 60.0);
        assert!(capture["elapsed"].as_f64().unwrap() <= 60.0);
        let step = &view["current"]["steps"][0];
        assert_eq!(step["label"], "capture");
        assert_eq!(step["state"], "running");
        assert!(step["started"].as_f64().is_some());
        assert!(step.get("finished").is_none());
        assert_eq!(view["current"]["steps"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn operator_script_starts_at_the_anchor_arrival() {
        let (_tmp, sources) = sources();
        let dir = sources.root.join(CAPTURES).join("op");
        fs::create_dir_all(&dir).unwrap();
        fs::create_dir_all(sources.status()).unwrap();

        fs::write(dir.join("serial-rx.bin"), b"boot\r\nPLOPT1 open\r\n").unwrap();
        fs::write(
            dir.join("serial-events.jsonl"),
            "{\"utc\": \"2026-10-22T10:00:00+00:00\", \"direction\": \"rx\", \"offset\": 0, \"bytes\": 6}\n\
             {\"utc\": \"2026-10-22T10:00:01.500000+00:00\", \"direction\": \"rx\", \"offset\": 6, \"bytes\": 14}\n",
        )
        .unwrap();

        fs::write(
            sources.status().join(SCRIPT),
            r#"{"title": "t", "capture": "private/captures/op", "anchor": "PLOPT1 open", "steps": [{"at": 0, "text": "go"}]}"#,
        )
        .unwrap();

        let script = json(snapshot(&sources))["script"].clone();
        let expected = chrono::DateTime::parse_from_rfc3339("2026-10-22T10:00:01.5+00:00").unwrap();
        assert_eq!(
            script["started"].as_f64(),
            Some(expected.timestamp_millis() as f64 / 1e3)
        );
        assert_eq!(script["capture_exists"], true);
        assert_eq!(script["steps"], json!([{"at": 0.0, "text": "go"}]));
        assert_eq!(script["title"], "t");
    }

    #[test]
    fn a_running_report_reads_as_ended_unless_its_command_is_live() {
        let (_tmp, sources) = sources();
        let p = Paths::new(&sources.root, "s2");
        fs::create_dir_all(&p.execute).unwrap();
        fs::write(p.execute.join("report.json"), r#"{"status": "running"}"#).unwrap();
        assert_eq!(json(sessions(&sources))["sessions"][0]["outcome"], "ended");

        let mut status = Status::new(&sources.root, sources.status());
        status.begin("flash-execute", Some("s2"), &[]).unwrap();
        assert_eq!(
            json(sessions(&sources))["sessions"][0]["outcome"],
            "running"
        );
        status.finish(0, None);
    }

    #[test]
    fn sessions_group_evidence_and_show_the_plan_and_next_command() {
        let (_tmp, sources) = sources();
        let p = Paths::new(&sources.root, "s1");
        fs::create_dir_all(&p.loader).unwrap();
        fs::create_dir_all(&p.inspect).unwrap();
        fs::create_dir_all(&p.execute).unwrap();
        fs::write(
            p.inspect.join("report.json"),
            r#"{"status": "complete", "error": null, "result": {"bytes": 4, "big": [1]}}"#,
        )
        .unwrap();
        fs::write(p.execute.join("serial-tx.bin"), [0u8; 2]).unwrap();
        let plan =
            r#"{"operation_plan": {"bank": 1, "erase_sectors": 2, "ordered_sectors": [{}, {}]}}"#;
        fs::write(&p.plan, plan).unwrap();

        let mut status = Status::new(&sources.root, sources.status());
        status.begin("flash-prepare", Some("s1"), &[]).unwrap();
        status.next("petlibre-bench flash-execute s1 …".into());
        status.finish(0, None);

        let list = json(sessions(&sources));
        assert_eq!(list["sessions"][0]["name"], "s1");
        assert_eq!(list["sessions"][0]["outcome"], "unknown");
        assert_eq!(
            list["sessions"][0]["parts"],
            json!(["loader", "inspect", "plan", "execute"])
        );

        let detail = json(session(&sources, "s1").unwrap());
        let part = |kind: &str| {
            detail["parts"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["kind"] == kind)
                .unwrap()
                .clone()
        };
        let plan_part = part("plan");
        assert_eq!(plan_part["plan"]["sectors"], 2);
        assert_eq!(plan_part["plan"]["bank"], 1);
        assert_eq!(plan_part["plan"]["plan_sha256"], sha256(plan.as_bytes()));
        assert!(plan_part.get("report").is_none());

        let inspect = part("inspect");
        assert_eq!(
            inspect["report"],
            json!({"status": "complete", "result": {"bytes": 4}})
        );
        assert!(inspect.get("serial").is_none());
        assert_eq!(part("loader")["report"], Value::Null);
        assert_eq!(part("execute")["serial"], json!({"tx": 2, "rx": 0}));
        assert_eq!(detail["next"], "petlibre-bench flash-execute s1 …");

        assert!(session(&sources, "../s1").is_none());
        assert!(session(&sources, "absent").is_none());
    }

    #[test]
    fn health_reads_the_last_banner() {
        let (_tmp, sources) = sources();
        let dir = sources.root.join(CAPTURES).join("x-boot");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("serial-rx.bin"),
            b"noise\r\nPLFW1 start build=abc123 bank=1 reset=00000001 mmu=0,0,0\r\n",
        )
        .unwrap();

        let view = json(health(&sources, &mut FlashCache::default()));
        assert_eq!(view["installed"]["build"], "abc123");
        assert_eq!(view["installed"]["bank"], "1");
        assert_eq!(view["installed"]["role"], "normal");
        assert_eq!(view["installed"]["capture"], "private/captures/x-boot");
        assert_eq!(view["config"], CONFIG);
        assert_eq!(view["config_ok"], false);
        assert!(view["ports"]["error"].as_str().unwrap().contains(CONFIG));
        assert!(view["relays"].is_null());
        assert!(view["busy"].is_null());
        assert_eq!(view["flash_tool"]["present"], false);
        assert!(view["flash_tool"].get("sha256").is_none());
    }

    #[test]
    fn health_reports_present_ports_relays_and_the_flash_hash() {
        let (_tmp, sources) = sources();
        fs::create_dir_all(sources.root.join("private")).unwrap();
        fs::write(sources.root.join("baseline.bin"), b"").unwrap();
        fs::write(
            sources.root.join(CONFIG),
            r#"{"relay_port": "/nonexistent/relay", "uart_port": "/", "baseline": "baseline.bin", "rom_report": "rom.json"}"#,
        )
        .unwrap();
        fs::write(&sources.flash_bin, b"tool").unwrap();
        let mut status = Status::new(&sources.root, sources.status());
        status.begin("on", None, &[]).unwrap();
        status.relay(1, true);
        status.finish(0, None);

        let mut cache = FlashCache::default();
        let view = json(health(&sources, &mut cache));
        assert_eq!(view["config_ok"], true);
        assert_eq!(
            view["ports"],
            json!({"relay_port": false, "uart_port": true, "baseline": true, "rom_report": false})
        );
        assert_eq!(view["relays"]["1"]["on"], true);
        assert_eq!(
            view["flash_tool"],
            json!({"path": "petlibre-flash", "sha256": sha256(b"tool"), "present": true})
        );
        assert_eq!(
            json(health(&sources, &mut cache))["flash_tool"],
            view["flash_tool"]
        );
    }
}
