//! Durable hash-chained transaction journal.
//!
//! Port of `Journal` and `inspect_journal` in the former
//! `src/tools/flash/recovery-transaction.py`, whose journals still read. Every entry is fsynced before the operation it
//! announces. A stopped journal is evidence only: nothing resumes from it.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::error::{Result, refuse, require};
use crate::evidence::{Root, sync_directory, utc};
use crate::link::status::Assessment;
use petlibre_formats::canonical::{canonical, canonical_of, sha256};
use petlibre_formats::flash_plan::{DevelopmentPlan, GiveWayPlan};

/// Intent events after which flash may already differ from the preimage.
const MUTATIONS: [&str; 3] = [
    "erase_intent",
    "program_packet_intent",
    "marker_commit_intent",
];
const ZERO: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Every event a transaction records, named as the Python journal names it.
///
/// Some names carry different fields per transaction kind (a development
/// update and a stock recovery each have their own `transaction_start`), so
/// those variants rename to the same event.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    #[serde(rename = "transaction_start")]
    DevelopmentStart {
        plan: DevelopmentPlan,
    },
    #[serde(rename = "transaction_start")]
    GiveWayStart {
        plan: GiveWayPlan,
    },
    #[serde(rename = "transaction_start")]
    RecoveryStart {
        transaction_id: String,
        baseline_sha256: String,
        current_sha256: String,
        plan_sha256: String,
    },

    RegionVerified {
        phase: &'static str,
        sha256: String,
        bytes: usize,
    },
    GuardsVerified {
        phase: &'static str,
    },
    InitialSnapshotVerified {
        sha256: String,
    },

    FlashStatusUnstable {
        boundary: &'static str,
        start: usize,
        end_exclusive: usize,
        first: [u8; 3],
        second: [u8; 3],
    },
    FlashStatusChecked {
        boundary: &'static str,
        registers: [u8; 3],
        assessment: Assessment,
    },

    MarkerAlreadyStaged {
        sha256: String,
    },
    BeforeVerified {
        sha256: String,
    },
    EraseIntent {
        bytes: usize,
        /// The sector's restored hash, which stock recovery announces.
        #[serde(skip_serializing_if = "Option::is_none")]
        after_sha256: Option<String>,
    },
    EraseVerified {},
    EraseReadbackVerified {},
    ProgramPacketIntent {
        sequence: usize,
        frame_sha256: String,
    },
    ProgramPacketAck {
        sequence: usize,
    },
    ProgramEotIntent {},
    SectorStaged {
        sha256: String,
    },
    SectorVerified {
        sha256: String,
    },

    MarkerPreconditionsVerified {
        staged_snapshot_sha256: String,
    },
    #[serde(rename = "marker_commit_intent")]
    DevelopmentCommitIntent {
        target_region_sha256: String,
    },
    MarkerCommitIntent {
        proposed_snapshot_sha256: String,
    },
    #[serde(rename = "marker_commit_verified")]
    DevelopmentCommitVerified {
        sha256: String,
    },
    MarkerCommitVerified {
        snapshot_sha256: String,
    },

    GiveWayIntent {
        frame_sha256: String,
    },
    GiveWayVerified {
        after_sha256: String,
    },

    #[serde(rename = "transaction_verified")]
    DevelopmentVerified {
        snapshot_sha256: String,
        erase_sectors: usize,
    },
    #[serde(rename = "transaction_verified")]
    RecoveryVerified {
        snapshot_sha256: String,
        sector_count: usize,
        marker_commit: bool,
    },

    /// A sector or marker step stopped; the transaction's own stop follows.
    Stopped {
        mutation_possible: bool,
        error_type: &'static str,
    },
    TransactionStopped {
        error_type: &'static str,
        mutation_possible: bool,
    },
}

/// An event at a flash offset, as sector and marker steps record it.
#[derive(Debug, Clone, Serialize)]
pub struct At {
    pub offset: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<&'static str>,
    #[serde(flatten)]
    pub event: Event,
}

/// Journal or report sink for one event.
pub type Record<'a> = dyn FnMut(Event) -> Result<()> + 'a;

