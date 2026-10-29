//! `/api/status`: the "Now" view's single snapshot of the command in
//! progress, recent history, the latest capture and the operator script.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use super::qa::{QaView, qa_view};
use super::script::{ScriptView, script_view};
use super::tail::{TailView, rx_view};
use super::{Sources, history, inside, latest_capture, serial_sizes};
use wellfed_formats::bench::now;
use wellfed_formats::bench::progress::{
    BenchCommand, Current, FlashTool, HistoryEntry, Outcome, Progress, Step,
};
use wellfed_formats::bench::{CAPTURES, VERIFICATION};

const HISTORY_SHOWN: usize = 15;

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
    /// The QA firmware's guided run in the latest capture.
    qa: Option<QaView>,
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
    let current = sources.current().map(|current| current_view(root, current));

    let mut history = history(sources);
    let shown = history.len().saturating_sub(HISTORY_SHOWN);
    let mut history = history.split_off(shown);
    history.reverse();

    let latest = latest_capture(root);
    Snapshot {
        now: now(),
        current,
        history,
        rx: latest.as_deref().map(|dir| rx_view(root, dir)),
        script: script_view(sources),
        qa: latest.as_deref().and_then(|dir| qa_view(root, dir)),
    }
}

/// The running command's record as the page reads it: a dead process's
/// `running` record shows as `ended`, and capture and transfer carry what has
/// arrived so far.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
struct CurrentView {
    pid: u32,
    command: BenchCommand,
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
    let alive = crate::live(&current);
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

/// This module's page types, for [`super::bindings`].
#[cfg(test)]
pub(super) fn bindings(cfg: &ts_rs::Config) -> Vec<String> {
    use crate::bindings::decl;
    vec![
        decl::<Snapshot>(cfg),
        decl::<CurrentView>(cfg),
        decl::<CaptureView>(cfg),
        decl::<TransferView>(cfg),
    ]
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;
    use crate::views::tests::{json, live_sources, sources};
    use wellfed_formats::bench::progress::BenchCommand;

    #[test]
    fn a_dead_running_process_reads_as_ended_and_history_is_newest_first() {
        let (_tmp, sources, live) = live_sources();
        let mut status = live.status(&sources);
        for name in ["one", "two"] {
            status.begin(BenchCommand::Boot, Some(name), &[]).unwrap();
            status.finish(0, None);
        }

        // A pid that cannot exist stands in for a process that died mid-run.
        {
            let mut current = live.current.lock().unwrap();
            let current = current.as_mut().unwrap();
            current.outcome = Outcome::Running;
            current.pid = u32::MAX;
        }

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
            command: BenchCommand::Boot,
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
        let (_tmp, sources, live) = live_sources();
        let execute = sources.root.join(VERIFICATION).join("s-execute");
        fs::create_dir_all(&execute).unwrap();
        fs::write(execute.join("serial-tx.bin"), [0u8; 3]).unwrap();
        fs::write(execute.join("serial-rx.bin"), [0u8; 5]).unwrap();

        let mut status = live.status(&sources);
        status
            .begin(BenchCommand::FlashExecute, Some("s"), &[])
            .unwrap();
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
        let (_tmp, sources, live) = live_sources();
        let dir = sources.root.join(CAPTURES).join("c");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("serial-rx.bin"), b"hello\r\n").unwrap();

        let mut status = live.status(&sources);
        status
            .begin(BenchCommand::Capture, Some("c"), &["capture"])
            .unwrap();
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
}
