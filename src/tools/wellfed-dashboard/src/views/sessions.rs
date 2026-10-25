//! `/api/sessions` and `/api/sessions/{name}`: evidence sessions, grouped by
//! name across the capture, backup and verification folders.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use wellfed_formats::canonical::sha256;
use serde::Serialize;
use serde_json::{Map, Value};

use super::{Sources, history, modified, read, serial_sizes};
use wellfed_formats::bench::progress;
use wellfed_formats::bench::{CAPTURES, FLASH_BACKUPS, Paths, VERIFICATION, plain_name, relative};

const SESSIONS_SHOWN: usize = 40;

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
    let live = progress::current(&sources.status())
        .filter(crate::live)
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

/// A `wellfed-flash` report's headline fields and result scalars. The
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
    plain_name(name).then_some(())?;
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
            path: relative(root, path),
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

/// This module's page types, for [`super::bindings`].
#[cfg(test)]
pub(super) fn bindings(cfg: &ts_rs::Config) -> Vec<String> {
    use crate::bindings::decl;
    vec![
        decl::<Sessions>(cfg),
        decl::<SessionRow>(cfg),
        decl::<SessionView>(cfg),
        decl::<PartView>(cfg),
        decl::<PartDetail>(cfg),
        decl::<Serial>(cfg),
    ]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::views::tests::{json, sources};
    use wellfed_formats::bench::progress::Status;

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
        status.next("wellfed-bench flash-execute s1 …".into());
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
        assert_eq!(detail["next"], "wellfed-bench flash-execute s1 …");

        assert!(session(&sources, "../s1").is_none());
        assert!(session(&sources, "absent").is_none());
    }
}