/// Journal sink for the events of one sector or marker step.
pub type Located<'a> = dyn FnMut(At) -> Result<()> + 'a;

/// One journal line; `sha256` covers the entry without itself.
#[derive(Serialize)]
struct Entry<'a> {
    sequence: u64,
    previous_sha256: &'a str,
    utc: String,
    event: &'a Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha256: Option<&'a str>,
}

/// A terminal report bound to the journal head.
#[derive(Serialize)]
struct Headed<'a, T> {
    #[serde(flatten)]
    report: &'a T,
    journal_head_sha256: &'a str,
}

#[derive(Serialize)]
struct StopReport<'a> {
    status: &'static str,
    error_type: &'static str,
    mutation_possible: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    transaction_id: Option<&'a str>,
}

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

    /// Append one event: anything that serializes to an object with a string
    /// `event` name, normally an [`Event`] or an [`At`].
    pub fn append(&mut self, event: &impl Serialize) -> Result<()> {
        require(
            !self.failed,
            "Journal failed previously; no append or replay",
        )?;

        let Value::Object(body) = serde_json::to_value(event)? else {
            return refuse("Journal events must be objects");
        };
        let Some(Value::String(name)) = body.get("event") else {
            return refuse("Journal events must be named");
        };
        let mutation = MUTATIONS.contains(&name.as_str());

        let mut entry = Entry {
            sequence: self.sequence,
            previous_sha256: &self.head,
            utc: utc(),
            event: &body,
            sha256: None,
        };
        let head = sha256(&canonical_of(&entry)?);
        entry.sha256 = Some(&head);
        let mut line = canonical_of(&entry)?;
        line.push(b'\n');

        // A failed write or fsync poisons the journal: no later append can
        // announce an operation the journal does not durably record.
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

        if mutation {
            self.mutation_possible = true;
        }
        Ok(())
    }

    /// The terminal report, bound to the journal head.
    pub fn save_report(&self, report: &impl Serialize) -> Result<()> {
        let mut data = canonical_of(&Headed {
            report,
            journal_head_sha256: &self.head,
        })?;
        data.push(b'\n');

        crate::evidence::write_new(&self.output.join("report.json"), &data)?;
        sync_directory(&self.output)
    }

    /// Record a stop and its report. The first failure stays the caller's
    /// error; a failing disk here is more evidence, never a replay trigger.
    pub fn stopped(&mut self, error: &crate::Error, transaction_id: Option<&str>) {
        let possible = self.mutation_possible;
        let appended = self.append(&Event::TransactionStopped {
            error_type: error.kind(),
            mutation_possible: possible,
        });

        if appended.is_err() {
            // As in Python: no report a terminal record does not back.
            return;
        }

        let _ = self.save_report(&StopReport {
            status: "stopped",
            error_type: error.kind(),
            mutation_possible: possible,
            transaction_id,
        });
    }
}

/// What a journal shows, as `inspect_journal` in Python reports it. The
/// copied fields are whatever the writer stored, so they stay JSON.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Receipt {
    pub records: usize,
    pub head_sha256: String,
    pub first_event: String,
    pub transaction_id: Option<Value>,
    pub verified_terminal: bool,
    pub terminal_event: String,
    pub terminal_snapshot_sha256: Option<Value>,
    pub mutation_intent_recorded: bool,
    pub erase_order: Vec<Value>,
}

