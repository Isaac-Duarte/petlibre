//! `private/bench-status/notes.jsonl`: what the operator records with
//! `wellfed-bench note`, one line each: the step under way, the steps
//! after it, timers (each with the device event it expects, if any) and
//! evidence by card with a verdict.
//!
//! The file is append-only; [`Operator::fold`] replays it into what the
//! page and the console show now.

use serde::{Deserialize, Serialize};

/// Root-relative: the operator's notes.
pub const NOTES: &str = "private/bench-status/notes.jsonl";

/// One line of the notes file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Note {
    pub at: f64,
    pub body: Body,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Body {
    /// The step under way, replacing the last.
    Step { text: String },
    /// The steps after it, replacing the last list.
    Next { steps: Vec<String> },
    /// A timer from `at`, which may expect a device event (a UART record
    /// prefix such as `PLFW1 start`) before it runs out.
    Timer {
        label: String,
        seconds: f64,
        expect: Option<String>,
    },
    /// The timer of this label is done with.
    Clear { label: String },
    /// Evidence for a card: what was seen, where it is, and the verdict.
    Evidence {
        card: String,
        verdict: Verdict,
        text: String,
        path: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Pass,
    Fail,
}

/// A timer still running or run out, as the notes leave it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Timer {
    pub label: String,
    pub started: f64,
    pub seconds: f64,
    pub expect: Option<String>,
}

/// One card's evidence, in the order it was noted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
// The page's types already have a run record's `Evidence`.
#[cfg_attr(feature = "ts", ts(rename = "NoteEvidence"))]
pub struct Evidence {
    pub at: f64,
    pub verdict: Verdict,
    pub text: String,
    pub path: Option<String>,
}

/// What the notes say now.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Operator {
    pub step: Option<String>,
    pub next: Vec<String>,
    pub timers: Vec<Timer>,
    /// By card, in the order each card was first noted.
    pub evidence: Vec<(String, Vec<Evidence>)>,
}

impl Operator {
    /// Replay the notes in order.
    pub fn fold<'a>(notes: impl IntoIterator<Item = &'a Note>) -> Self {
        let mut operator = Self::default();
        for note in notes {
            operator.apply(note);
        }
        operator
    }

    pub fn apply(&mut self, note: &Note) {
        match &note.body {
            Body::Step { text } => self.step = Some(text.clone()),
            Body::Next { steps } => self.next.clone_from(steps),

            Body::Timer {
                label,
                seconds,
                expect,
            } => {
                // A timer restarted under the same label replaces itself.
                self.timers.retain(|timer| timer.label != *label);
                self.timers.push(Timer {
                    label: label.clone(),
                    started: note.at,
                    seconds: *seconds,
                    expect: expect.clone(),
                });
            }
            Body::Clear { label } => self.timers.retain(|timer| timer.label != *label),

            Body::Evidence {
                card,
                verdict,
                text,
                path,
            } => {
                let evidence = Evidence {
                    at: note.at,
                    verdict: *verdict,
                    text: text.clone(),
                    path: path.clone(),
                };
                match self.evidence.iter_mut().find(|(name, _)| name == card) {
                    Some((_, list)) => list.push(evidence),
                    None => self.evidence.push((card.clone(), vec![evidence])),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(at: f64, body: Body) -> Note {
        Note { at, body }
    }

    #[test]
    fn the_notes_fold_into_the_step_timers_and_evidence() {
        let notes = [
            note(
                1.0,
                Body::Step {
                    text: "boot".into(),
                },
            ),
            note(
                2.0,
                Body::Next {
                    steps: vec!["ota".into(), "itest".into()],
                },
            ),
            note(
                3.0,
                Body::Timer {
                    label: "boot".into(),
                    seconds: 60.0,
                    expect: Some("PLFW1 start".into()),
                },
            ),
            note(
                4.0,
                Body::Timer {
                    label: "soak".into(),
                    seconds: 600.0,
                    expect: None,
                },
            ),
            note(
                5.0,
                Body::Clear {
                    label: "boot".into(),
                },
            ),
            note(
                6.0,
                Body::Evidence {
                    card: "QA7".into(),
                    verdict: Verdict::Pass,
                    text: "booted".into(),
                    path: None,
                },
            ),
            note(
                7.0,
                Body::Evidence {
                    card: "QA7".into(),
                    verdict: Verdict::Fail,
                    text: "no status".into(),
                    path: Some("private/captures/x".into()),
                },
            ),
        ];

        let operator = Operator::fold(&notes);

        assert_eq!(operator.step.as_deref(), Some("boot"));
        assert_eq!(operator.next, ["ota", "itest"]);
        assert_eq!(operator.timers.len(), 1);
        assert_eq!(
            (
                operator.timers[0].label.as_str(),
                operator.timers[0].started
            ),
            ("soak", 4.0)
        );
        let (card, evidence) = &operator.evidence[0];
        assert_eq!(card, "QA7");
        assert_eq!(
            evidence.iter().map(|e| e.verdict).collect::<Vec<_>>(),
            [Verdict::Pass, Verdict::Fail]
        );
    }

    #[test]
    fn a_note_reads_back_from_its_line() {
        let line = note(
            1.0,
            Body::Timer {
                label: "t".into(),
                seconds: 5.0,
                expect: None,
            },
        );

        let text = serde_json::to_string(&line).unwrap();

        assert!(text.contains(r#""kind":"timer""#), "{text}");
        assert_eq!(serde_json::from_str::<Note>(&text).unwrap(), line);
    }
}
