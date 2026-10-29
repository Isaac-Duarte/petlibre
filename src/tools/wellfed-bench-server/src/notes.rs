//! The operator's notes (`wellfed_formats::bench::notes`): appended from
//! `POST /notes`, folded into the live state, and the timers settled as
//! what they expect arrives.
//!
//! Lines settle a timer from the capture, the device log and the running
//! job's output; device events from the feeder session. Each match or
//! miss is a note of its own, so a restarted server folds the same state.
//! The file is append-only, written through a buffer and never synced; it
//! is not flushed while the flash client holds the UART, so a note during
//! a write costs the write no disk activity.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::broadcast;

use wellfed_formats::bench::live::Uart;
use wellfed_formats::bench::notes::{Body, Expect, NOTES, Note, Operator, Seen};
use wellfed_formats::bench::now;
use wellfed_model::EventKind;

use crate::api::NoteError;
use crate::state::Store;

/// The notes, shared by the socket, the capture, the session and the
/// output pump.
#[derive(Clone)]
pub(crate) struct Notes {
    inner: Arc<Mutex<Inner>>,
    store: Store,
    /// Every line and event, for `GET /wait`; sent only while one listens.
    heard: broadcast::Sender<Heard>,
}

/// A line or a device event's kind, as [`Notes::seen`] got it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Heard {
    Line(String),
    Event(String),
}

/// How far a slow listener may fall behind before it misses lines.
const HEARD: usize = 1024;

struct Inner {
    path: PathBuf,
    /// The source of truth; the live state holds a copy.
    operator: Operator,
    /// Opened at the first note, so a server that takes none creates no
    /// file.
    file: Option<BufWriter<File>>,
}

impl Notes {
    /// The notes in the checkout at `root`, replayed from the start; a line
    /// that does not read is skipped, as the page skips one.
    pub(crate) fn open(root: &Path, store: Store) -> Self {
        let path = root.join(NOTES);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let notes: Vec<Note> = text
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        let operator = Operator::fold(&notes);

        store.update(|state| state.operator = operator.clone());
        Self {
            inner: Arc::new(Mutex::new(Inner {
                path,
                operator,
                file: None,
            })),
            store,
            heard: broadcast::channel(HEARD).0,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A client's note, checked, stamped now and kept.
    pub(crate) fn note(&self, body: Body) -> Result<Note, NoteError> {
        check(&body)?;

        let note = Note { at: now(), body };
        let mut inner = self.lock();
        self.append(&mut inner, &note)?;
        Ok(note)
    }

    /// Every line and event from now on.
    pub(crate) fn listen(&self) -> broadcast::Receiver<Heard> {
        self.heard.subscribe()
    }

    /// Settle every waiting timer that `seen` matches, and tell whoever
    /// waits for it.
    pub(crate) fn seen(&self, seen: &Seen<'_>) {
        if self.heard.receiver_count() > 0 {
            let heard = match seen {
                Seen::Line(line) => Heard::Line((*line).to_string()),
                Seen::Event(kind) => Heard::Event((*kind).to_string()),
            };
            let _ = self.heard.send(heard);
        }

        let at = now();
        let mut inner = self.lock();
        for body in inner.operator.seen(seen, at) {
            // A note the disk refuses is still in the state.
            let _ = self.append(&mut inner, &Note { at, body });
        }
    }

    /// Mark missed every timer whose window has passed.
    pub(crate) fn tick(&self) {
        let at = now();
        let mut inner = self.lock();
        for body in inner.operator.missed(at) {
            let _ = self.append(&mut inner, &Note { at, body });
        }
    }

    /// Write out what the buffer holds, once the UART is free again.
    pub(crate) fn flush(&self) {
        if let Some(file) = self.lock().file.as_mut() {
            let _ = file.flush();
        }
    }

    fn append(&self, inner: &mut Inner, note: &Note) -> Result<(), NoteError> {
        inner.operator.apply(note);
        let operator = inner.operator.clone();
        self.store.update(|state| state.operator = operator);

        let written = Self::write(inner, note);
        let held = self
            .store
            .with(|state| matches!(state.uart, Uart::Writing { .. } | Uart::Loader));
        let flushed = match inner.file.as_mut() {
            Some(file) if !held => file.flush(),
            _ => Ok(()),
        };
        written.and(flushed).map_err(|error| NoteError::Write {
            path: NOTES.into(),
            error: error.to_string(),
        })
    }

    fn write(inner: &mut Inner, note: &Note) -> std::io::Result<()> {
        if inner.file.is_none() {
            if let Some(parent) = inner.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&inner.path)?;
            inner.file = Some(BufWriter::new(file));
        }
        let Some(file) = inner.file.as_mut() else {
            return Ok(());
        };
        let line = serde_json::to_string(note).map_err(std::io::Error::other)?;
        writeln!(file, "{line}")
    }
}

/// What a client may note: never a server's own settlement, a timer whose
/// window ends before it is due, an expectation that matches everything,
/// an event kind the device API does not have, or evidence outside
/// `private/`.
fn check(body: &Body) -> Result<(), NoteError> {
    match body {
        Body::Seen { .. } => Err(NoteError::ServerOnly {
            kind: "seen".into(),
        }),
        Body::Missed { .. } => Err(NoteError::ServerOnly {
            kind: "missed".into(),
        }),

        Body::Timer {
            label,
            due,
            until,
            expect,
        } => {
            wellfed_formats::require(!label.trim().is_empty(), || NoteError::NoLabel)?;
            wellfed_formats::require(due.is_finite() && until.is_finite() && until >= due, || {
                NoteError::Window {
                    label: label.clone(),
                }
            })?;
            match expect {
                Some(Expect::Line { text }) if text.is_empty() => Err(NoteError::EmptyExpect {
                    label: label.clone(),
                }),
                Some(Expect::Event { kind }) => event_kind(kind).map(|_| ()),
                _ => Ok(()),
            }
        }
        Body::Clear { label } => {
            wellfed_formats::require(!label.trim().is_empty(), || NoteError::NoLabel)
        }

        Body::Evidence {
            path: Some(path), ..
        } => under_private(path),
        Body::Step { .. } | Body::Next { .. } | Body::Evidence { path: None, .. } => Ok(()),
    }
}

/// A root-relative path inside `private/`, with no way out of it.
fn under_private(path: &str) -> Result<(), NoteError> {
    let path = Path::new(path);
    let mut components = path.components();
    let inside = components.next() == Some(Component::Normal("private".as_ref()))
        && components.all(|component| matches!(component, Component::Normal(_)))
        && path.components().count() > 1;
    wellfed_formats::require(inside, || NoteError::OutsidePrivate {
        path: path.display().to_string(),
    })
}

/// The device API's name for `kind`, as its events carry it.
pub(crate) fn event_name(kind: EventKind) -> String {
    match serde_json::to_value(kind) {
        Ok(serde_json::Value::String(name)) => name,
        _ => format!("{kind:?}"),
    }
}

/// The event kind a note names, or the refusal that lists them.
pub(crate) fn event_kind(name: &str) -> Result<EventKind, NoteError> {
    serde_json::from_value(serde_json::Value::String(name.to_string())).map_err(|_| {
        NoteError::UnknownEvent {
            kind: name.to_string(),
            known: EventKind::ALL
                .iter()
                .map(|kind| event_name(*kind))
                .collect::<Vec<_>>()
                .join(", "),
        }
    })
}

#[cfg(test)]
mod tests {
    use wellfed_formats::bench::live::BenchState;
    use wellfed_formats::bench::notes::{TimerState, Verdict, WINDOW};

