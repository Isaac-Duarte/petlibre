//! The optional operator script on the "Now" view: steps the owner writes by
//! hand, timed from when an anchor arrives in a capture.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{Sources, inside, read};
use wellfed_formats::bench::CAPTURES;

/// An operator script the owner writes by hand, in the status directory.
const SCRIPT: &str = "script.json";

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
    wellfed_formats::capture::arrival(&events, offset)
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
pub(super) struct ScriptView {
    title: String,
    capture: String,
    steps: Vec<ScriptStep>,
    anchor: String,
    /// When the anchor arrived; `None` until it has.
    started: Option<f64>,
    capture_exists: bool,
}

pub(super) fn script_view(sources: &Sources) -> Option<ScriptView> {
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

/// This module's page types, for [`super::bindings`].
#[cfg(test)]
pub(super) fn bindings(cfg: &ts_rs::Config) -> Vec<String> {
    use crate::bindings::decl;
    vec![decl::<ScriptStep>(cfg), decl::<ScriptView>(cfg)]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::views::snapshot::snapshot;
    use crate::views::tests::{json, sources};

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
}
