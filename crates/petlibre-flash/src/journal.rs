//! Durable hash-chained transaction journal.
//!
//! Port of `Journal` and `inspect_journal` in
//! `tools/flash/recovery-transaction.py`; either implementation reads the
//! other's journals. Every entry is fsynced before the operation it
//! announces. A stopped journal is evidence only: nothing resumes from it.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::canonical::{canonical, sha256};
use crate::error::{Result, require};
use crate::evidence::{Root, sync_directory, utc};

const MUTATIONS: [&str; 3] = [
    "erase_intent",
    "program_packet_intent",
    "marker_commit_intent",
];
const ZERO: &str = "0000000000000000000000000000000000000000000000000000000000000000";

pub struct Journal {
    output: PathBuf,
    stream: File,
    sequence: u64,
    head: String,
    failed: bool,
    mutation_possible: bool,
}

impl Journal {
    /// A new private directory; an existing one is terminal, never resumed.
    pub fn create(root: &Root, output: &Path) -> Result<Self> {
        let output = std::path::absolute(output)?;
        require(
            !output.is_symlink() && root.is_private(&output),
            "Journal output must be a new private directory",
        )?;
        let parent = output.parent().map(Path::to_path_buf).unwrap_or_default();
        require(parent.is_dir(), "Journal parent must already exist")?;
        fs::DirBuilder::new().mode(0o700).create(&output)?;
        let stream = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(output.join("journal.jsonl"))?;
        sync_directory(&output)?;
        sync_directory(&parent)?;
        Ok(Self {
            output,
            stream,
            sequence: 0,
            head: ZERO.into(),
            failed: false,
            mutation_possible: false,
        })
    }

    pub fn mutation_possible(&self) -> bool {
        self.mutation_possible
    }

    pub fn append(&mut self, event: &str, fields: Map<String, Value>) -> Result<()> {
        require(
            !self.failed,
            "Journal failed previously; no append or replay",
        )?;
        let mut body = fields;
        body.insert("event".into(), Value::String(event.into()));
        let mut entry = json!({"sequence": self.sequence, "previous_sha256": self.head,
                               "utc": utc(), "event": body});
        let head = sha256(&canonical(&entry)?);
        entry["sha256"] = Value::String(head.clone());
        let mut line = canonical(&entry)?;
        line.push(b'\n');
        if let Err(error) = self
            .stream
            .write_all(&line)
            .and_then(|()| self.stream.sync_all())
        {
            self.failed = true;
            return Err(error.into());
        }
        self.sequence += 1;
        self.head = head;
        if MUTATIONS.contains(&event) {
            self.mutation_possible = true;
        }
        Ok(())
    }

    /// The terminal report, bound to the journal head.
    pub fn save_report(&self, report: Value) -> Result<()> {
        let mut report = report;
        report["journal_head_sha256"] = Value::String(self.head.clone());
        let mut data = canonical(&report)?;
        data.push(b'\n');
        crate::evidence::write_new(&self.output.join("report.json"), &data)?;
        sync_directory(&self.output)
    }

    /// Record a stop and its report. The first failure stays the caller's
    /// error; a failing disk here is more evidence, never a replay trigger.
    pub fn stopped(&mut self, error: &crate::Error, extra: Map<String, Value>) {
        let possible = self.mutation_possible;
        let appended = self.append(
            "transaction_stopped",
            crate::status::fields(
                json!({"error_type": error.kind(), "mutation_possible": possible}),
            ),
        );
        if appended.is_err() {
            // As in Python: no report a terminal record does not back.
            return;
        }
        let mut report = json!({"status": "stopped", "error_type": error.kind(),
                                "mutation_possible": possible});
        for (key, value) in extra {
            report[key] = value;
        }
        let _ = self.save_report(report);
    }
}

