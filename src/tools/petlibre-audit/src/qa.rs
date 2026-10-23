//! The QA firmware's guided self-test, graded from its `PLQA1` lines after a
//! passing startup.
//!
//! The firmware (`src/firmware/petlibre/qa`) starts the five checks in a
//! fixed order, records each guided item and each check's verdict, and ends
//! with one `PLQA1 result` line. The audit trusts none of the summaries on
//! their own: every check must have started in order and reported a
//! verdict, the result line must repeat those verdicts, and the overall
//! status must follow from them. A run that passes all of that is graded by
//! its result; anything else is a sequence fault, not a pass or a fail.

use std::path::Path;
use std::sync::LazyLock;

use regex::bytes::Regex;
use serde::Serialize;

use crate::startup::{self, Evidence, Outcome, StartupReport};
use crate::{Error, Result, outcomes};

const LIMIT: &str = "UART evidence of the QA firmware's own verdicts. Button presses, lit indicators and \
    audible tunes are what the operator answered; power values are reported, not judged.";

/// The checks in sequence order, as the firmware names them.
const CHECKS: [&str; 5] = ["optics", "power", "buttons", "indicators", "speaker"];

outcomes! {
    /// One check's verdict, as the result line prints it.
    pub enum Verdict {
        Pass => "pass",
        Fail => "fail",
        None => "none",
    }
}

impl Verdict {
    fn parse(text: &[u8]) -> Option<Self> {
        match text {
            b"pass" => Some(Self::Pass),
            b"fail" => Some(Self::Fail),
            b"none" => Some(Self::None),
            _ => None,
        }
    }
}

/// The audit's outcome: a failed startup, a broken sequence, or the run's
/// own result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QaOutcome {
    Pass,
    /// The startup before the sequence failed with this outcome.
    Startup(Outcome),
    /// Missing, repeated, out-of-order or inconsistent `PLQA1` records.
    Sequence,
    /// The sequence ran in full and at least one check failed.
    Fail,
}

impl std::fmt::Display for QaOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Pass => "pass",
            Self::Startup(outcome) => return write!(f, "startup_{outcome}"),
            Self::Sequence => "qa_sequence",
            Self::Fail => "qa_fail",
        })
    }
}

impl Serialize for QaOutcome {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// A guided item that failed, as the firmware recorded it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FailedItem {
    pub stage: String,
    /// `press=reset`, `led=PA2` or `tune=test`.
    pub item: String,
    pub reason: String,
}

/// What the `PLQA1` records say, whatever the outcome.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct QaFields {
    /// Each check's verdict from its `check` line, in sequence order;
    /// `None` for a check that never reported one.
    pub qa_checks: Vec<(String, Option<Verdict>)>,
    pub qa_failed_items: Vec<FailedItem>,
    pub qa_prompts: usize,
    /// Why the sequence was refused, for a `qa_sequence` outcome.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qa_sequence_fault: Option<&'static str>,
}

static START: LazyLock<Regex> = LazyLock::new(|| line(r"start stage=([a-z]+)"));
static PROMPT: LazyLock<Regex> = LazyLock::new(|| {
    line(
        r"prompt (?:press=[a-z]+|look=P[AB][0-9]+ yes=dispense no=lock|listen=[a-z]+ yes=dispense no=lock)",
    )
});
static ITEM: LazyLock<Regex> = LazyLock::new(|| {
    line(
        r"item stage=([a-z]+) ((?:press|led|tune)=[A-Za-z0-9]+) status=(?:pass|fail reason=([a-z]+))",
    )
});
static CHECK: LazyLock<Regex> = LazyLock::new(|| line(r"check stage=([a-z]+) status=(pass|fail)"));
static RESULT: LazyLock<Regex> = LazyLock::new(|| {
    line(
        r"result status=(pass|fail) optics=([a-z]+) power=([a-z]+) buttons=([a-z]+) indicators=([a-z]+) speaker=([a-z]+)",
    )
});