    use super::*;

    fn notes() -> (tempfile::TempDir, Notes, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(crate::tests::idle_state());
        let notes = Notes::open(dir.path(), store.clone());
        (dir, notes, store)
    }

    fn timer(label: &str, expect: Expect) -> Body {
        let due = now() + 60.0;
        Body::Timer {
            label: label.into(),
            due,
            until: due + WINDOW,
            expect: Some(expect),
        }
    }

    fn operator(store: &Store) -> Operator {
        store.with(|state: &BenchState| state.operator.clone())
    }

    #[test]
    fn a_note_folds_into_the_state_and_reads_back_after_a_restart() {
        let (dir, notes, store) = notes();

        notes
            .note(Body::Step {
                text: "Install the candidate".into(),
            })
            .unwrap();
        notes
            .note(Body::Evidence {
                card: "schedule fire (free text)".into(),
                verdict: Verdict::Pass,
                text: "banner seen".into(),
                path: Some("private/captures/boot-01".into()),
            })
            .unwrap();
        notes.flush();

        assert_eq!(
            operator(&store).step.as_deref(),
            Some("Install the candidate")
        );
        let again = Store::new(crate::tests::idle_state());
        Notes::open(dir.path(), again.clone());
        assert_eq!(operator(&again), operator(&store));
    }

    #[test]
    fn evidence_outside_private_and_the_server_s_own_notes_are_refused() {
        let (_dir, notes, _store) = notes();
        let evidence = |path: &str| Body::Evidence {
            card: "QA1".into(),
            verdict: Verdict::Fail,
            text: String::new(),
            path: Some(path.into()),
        };

        for path in ["/etc/passwd", "private/../src", "notes/private", "private"] {
            assert!(
                matches!(
                    notes.note(evidence(path)),
                    Err(NoteError::OutsidePrivate { .. })
                ),
                "{path}"
            );
        }
        assert!(matches!(
            notes.note(Body::Missed { label: "x".into() }),
            Err(NoteError::ServerOnly { .. })
        ));
        assert!(matches!(
            notes.note(timer(
                "x",
                Expect::Event {
                    kind: "feed".into()
                }
            )),
            Err(NoteError::UnknownEvent { .. })
        ));
    }

    #[test]
    fn a_uart_line_and_a_device_event_settle_their_timers() {
        let (_dir, notes, store) = notes();
        notes
            .note(timer(
                "fire",
                Expect::Line {
                    text: "PLSCH1 fire".into(),
                },
            ))
            .unwrap();
        notes
            .note(timer(
                "boot",
                Expect::Event {
                    kind: "boot".into(),
                },
            ))
            .unwrap();

        notes.seen(&Seen::Line("PLSCH1 skip slot=0"));
        notes.seen(&Seen::Line("[12.5] PLSCH1 fire slot=0 portions=2"));
        notes.seen(&Seen::Event(&event_name(EventKind::Boot)));

        let timers = operator(&store).timers;
        assert!(
            matches!(&timers[0].state, TimerState::Seen { line, .. } if line.contains("slot=0")),
            "{timers:?}"
        );
        assert!(
            matches!(timers[1].state, TimerState::Seen { .. }),
            "{timers:?}"
        );
    }

    #[test]
    fn a_window_that_passes_with_no_match_is_missed() {
        let (_dir, notes, store) = notes();
        let at = now();
        notes
            .note(Body::Timer {
                label: "fire".into(),
                due: at - 10.0,
                until: at - 1.0,
                expect: Some(Expect::Line {
                    text: "PLSCH1 fire".into(),
                }),
            })
            .unwrap();

        notes.tick();
        notes.seen(&Seen::Line("PLSCH1 fire"));

        assert!(matches!(
            operator(&store).timers[0].state,
            TimerState::Missed { .. }
        ));
    }
}