/// Check for corruption or truncation; never resumes or sends commands.
pub fn inspect(data: &[u8]) -> Result<Value> {
    require(
        data.len() <= 4 * 1024 * 1024 && data.ends_with(b"\n"),
        "Oversized or incomplete journal line",
    )?;
    let mut head = ZERO.to_string();
    let mut events: Vec<Map<String, Value>> = Vec::new();
    for (sequence, line) in data[..data.len() - 1].split(|b| *b == b'\n').enumerate() {
        let mut entry: Map<String, Value> = serde_json::from_slice(line)?;
        let mut keys: Vec<&str> = entry.keys().map(String::as_str).collect();
        keys.sort();
        require(
            keys == ["event", "previous_sha256", "sequence", "sha256", "utc"],
            "Unexpected journal fields",
        )?;
        require(
            entry["event"].get("event").is_some_and(Value::is_string),
            "Unexpected journal event",
        )?;
        require(
            entry["sequence"].as_u64() == Some(sequence as u64)
                && entry["previous_sha256"] == head.as_str(),
            "Broken journal ordering/hash link",
        )?;
        let supplied = entry.remove("sha256").unwrap();
        require(
            supplied.as_str() == Some(&sha256(&canonical(&Value::Object(entry.clone()))?)),
            "Journal content hash mismatch",
        )?;
        head = supplied.as_str().unwrap().to_string();
        let Some(Value::Object(event)) = entry.remove("event") else {
            unreachable!()
        };
        events.push(event);
    }
    require(!events.is_empty(), "Empty journal")?;
    let name = |e: &Map<String, Value>| e["event"].as_str().unwrap().to_string();
    let last = events.last().unwrap();
    Ok(json!({
        "records": events.len(), "head_sha256": head,
        "first_event": name(&events[0]),
        "transaction_id": events[0].get("transaction_id"),
        "verified_terminal": name(last) == "transaction_verified",
        "terminal_event": name(last),
        "terminal_snapshot_sha256": last.get("snapshot_sha256"),
        "mutation_intent_recorded": events.iter().any(|e| MUTATIONS.contains(&name(e).as_str())),
        "erase_order": events.iter().filter(|e| name(e) == "erase_intent")
            .map(|e| e["offset"].clone()).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::fields;

    #[test]
    fn chained_entries_inspect_and_tampering_is_found() {
        let checkout = tempfile::tempdir().unwrap();
        fs::create_dir(checkout.path().join("private")).unwrap();
        let root = Root::new(checkout.path()).unwrap();
        let output = checkout.path().join("private/transaction");
        let mut journal = Journal::create(&root, &output).unwrap();
        journal
            .append("transaction_start", fields(json!({"transaction_id": "t"})))
            .unwrap();
        assert!(!journal.mutation_possible());
        journal
            .append("erase_intent", fields(json!({"offset": 0x186000})))
            .unwrap();
        assert!(journal.mutation_possible());
        journal
            .append(
                "transaction_verified",
                fields(json!({"snapshot_sha256": "x"})),
            )
            .unwrap();
        journal.save_report(json!({"status": "ok"})).unwrap();
        assert!(Journal::create(&root, &output).is_err());
        let data = fs::read(output.join("journal.jsonl")).unwrap();
        let receipt = inspect(&data).unwrap();
        assert_eq!(receipt["records"], 3);
        assert_eq!(receipt["verified_terminal"], true);
        assert_eq!(receipt["erase_order"], json!([0x186000]));
        assert_eq!(receipt["transaction_id"], "t");
        let report: Value =
            serde_json::from_slice(&fs::read(output.join("report.json")).unwrap()).unwrap();
        assert_eq!(report["journal_head_sha256"], receipt["head_sha256"]);
        let tampered = String::from_utf8(data.clone())
            .unwrap()
            .replace("1597440", "1597441");
        assert!(inspect(tampered.as_bytes()).is_err());
        assert!(inspect(&data[..data.len() - 1]).is_err());
    }

    #[test]
    fn journals_stay_inside_private() {
        let checkout = tempfile::tempdir().unwrap();
        fs::create_dir(checkout.path().join("private")).unwrap();
        let root = Root::new(checkout.path()).unwrap();
        assert!(Journal::create(&root, &checkout.path().join("elsewhere")).is_err());
        assert!(Journal::create(&root, &checkout.path().join("private/missing/t")).is_err());
    }
}
