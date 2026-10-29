//! `private/bench-status/notes.jsonl`: what the operator records with
//! `wellfed-bench note`, one line each: the step under way, the steps
//! after it, timers (each with what it expects to see, if anything) and
//! evidence by card with a verdict. The server adds a line when a timer's
//! expectation is seen, or its window passes without it.
//!
//! The file is append-only; [`Operator::fold`] replays it into what the
//! page and the console show now.

use serde::{Deserialize, Serialize};

/// Root-relative: the operator's notes.
pub const NOTES: &str = "private/bench-status/notes.jsonl";

/// How long after its due time a timer's expectation may still be seen,
/// unless the note says otherwise.
pub const WINDOW: f64 = 120.0;

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
    /// A timer from `at`, due at `due`. One that expects something is seen
    /// by the first match from `at` until `until`, and missed after.
    Timer {
        label: String,
        due: f64,
        until: f64,
        expect: Option<Expect>,
    },
    /// The timer of this label is done with.
    Clear { label: String },
    /// Written by the server: the timer's expectation arrived, as `line`.
    Seen { label: String, line: String },
    /// Written by the server: the timer's window passed with no match.
    Missed { label: String },
    /// Evidence for a card: what was seen, where it is, and the verdict.
    /// A card is the operator's own name for the work, free text here.
    Evidence {
        card: String,
        verdict: Verdict,
        text: String,
        /// Root-relative, under `private/`.
        path: Option<String>,
    },
}

/// What a timer waits for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "on", rename_all = "lowercase")]
pub enum Expect {
    /// A UART line or a device log line that contains `text`, such as
    /// `PLSCH1 fire`.
    Line { text: String },
    /// A device event of this kind, as the device API names it
    /// (`boot`, `fed`, `clock_sync`).
    Event { kind: String },
}

impl std::fmt::Display for Expect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Line { text } => write!(f, "{text:?}"),
            Self::Event { kind } => write!(f, "a {kind} event"),
        }
    }
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
    pub due: f64,
    /// The end of the window an expectation may be seen in.
    pub until: f64,
    pub expect: Option<Expect>,
    pub state: TimerState,
}

impl Timer {
    /// Whether `seen`, at `at`, settles this timer's expectation.
    pub fn wants(&self, seen: &Seen<'_>, at: f64) -> bool {
        let open = self.state == TimerState::Waiting && at >= self.started && at <= self.until;
        open && match (&self.expect, seen) {
            (Some(Expect::Line { text }), Seen::Line(line)) => line.contains(text.as_str()),
            (Some(Expect::Event { kind }), Seen::Event(seen)) => kind == seen,
            _ => false,
        }
    }

    /// Whether the window has passed with the expectation still unseen.
    pub fn overdue(&self, now: f64) -> bool {
        self.expect.is_some() && self.state == TimerState::Waiting && now > self.until
    }
}

/// Something that may settle a timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seen<'a> {
    /// A UART line, a device log line or a job's output line.
    Line(&'a str),
    /// A device event's kind.
    Event(&'a str),
}

/// Whether a timer's expectation has been settled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum TimerState {
    /// Running, or a timer that expects nothing.
    Waiting,
    Seen {
        at: f64,
        line: String,
    },
    Missed {
        at: f64,
    },
}

