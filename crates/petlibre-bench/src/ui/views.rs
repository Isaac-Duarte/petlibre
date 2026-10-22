//! Read-only JSON views of the status files and evidence directories.
//!
//! Each public view backs one of the `--ui` page's API routes in the parent
//! module, which runs it on the blocking pool.
//!
//! Nothing here opens a serial port or writes a file. Every evidence path
//! from a status file must stay inside its private/ folder; others are ignored.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::{Map, Value, json};

use crate::commands::{Paths, check_name};
use crate::config::{CAPTURES, CONFIG, Config, FLASH_BACKUPS, STATUS_DIR, VERIFICATION};
use crate::status::{self, CURRENT, Current, HISTORY, HistoryEntry, RELAYS, SCRIPT, alive};
use crate::{now, sha256_hex};

const TAIL_BYTES: u64 = 12_000;
const TAIL_LINES: usize = 60;
const HISTORY_SHOWN: usize = 15;
const SESSIONS_SHOWN: usize = 40;

/// Boot captures are small; skip anything larger when looking for a banner.
const BANNER_SCAN_BYTES: u64 = 4 << 20;
const BANNER_SCAN_CAPTURES: usize = 25;

/// Where the views look: the checkout root and the flash client in use.
#[derive(Clone)]
pub struct Sources {
    pub root: PathBuf,
    pub flash_bin: PathBuf,
}

impl Sources {
    fn status(&self) -> PathBuf {
        self.root.join(STATUS_DIR)
    }
}

fn modified(path: &Path) -> Option<f64> {
    let time = fs::metadata(path).ok()?.modified().ok()?;
    Some(time.duration_since(UNIX_EPOCH).ok()?.as_secs_f64())
}