/// Check for corruption or truncation; never resumes or sends commands.
///
/// Entries stay maps: each hash covers exactly what its writer (Python or
/// Rust) stored, including fields this client never writes.
pub fn inspect(data: &[u8]) -> Result<Receipt> {
    let Some(lines) = data
        .strip_suffix(b"\n")
        .filter(|_| data.len() <= 4 * 1024 * 1024)
    else {
        return refuse("Oversized or incomplete journal line");
    };

    let mut head = ZERO.to_string();
    let mut events: Vec<(String, Map<String, Value>)> = Vec::new();
    for (sequence, line) in lines.split(|b| *b == b'\n').enumerate() {
        let mut entry: Map<String, Value> = serde_json::from_slice(line)?;
        let mut keys: Vec<&str> = entry.keys().map(String::as_str).collect();
        keys.sort();
        require(
            keys == ["event", "previous_sha256", "sequence", "sha256", "utc"],
            "Unexpected journal fields",
        )?;

        let name = match entry["event"].get("event") {
            Some(Value::String(name)) => name.clone(),
            _ => return refuse("Unexpected journal event"),
        };
        require(
            entry["sequence"].as_u64() == u64::try_from(sequence).ok()
                && entry["previous_sha256"] == head.as_str(),
            "Broken journal ordering/hash link",
        )?;

        let supplied = entry.remove("sha256");
        let digest = sha256(&canonical(&Value::Object(entry.clone()))?);
        let Some(Value::String(supplied)) = supplied.filter(|s| *s == digest.as_str()) else {
            return refuse("Journal content hash mismatch");
        };

        let Some(Value::Object(event)) = entry.remove("event") else {
            return refuse("Unexpected journal event");
        };
        head = supplied;
        events.push((name, event));
    }

    let (Some((first, opening)), Some((last, terminal))) = (events.first(), events.last()) else {
        return refuse("Empty journal");
    };

    Ok(Receipt {
        records: events.len(),
        head_sha256: head.clone(),
        first_event: first.clone(),
        transaction_id: opening.get("transaction_id").cloned(),
        verified_terminal: last == "transaction_verified",
        terminal_event: last.clone(),
        terminal_snapshot_sha256: terminal.get("snapshot_sha256").cloned(),
        mutation_intent_recorded: events
            .iter()
            .any(|(name, _)| MUTATIONS.contains(&name.as_str())),
        erase_order: events
            .iter()
            .filter(|(name, _)| name == "erase_intent")
            .map(|(_, event)| event.get("offset").cloned().unwrap_or(Value::Null))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn chained_entries_inspect_and_tampering_is_found() {
        let checkout = tempfile::tempdir().unwrap();
        fs::create_dir(checkout.path().join("private")).unwrap();
        let root = Root::new(checkout.path()).unwrap();
        let output = checkout.path().join("private/transaction");
        let mut journal = Journal::create(&root, &output).unwrap();

        journal
            .append(&json!({"event": "transaction_start", "transaction_id": "t"}))
            .unwrap();
        assert!(!journal.mutation_possible());
        journal
            .append(&At {
                offset: 0x186000,
                phase: None,
                event: Event::EraseIntent {
                    bytes: 4096,
                    after_sha256: None,
                },
            })
            .unwrap();
        assert!(journal.mutation_possible());
        journal
            .append(&Event::DevelopmentVerified {
                snapshot_sha256: "x".into(),
                erase_sectors: 1,
            })
            .unwrap();
        journal.save_report(&json!({"status": "ok"})).unwrap();

        assert!(Journal::create(&root, &output).is_err());

        let data = fs::read(output.join("journal.jsonl")).unwrap();
        let receipt = inspect(&data).unwrap();
        assert_eq!(receipt.records, 3);
        assert!(receipt.verified_terminal);
        assert_eq!(receipt.erase_order, vec![json!(0x186000)]);
        assert_eq!(receipt.transaction_id, Some(json!("t")));
        assert_eq!(receipt.terminal_snapshot_sha256, Some(json!("x")));

        let report: Value =
            serde_json::from_slice(&fs::read(output.join("report.json")).unwrap()).unwrap();
        assert_eq!(report["journal_head_sha256"], receipt.head_sha256.as_str());

        let tampered = String::from_utf8(data.clone())
            .unwrap()
            .replace("1597440", "1597441");
        assert!(inspect(tampered.as_bytes()).is_err());
        assert!(inspect(&data[..data.len() - 1]).is_err());
        assert!(inspect(b"").is_err());
    }

    #[test]
    fn events_must_be_named_objects() {
        let checkout = tempfile::tempdir().unwrap();
        fs::create_dir(checkout.path().join("private")).unwrap();
        let root = Root::new(checkout.path()).unwrap();
        let mut journal = Journal::create(&root, &checkout.path().join("private/t")).unwrap();

        assert!(journal.append(&json!([1])).is_err());
        assert!(journal.append(&json!({"offset": 1})).is_err());
        assert!(journal.append(&Event::ProgramEotIntent {}).is_ok());
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