/// One card's evidence, in the order it was noted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CardEvidence {
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
    pub evidence: Vec<(String, Vec<CardEvidence>)>,
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
                due,
                until,
                expect,
            } => {
                // A timer restarted under the same label replaces itself.
                self.timers.retain(|timer| timer.label != *label);
                self.timers.push(Timer {
                    label: label.clone(),
                    started: note.at,
                    due: *due,
                    until: *until,
                    expect: expect.clone(),
                    state: TimerState::Waiting,
                });
            }
            Body::Clear { label } => self.timers.retain(|timer| timer.label != *label),
            Body::Seen { label, line } => self.settle(
                label,
                TimerState::Seen {
                    at: note.at,
                    line: line.clone(),
                },
            ),
            Body::Missed { label } => self.settle(label, TimerState::Missed { at: note.at }),

            Body::Evidence {
                card,
                verdict,
                text,
                path,
            } => {
                let evidence = CardEvidence {
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

    /// Only a waiting timer settles: the first match or miss is the one
    /// the notes keep.
    fn settle(&mut self, label: &str, state: TimerState) {
        if let Some(timer) = self
            .timers
            .iter_mut()
            .find(|timer| timer.label == label && timer.state == TimerState::Waiting)
        {
            timer.state = state;
        }
    }

    /// The notes that `seen`, at `at`, adds: one `seen` for every waiting
    /// timer it settles.
    pub fn seen(&self, seen: &Seen<'_>, at: f64) -> Vec<Body> {
        if !self.expecting() {
            return Vec::new();
        }
        let line = match seen {
            Seen::Line(line) => (*line).to_string(),
            Seen::Event(kind) => format!("{kind} event"),
        };
        self.timers
            .iter()
            .filter(|timer| timer.wants(seen, at))
            .map(|timer| Body::Seen {
                label: timer.label.clone(),
                line: line.clone(),
            })
            .collect()
    }

    /// Whether any timer still waits for a line or an event: while none
    /// does, nothing seen can settle one.
    pub fn expecting(&self) -> bool {
        self.timers
            .iter()
            .any(|timer| timer.expect.is_some() && timer.state == TimerState::Waiting)
    }

    /// The notes for every timer whose window has passed unseen by `now`.
    pub fn missed(&self, now: f64) -> Vec<Body> {
        self.timers
            .iter()
            .filter(|timer| timer.overdue(now))
            .map(|timer| Body::Missed {
                label: timer.label.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(at: f64, body: Body) -> Note {
        Note { at, body }
    }

    fn timer(label: &str, due: f64, expect: Option<Expect>) -> Body {
        Body::Timer {
            label: label.into(),
            due,
            until: due + WINDOW,
            expect,
        }
    }

    fn line(text: &str) -> Option<Expect> {
        Some(Expect::Line { text: text.into() })
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
            note(3.0, timer("boot", 63.0, line("PLFW1 start"))),
            note(4.0, timer("soak", 604.0, None)),
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
    fn a_line_or_event_in_the_window_settles_its_timer_once() {
        let mut operator = Operator::fold(&[
            note(10.0, timer("fire", 70.0, line("PLSCH1 fire"))),
            note(
                10.0,
                timer(
                    "boot",
                    20.0,
                    Some(Expect::Event {
                        kind: "boot".into(),
                    }),
                ),
            ),
        ]);

        // Before the timer started, and a line that does not match.
        assert!(
            operator
                .seen(&Seen::Line("PLSCH1 fire slot=0"), 5.0)
                .is_empty()
        );
        assert!(operator.seen(&Seen::Line("PLSCH1 skip"), 30.0).is_empty());
        let fired = operator.seen(&Seen::Line("12:00 PLSCH1 fire slot=0"), 30.0);
        assert_eq!(
            fired,
            [Body::Seen {
                label: "fire".into(),
                line: "12:00 PLSCH1 fire slot=0".into()
            }]
        );
        for body in fired {
            operator.apply(&note(30.0, body));
        }

        assert!(
            operator
                .seen(&Seen::Line("PLSCH1 fire slot=1"), 31.0)
                .is_empty()
        );
        assert_eq!(operator.seen(&Seen::Event("boot"), 31.0).len(), 1);
        assert!(matches!(
            operator.timers[0].state,
            TimerState::Seen { at: 30.0, .. }
        ));
    }

    #[test]
    fn a_window_that_passes_unseen_is_missed_and_a_plain_timer_never_is() {
        let mut operator = Operator::fold(&[
            note(0.0, timer("fire", 60.0, line("PLSCH1 fire"))),
            note(0.0, timer("soak", 60.0, None)),
        ]);

        assert!(operator.missed(60.0 + WINDOW).is_empty());
        let missed = operator.missed(60.0 + WINDOW + 1.0);
        assert_eq!(
            missed,
            [Body::Missed {
                label: "fire".into()
            }]
        );
        for body in missed {
            operator.apply(&note(200.0, body));
        }

        assert_eq!(operator.timers[0].state, TimerState::Missed { at: 200.0 });
        assert!(operator.missed(1_000.0).is_empty());
        // Late now, so the line no longer counts.
        assert!(operator.seen(&Seen::Line("PLSCH1 fire"), 201.0).is_empty());
    }

    #[test]
    fn a_note_reads_back_from_its_line() {
        let line = note(1.0, timer("t", 5.0, line("PLFW1 start")));

        let text = serde_json::to_string(&line).unwrap();

        assert!(text.contains(r#""kind":"timer""#), "{text}");
        assert!(text.contains(r#""on":"line""#), "{text}");
        assert_eq!(serde_json::from_str::<Note>(&text).unwrap(), line);
    }
}