/// `relative` under `base` (both under `root`), with no escape by `..` or links.
pub fn inside(root: &Path, base: &str, relative: &str) -> Option<PathBuf> {
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

/// Size, modification time and printable tail of a capture's serial-rx.bin.
pub fn rx_view(root: &Path, directory: &Path) -> Value {
    let rx = directory.join("serial-rx.bin");
    let path = status::relative(root, directory);
    let Ok(mut file) = fs::File::open(&rx) else {
        return json!({"path": path, "bytes": 0, "modified": null, "tail": []});
    };

    let size = file.metadata().map_or(0, |m| m.len());
    let mut data = Vec::new();
    if file
        .seek(SeekFrom::Start(size.saturating_sub(TAIL_BYTES)))
        .is_ok()
    {
        let _ = file.read_to_end(&mut data);
    }

    let text = printable(&data);
    let lines: Vec<&str> = text.split('\n').filter(|l| !l.trim().is_empty()).collect();
    let tail = &lines[lines.len().saturating_sub(TAIL_LINES)..];
    json!({"path": path, "bytes": size, "modified": modified(&rx), "tail": tail})
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

fn history(sources: &Sources) -> Vec<HistoryEntry> {
    fs::read_to_string(sources.status().join(HISTORY))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// Everything the "Now" view shows, as one snapshot.
pub fn snapshot(sources: &Sources) -> Value {
    let root = &sources.root;
    let current = fs::read_to_string(sources.status().join(CURRENT))
        .ok()
        .and_then(|text| serde_json::from_str::<Current>(&text).ok())
        .map(|current| current_view(root, current));

    let mut history = history(sources);
    let shown = history.len().saturating_sub(HISTORY_SHOWN);
    let mut history = history.split_off(shown);
    history.reverse();
    json!({"now": now(), "current": current, "history": history,
           "rx": latest_capture(root).map(|dir| rx_view(root, &dir)), "script": script_view(sources)})
}

fn current_view(root: &Path, current: Current) -> Value {
    let running = current.outcome == "running";
    let live = running && alive(current.pid);
    let capture = current.capture.clone();
    let transfer = current.transfer.clone();
    let mut view = serde_json::to_value(current).unwrap_or_default();
    view["alive"] = json!(live);
    if running && !live {
        // The process exited without recording a finish.
        view["outcome"] = json!("ended");
    }

    view["capture"] = capture
        .and_then(|capture| {
            let path = inside(root, CAPTURES, &capture.path)?;
            let mut rx = rx_view(root, &path);
            rx["seconds"] = json!(capture.seconds);
            rx["elapsed"] = json!((now() - capture.started).min(capture.seconds));
            Some(rx)
        })
        .unwrap_or(Value::Null);
    view["transfer"] = transfer
        .and_then(|transfer| {
            let path = inside(root, VERIFICATION, &transfer.path)?;
            let (tx, rx) = serial_sizes(&path);
            Some(
                json!({"tx": tx, "rx": rx, "tx_expected": transfer.tx, "rx_expected": transfer.rx,
                        "started": transfer.started}),
            )
        })
        .unwrap_or(Value::Null);

    view
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
pub fn anchor_time(directory: &Path, anchor: &str) -> Option<f64> {
    if anchor.is_empty() {
        return None;
    }
    let rx = fs::read(directory.join("serial-rx.bin")).ok()?;
    let offset = rx
        .windows(anchor.len())
        .position(|w| w == anchor.as_bytes())? as u64;

    // The receive event whose byte range holds that offset carries its time.
    let events = fs::read_to_string(directory.join("serial-events.jsonl")).ok()?;
    events
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|event| {
            let start = event["offset"].as_u64().unwrap_or(u64::MAX);
            let bytes = event["bytes"].as_u64().unwrap_or(0);
            event["direction"] == "rx" && start <= offset && offset < start.saturating_add(bytes)
        })
        .and_then(|event| {
            let utc = chrono::DateTime::parse_from_rfc3339(event["utc"].as_str()?).ok()?;
            Some(utc.timestamp_micros() as f64 / 1e6)
        })
}

/// The optional operator script: timed steps from an anchor in a capture.
fn script_view(sources: &Sources) -> Value {
    let Some(script) = read_json(&sources.status().join(SCRIPT)) else {
        return Value::Null;
    };
    let capture = script["capture"].as_str().unwrap_or("");
    let Some(path) = inside(&sources.root, CAPTURES, capture) else {
        return Value::Null;
    };
    let anchor = script["anchor"].as_str().unwrap_or("");
    json!({"title": script["title"].as_str().unwrap_or(""), "capture": capture,
           "steps": script.get("steps").cloned().unwrap_or(json!([])), "anchor": anchor,
           "started": anchor_time(&path, anchor), "capture_exists": path.is_dir()})
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
    read_json(&path.join("report.json"))?["status"]
        .as_str()
        .map(str::to_string)
}

/// Recent session names with the evidence each one has.
pub fn sessions(sources: &Sources) -> Value {
    let root = &sources.root;
    let mut found: BTreeMap<String, (f64, Vec<&str>)> = BTreeMap::new();
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
    let live = fs::read_to_string(sources.status().join(CURRENT))
        .ok()
        .and_then(|text| serde_json::from_str::<Current>(&text).ok())
        .filter(|current| current.outcome == "running" && alive(current.pid))
        .and_then(|current| current.name);
    let list: Vec<Value> = list
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
            json!({"name": name, "modified": modified, "parts": parts, "outcome": outcome})
        })
        .collect();

    json!({"sessions": list})
}

/// Scalar fields of a report's result: the small facts, not bulk data.
fn scalars(value: &Value) -> Value {
    let Some(object) = value.as_object() else {
        return Value::Null;
    };
    Value::Object(
        object
            .iter()
            .filter(|(_, v)| !v.is_array() && !v.is_object())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Map<_, _>>(),
    )
}

fn report_summary(directory: &Path) -> Value {
    let Some(report) = read_json(&directory.join("report.json")) else {
        return Value::Null;
    };

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

    let result = scalars(&report["result"]);
    if !result.is_null() {
        summary.insert("result".into(), result);
    }
    Value::Object(summary)
}

fn plan_summary(path: &Path) -> Value {
    let Ok(raw) = fs::read(path) else {
        return Value::Null;
    };
    let plan: Value = serde_json::from_slice(&raw).unwrap_or_default();
    let operation = &plan["operation_plan"];
    let mut summary = scalars(operation);
    summary["plan_sha256"] = json!(sha256_hex(&raw));
    summary["sectors"] = json!(operation["ordered_sectors"].as_array().map(Vec::len));
    summary
}

/// One session's evidence, plan and next command.
pub fn session(sources: &Sources, name: &str) -> Option<Value> {
    check_name(name).ok()?;
    let root = &sources.root;
    let p = Paths::new(root, name);

    let parts: Vec<Value> = [
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
        let mut part =
            json!({"kind": kind, "path": status::relative(root, path), "modified": modified(path)});
        if kind == "plan" {
            part["plan"] = plan_summary(path);
        } else {
            part["report"] = report_summary(path);
        }
        if kind == "execute" {
            let (tx, rx) = serial_sizes(path);
            part["serial"] = json!({"tx": tx, "rx": rx});
        }
        part
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
    Some(json!({"name": name, "parts": parts, "next": next}))
}

/// Ports, last relay states, flash client and the image the feeder last booted.
pub fn health(sources: &Sources, flash: &mut FlashCache) -> Value {
    let root = &sources.root;
    let config = Config::load(root);
    let ports = match &config {
        Ok(config) => json!({
            "relay_port": Path::new(&config.relay_port).exists(),
            "uart_port": Path::new(&config.uart_port).exists(),
            "baseline": root.join(&config.baseline).is_file(),
            "rom_report": root.join(&config.rom_report).is_file(),
        }),
        Err(error) => json!({"error": error.to_string()}),
    };

    json!({
        "config": CONFIG, "config_ok": config.is_ok(), "ports": ports,
        "relays": read_json(&sources.status().join(RELAYS)),
        "flash_tool": flash.get(sources),
        "installed": installed(root),
        "busy": status::busy(&sources.status()),
    })
}

/// The newest capture that shows a replacement-firmware start banner.
fn installed(root: &Path) -> Value {
    const START: &[u8] = b"PLFW1 start build=";
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
        let recovery = data
            .windows(b"PLOTA1 start role=recovery".len())
            .any(|w| w == b"PLOTA1 start role=recovery");
        return json!({"capture": status::relative(root, &dir), "at": at,
                      "build": field("build="), "bank": field("bank="),
                      "role": if recovery { "recovery" } else { "normal" }});
    }
    Value::Null
}

/// The flash client's hash, recomputed only when the binary changes.
#[derive(Default)]
pub struct FlashCache {
    key: Option<(f64, u64)>,
    value: Value,
}

impl FlashCache {
    pub fn get(&mut self, sources: &Sources) -> Value {
        let Ok(metadata) = fs::metadata(&sources.flash_bin) else {
            return json!({"path": sources.flash_bin.display().to_string(), "present": false});
        };

        let key = (modified(&sources.flash_bin).unwrap_or(0.0), metadata.len());
        if self.key != Some(key) {
            self.key = Some(key);
            self.value = match crate::tools::flash_tool(&sources.root, &sources.flash_bin) {
                Some(tool) => json!({"path": tool.path, "sha256": tool.sha256, "present": true}),
                None => json!({"path": sources.flash_bin.display().to_string(), "present": false}),
            };
        }
        self.value.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::Status;

    fn sources() -> (tempfile::TempDir, Sources) {
        let tmp = tempfile::tempdir().unwrap();
        let sources = Sources {
            root: tmp.path().to_path_buf(),
            flash_bin: tmp.path().join("petlibre-flash"),
        };
        (tmp, sources)
    }

    #[test]
    fn empty() {
        let (_tmp, sources) = sources();

        let view = snapshot(&sources);
        assert!(view["current"].is_null());
        assert_eq!(view["history"], json!([]));
        assert!(view["rx"].is_null());
        assert_eq!(sessions(&sources)["sessions"], json!([]));
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
        let tail = view["tail"].as_array().unwrap();
        assert_eq!(tail.len(), TAIL_LINES);
        assert_eq!(tail.last().unwrap(), "line 199");
        assert_eq!(view["bytes"], data.len());
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
        let mut current: Current =
            serde_json::from_str(&fs::read_to_string(sources.status().join(CURRENT)).unwrap())
                .unwrap();
        current.outcome = "running".into();
        current.pid = u32::MAX;
        fs::write(
            sources.status().join(CURRENT),
            serde_json::to_string(&current).unwrap(),
        )
        .unwrap();

        let view = snapshot(&sources);
        assert_eq!(view["current"]["outcome"], "ended");
        assert_eq!(view["history"][0]["name"], "two");
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

        let view = snapshot(&sources);
        let transfer = &view["current"]["transfer"];
        assert_eq!(
            (transfer["tx"].as_u64(), transfer["rx"].as_u64()),
            (Some(3), Some(5))
        );
        assert_eq!(transfer["tx_expected"], 100);
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

        let script = snapshot(&sources)["script"].clone();
        let expected = chrono::DateTime::parse_from_rfc3339("2026-10-22T10:00:01.5+00:00").unwrap();
        assert_eq!(
            script["started"].as_f64(),
            Some(expected.timestamp_millis() as f64 / 1e3)
        );
        assert_eq!(script["capture_exists"], true);
    }

    #[test]
    fn a_running_report_reads_as_ended_unless_its_command_is_live() {
        let (_tmp, sources) = sources();
        let p = Paths::new(&sources.root, "s2");
        fs::create_dir_all(&p.execute).unwrap();
        fs::write(p.execute.join("report.json"), r#"{"status": "running"}"#).unwrap();
        assert_eq!(sessions(&sources)["sessions"][0]["outcome"], "ended");

        let mut status = Status::new(&sources.root, sources.status());
        status.begin("flash-execute", Some("s2"), &[]).unwrap();
        assert_eq!(sessions(&sources)["sessions"][0]["outcome"], "running");
        status.finish(0, None);
    }

    #[test]
    fn sessions_group_evidence_and_show_the_plan_and_next_command() {
        let (_tmp, sources) = sources();
        let p = Paths::new(&sources.root, "s1");
        fs::create_dir_all(&p.loader).unwrap();
        fs::create_dir_all(&p.inspect).unwrap();
        fs::write(
            p.inspect.join("report.json"),
            r#"{"status": "complete", "result": {"bytes": 4, "big": [1]}}"#,
        )
        .unwrap();
        let plan =
            r#"{"operation_plan": {"bank": 1, "erase_sectors": 2, "ordered_sectors": [{}, {}]}}"#;
        fs::write(&p.plan, plan).unwrap();

        let mut status = Status::new(&sources.root, sources.status());
        status.begin("flash-prepare", Some("s1"), &[]).unwrap();
        status.next("petlibre-bench flash-execute s1 …".into());
        status.finish(0, None);

        let list = sessions(&sources);
        assert_eq!(list["sessions"][0]["name"], "s1");
        assert_eq!(list["sessions"][0]["outcome"], "prepared");

        let detail = session(&sources, "s1").unwrap();
        let plan_part = detail["parts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["kind"] == "plan")
            .unwrap();
        assert_eq!(plan_part["plan"]["sectors"], 2);
        assert_eq!(
            plan_part["plan"]["plan_sha256"],
            sha256_hex(plan.as_bytes())
        );

        let inspect = detail["parts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["kind"] == "inspect")
            .unwrap();
        assert_eq!(inspect["report"]["result"], json!({"bytes": 4}));
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

        let view = health(&sources, &mut FlashCache::default());
        assert_eq!(view["installed"]["build"], "abc123");
        assert_eq!(view["installed"]["bank"], "1");
        assert_eq!(view["installed"]["role"], "normal");
        assert_eq!(view["config_ok"], false);
        assert_eq!(view["flash_tool"]["present"], false);
    }
}
