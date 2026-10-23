//! The integration suite as the page shows it: the scenario catalog, every
//! run's `NAME-itest.json` record, and per-scenario history across runs.
//!
//! Like every view, this only reads. A run in progress has no record yet;
//! the page follows it through the status snapshot, whose steps are the
//! run's scenarios.

use std::fs;
use std::path::Path;

use serde::Serialize;

use super::views::tail::{TailView, rx_view, tail_view};
use super::views::{inside, modified, read};
use crate::commands::check_name;
use crate::commands::itest::record::{RunRecord, SUFFIX, ScenarioVerdict};
use crate::commands::itest::{CatalogEntry, catalog};
use crate::config::{CAPTURES, VERIFICATION};
use crate::status;

/// How many of the newest runs a scenario's history covers.
const HISTORY_RUNS: usize = 20;

/// `/api/tests`: the catalog, runs newest first, and each scenario's record.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct TestsView {
    catalog: Vec<CatalogEntry>,
    runs: Vec<RunRow>,
    /// Catalog order, then any scenario only older records name.
    scenarios: Vec<ScenarioStats>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
struct RunRow {
    #[serde(flatten)]
    record: RunRecord,
    /// When the record was written: the run's end.
    modified: f64,
}

/// One scenario across the runs that took it on.
#[derive(Debug, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
struct ScenarioStats {
    scenario: String,
    pass: u32,
    fail: u32,
    skip: u32,
    /// Median time of its passing runs.
    median_seconds: Option<f64>,
    last_failure: Option<Failure>,
    /// Newest first over the last [`HISTORY_RUNS`] runs; `null` where a run
    /// did not include it.
    history: Vec<Option<ScenarioVerdict>>,
    /// Pass and fail alternations in that history, skips aside: a scenario
    /// that flips is flaky, or its subject is.
    flips: u32,
}

#[derive(Debug, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
struct Failure {
    run: String,
    reason: Option<String>,
}

/// Every run record under `private/verification/`, newest first.
fn runs(root: &Path) -> Vec<RunRow> {
    let mut runs: Vec<RunRow> = fs::read_dir(root.join(VERIFICATION))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(SUFFIX))
        .filter_map(|entry| {
            let path = entry.path();
            Some(RunRow {
                record: read(&path)?,
                modified: modified(&path)?,
            })
        })
        .collect();
    runs.sort_by(|a, b| b.modified.total_cmp(&a.modified));
    runs
}

pub(crate) fn tests(root: &Path) -> TestsView {
    let catalog = catalog();
    let runs = runs(root);

    let mut names: Vec<String> = catalog.iter().map(|s| s.name.to_string()).collect();
    for run in &runs {
        for scenario in &run.record.scenarios {
            if !names.contains(&scenario.scenario) {
                names.push(scenario.scenario.clone());
            }
        }
    }
    let scenarios = names.iter().map(|name| stats(name, &runs)).collect();

    TestsView {
        catalog,
        runs,
        scenarios,
    }
}

fn stats(name: &str, runs: &[RunRow]) -> ScenarioStats {
    let mut stats = ScenarioStats {
        scenario: name.into(),
        pass: 0,
        fail: 0,
        skip: 0,
        median_seconds: None,
        last_failure: None,
        history: Vec::new(),
        flips: 0,
    };
    let mut passed = Vec::new();

    for (index, run) in runs.iter().enumerate() {
        let found = run.record.scenarios.iter().find(|s| s.scenario == name);
        if index < HISTORY_RUNS {
            stats.history.push(found.map(|s| s.verdict));
        }
        let Some(scenario) = found else { continue };

        match scenario.verdict {
            ScenarioVerdict::Pass => {
                stats.pass += 1;
                passed.push(scenario.seconds);
            }
            ScenarioVerdict::Fail => {
                stats.fail += 1;
                // Runs are newest first, so the first failure seen is the last.
                if stats.last_failure.is_none() {
                    stats.last_failure = Some(Failure {
                        run: run.record.name.clone(),
                        reason: scenario.reason.clone(),
                    });
                }
            }
            ScenarioVerdict::Skip => stats.skip += 1,
        }
    }

    let ran: Vec<ScenarioVerdict> = stats
        .history
        .iter()
        .flatten()
        .copied()
        .filter(|v| *v != ScenarioVerdict::Skip)
        .collect();
    stats.flips = ran.windows(2).filter(|w| w[0] != w[1]).count() as u32;
    stats.median_seconds = median(&mut passed);
    stats
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let mid = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    })
}

/// `/api/tests/{name}`: one run's record with its evidence tails.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct RunView {
    #[serde(flatten)]
    record: RunRecord,
    modified: Option<f64>,
    /// The tails of the record's captures that lie inside `private/captures/`.
    capture_tails: Vec<TailView>,
    /// `NAME-servers.log`: what `wellfed-local` and its helpers printed.
    servers: Option<TailView>,
}