fn line(body: &str) -> Regex {
    Regex::new(&format!(r"(?-u)^PLQA1 {body}$")).expect("QA pattern")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The check verdicts and failed items in `rx`, and the outcome they grade
/// to (`Pass`, `Fail` or `Sequence`).
pub(crate) fn classify(rx: &[u8]) -> (QaOutcome, QaFields) {
    let mut fields = QaFields {
        qa_checks: CHECKS.iter().map(|c| ((*c).to_string(), None)).collect(),
        ..QaFields::default()
    };
    match grade(rx, &mut fields) {
        Ok(outcome) => (outcome, fields),
        Err(fault) => {
            fields.qa_sequence_fault = Some(fault);
            (QaOutcome::Sequence, fields)
        }
    }
}

fn grade(rx: &[u8], fields: &mut QaFields) -> std::result::Result<QaOutcome, &'static str> {
    let mut started = 0;
    let mut result: Option<(Verdict, [Option<Verdict>; 5])> = None;

    for record in rx.split(|b| *b == b'\n') {
        // The firmware ends each record with "\r\n" and the console adds a
        // "\r" after every "\n", so on the wire a line arrives as
        // "\rPLQA1 …\r".
        let record = record.trim_ascii_start().trim_ascii_end();
        let Some(at) = record.windows(6).position(|w| w == b"PLQA1 ") else {
            continue;
        };
        if at != 0 {
            return Err("a PLQA1 record does not start its line");
        }
        if result.is_some() {
            return Err("records after the result");
        }

        if let Some(m) = START.captures(record) {
            if CHECKS.get(started).map(|c| c.as_bytes()) != Some(&m[1]) {
                return Err("checks started out of order");
            }
            started += 1;
        } else if PROMPT.is_match(record) {
            fields.qa_prompts += 1;
        } else if let Some(m) = ITEM.captures(record) {
            if let Some(reason) = m.get(3) {
                fields.qa_failed_items.push(FailedItem {
                    stage: text(&m[1]),
                    item: text(&m[2]),
                    reason: text(reason.as_bytes()),
                });
            }
        } else if let Some(m) = CHECK.captures(record) {
            let index = CHECKS
                .iter()
                .position(|c| c.as_bytes() == &m[1])
                .ok_or("a verdict for an unknown check")?;
            let slot = &mut fields.qa_checks[index].1;
            if index + 1 != started || slot.is_some() {
                return Err("a verdict outside its check");
            }
            *slot = Verdict::parse(&m[2]);
        } else if let Some(m) = RESULT.captures(record) {
            let mut verdicts = [None; 5];
            for (index, verdict) in verdicts.iter_mut().enumerate() {
                *verdict = Verdict::parse(&m[index + 2]);
            }
            let status = Verdict::parse(&m[1]).ok_or("an unreadable result")?;
            result = Some((status, verdicts));
        } else {
            return Err("an unreadable PLQA1 record");
        }
    }

    let (status, verdicts) = result.ok_or("no result")?;
    if started != CHECKS.len() {
        return Err("a check never started");
    }
    let reported: Vec<Option<Verdict>> = fields.qa_checks.iter().map(|(_, v)| *v).collect();
    if reported.iter().any(Option::is_none) {
        return Err("a check never reported a verdict");
    }
    if reported != verdicts {
        return Err("the result disagrees with the verdicts");
    }
    let all_passed = reported.iter().all(|v| *v == Some(Verdict::Pass));
    if all_passed != (status == Verdict::Pass) {
        return Err("the result status disagrees with the verdicts");
    }
    if all_passed && !fields.qa_failed_items.is_empty() {
        return Err("a failed item in a passing run");
    }

    Ok(if all_passed {
        QaOutcome::Pass
    } else {
        QaOutcome::Fail
    })
}

/// The QA audit's report.
#[derive(Debug, Clone, Serialize)]
pub struct QaReport {
    outcome: QaOutcome,
    #[serde(flatten)]
    qa: QaFields,
    startup: StartupReport,
    #[serde(flatten)]
    evidence: Evidence,
    limit: &'static str,
}

impl QaReport {
    pub fn passed(&self) -> bool {
        self.outcome == QaOutcome::Pass
    }
}

/// The QA run in one capture of the QA firmware, after a passing startup.
pub fn verify(capture: &Path, manifest: &Path, image: &Path) -> Result<QaReport> {
    let loaded = startup::load(capture, manifest, image)?;
    if loaded.manifest.firmware.as_deref() != Some("qa") {
        return Err(Error::Refused("A QA firmware build required".into()));
    }

    let c = &loaded.capture;
    let arrival = |offset: usize| c.arrival(offset);
    let check = startup::startup(
        &c.rx,
        &loaded.manifest,
        &loaded.image,
        &loaded.identity,
        Some(&arrival),
        Some(c.end()?),
    )?;

    let (mut outcome, qa) = classify(&c.rx);
    if check.outcome != Outcome::Pass {
        outcome = QaOutcome::Startup(check.outcome);
    }

    Ok(QaReport {
        outcome,
        qa,
        startup: check,
        evidence: loaded.evidence(),
        limit: LIMIT,
    })
}

#[cfg(test)]
mod tests;