pub(crate) fn run(root: &Path, name: &str) -> Option<RunView> {
    check_name(name).ok()?;
    let path = root.join(VERIFICATION).join(format!("{name}{SUFFIX}"));
    let record: RunRecord = read(&path)?;

    let capture_tails = record
        .captures
        .iter()
        .filter_map(|capture| inside(root, CAPTURES, capture))
        .map(|directory| rx_view(root, &directory))
        .collect();
    let log = root.join(VERIFICATION).join(format!("{name}-servers.log"));
    let servers = log
        .is_file()
        .then(|| tail_view(&log, status::relative(root, &log)));

    Some(RunView {
        record,
        modified: modified(&path),
        capture_tails,
        servers,
    })
}

/// This module's page types, for [`super::bindings`].
#[cfg(test)]
pub(super) fn bindings(cfg: &ts_rs::Config) -> Vec<String> {
    use super::bindings::decl;
    vec![
        decl::<RunRow>(cfg),
        decl::<Failure>(cfg),
        decl::<ScenarioStats>(cfg),
        decl::<TestsView>(cfg),
        decl::<RunView>(cfg),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn record(root: &Path, name: &str, scenarios: Value, captures: Value) {
        let dir = root.join(VERIFICATION);
        fs::create_dir_all(&dir).unwrap();
        let record = json!({
            "name": name,
            "evidence": "physical",
            "scenarios": scenarios,
            "silent": null,
            "captures": captures,
        });
        fs::write(
            dir.join(format!("{name}{SUFFIX}")),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        // Distinct modification times order the runs.
        std::thread::sleep(std::time::Duration::from_millis(15));
    }

    fn verdict(scenario: &str, verdict: &str, seconds: f64) -> Value {
        json!({ "scenario": scenario, "verdict": verdict, "reason": null, "seconds": seconds })
    }

    #[test]
    fn runs_are_newest_first_and_stats_follow_the_catalog() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        record(
            root,
            "run-1",
            json!([
                verdict("ota-update", "pass", 60.0),
                verdict("requests", "pass", 9.0)
            ]),
            json!([]),
        );
        record(
            root,
            "run-2",
            json!([
                verdict("ota-update", "pass", 80.0),
                {
                    "scenario": "requests", "verdict": "fail",
                    "reason": "No reply to ping within 20 s", "seconds": 21.0
                },
                verdict("retired", "skip", 0.0),
            ]),
            json!([]),
        );
        record(
            root,
            "run-3",
            json!([
                verdict("ota-update", "pass", 70.0),
                verdict("requests", "pass", 8.0)
            ]),
            json!([]),
        );

        let view = tests(root);

        let names: Vec<&str> = view.runs.iter().map(|r| r.record.name.as_str()).collect();
        assert_eq!(names, ["run-3", "run-2", "run-1"]);

        assert_eq!(view.scenarios[0].scenario, "ota-update");
        assert_eq!(view.scenarios[0].median_seconds, Some(70.0));
        assert_eq!(view.scenarios.last().unwrap().scenario, "retired");
        assert_eq!(view.scenarios.len(), view.catalog.len() + 1);

        let requests = &view.scenarios[1];
        assert_eq!((requests.pass, requests.fail, requests.skip), (2, 1, 0));
        assert_eq!(requests.flips, 2);
        assert_eq!(
            requests.last_failure,
            Some(Failure {
                run: "run-2".into(),
                reason: Some("No reply to ping within 20 s".into()),
            })
        );
        assert_eq!(
            requests.history,
            [
                Some(ScenarioVerdict::Pass),
                Some(ScenarioVerdict::Fail),
                Some(ScenarioVerdict::Pass)
            ]
        );

        let soak = view
            .scenarios
            .iter()
            .find(|s| s.scenario == "soak")
            .unwrap();
        assert_eq!(soak.history, [None, None, None]);
        assert_eq!(soak.median_seconds, None);
    }

    #[test]
    fn a_run_shows_its_captures_and_server_log_but_not_paths_outside() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let capture = root.join(CAPTURES).join("run-1-uart-01");
        fs::create_dir_all(&capture).unwrap();
        fs::write(capture.join("serial-rx.bin"), "PLFW1 start build=abc\r\n").unwrap();
        record(
            root,
            "run-1",
            json!([verdict("ota-update", "pass", 60.0)]),
            json!(["private/captures/run-1-uart-01", "../elsewhere"]),
        );
        fs::write(
            root.join(VERIFICATION).join("run-1-servers.log"),
            "serve: listening\n",
        )
        .unwrap();

        let view = run(root, "run-1").unwrap();

        assert_eq!(view.capture_tails.len(), 1);
        let shown = serde_json::to_value(&view).unwrap();
        assert_eq!(shown["evidence"], "physical");
        assert_eq!(
            shown["capture_tails"][0]["tail"][0],
            "PLFW1 start build=abc"
        );
        assert_eq!(shown["servers"]["tail"][0], "serve: listening");
        assert!(run(root, "absent").is_none());
        assert!(run(root, "../run-1").is_none());
    }
}
