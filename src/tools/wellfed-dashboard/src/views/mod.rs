//! Read-only views of the evidence directories and the command history, as
//! the page reads them.
//!
//! Each public view backs one of the page's API routes in the parent
//! module, which runs it on the blocking pool; each route's view has its own
//! submodule, and this module holds what they share. Field names and shapes
//! are the page's contract, through the generated TypeScript types.
//!
//! Nothing here opens a serial port or writes a file. Every evidence path
//! from a record must stay inside its private/ folder; others are ignored.

pub(super) mod evidence;
pub(super) mod health;
mod qa;
mod script;
pub(super) mod sessions;
pub(super) mod tail;

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use crate::Live;
use wellfed_formats::bench::progress::{Current, HISTORY, HistoryEntry};
use wellfed_formats::bench::{CAPTURES, STATUS_DIR};

/// Where the views look: the checkout root, the flash client in use, and
/// the bench server's live state when it serves the page.
#[derive(Clone)]
pub(crate) struct Sources {
    pub root: PathBuf,
    pub flash_bin: PathBuf,
    pub live: Arc<dyn Live>,
}

impl Sources {
    fn status(&self) -> PathBuf {
        self.root.join(STATUS_DIR)
    }

    /// The running job's record, from its first step until it ends.
    fn running(&self) -> Option<Current> {
        let job = self.live.subscribe().borrow().job.clone()?;
        job.end.is_none().then_some(job.record).flatten()
    }
}

pub(super) fn modified(path: &Path) -> Option<f64> {
    let time = fs::metadata(path).ok()?.modified().ok()?;
    Some(time.duration_since(UNIX_EPOCH).ok()?.as_secs_f64())
}

/// `relative` under `base` (both under `root`), with no escape by `..` or links.
pub(super) fn inside(root: &Path, base: &str, relative: &str) -> Option<PathBuf> {
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

/// A JSON file under the root, if it reads as `T`; the views show what is
/// there and skip what is not.
pub(super) fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    wellfed_formats::load(path).ok()
}

fn history(sources: &Sources) -> Vec<HistoryEntry> {
    fs::read_to_string(sources.status().join(HISTORY))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
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

/// This module's page types, for [`super::bindings`].
#[cfg(test)]
pub(super) fn bindings(cfg: &ts_rs::Config) -> Vec<String> {
    let mut decls = tail::bindings(cfg);
    decls.extend(evidence::bindings(cfg));
    decls.extend(script::bindings(cfg));
    decls.extend(qa::bindings(cfg));
    decls.extend(sessions::bindings(cfg));
    decls.extend(health::bindings(cfg));
    decls
}

#[cfg(test)]
pub(crate) mod tests {
    use serde::Serialize;
    use serde_json::{Value, json};

    use super::evidence::evidence;
    use super::sessions::sessions;
    use super::*;
    use tokio::sync::watch;
    use wellfed_formats::bench::live::{
        BenchState, CancelState, JobEnd, JobId, JobView, Phase, ServerView, Uart,
    };
    use wellfed_formats::bench::progress::{Outcome, ProgressSink, Status};

    /// What a bench server would hand the page, set by the test.
    pub(crate) struct Recorded {
        pub state: watch::Sender<BenchState>,
    }

    impl Default for Recorded {
        fn default() -> Self {
            Self {
                state: watch::Sender::new(idle()),
            }
        }
    }

    /// A server with no job yet.
    pub(crate) fn idle() -> BenchState {
        BenchState {
            now: 0.0,
            server: ServerView {
                pid: 1,
                started: 0.0,
                phase: Phase::Serving,
            },
            job: None,
            activity: Vec::new(),
            uart: Uart::Idle,
            relays: Default::default(),
            banks: Default::default(),
            operator: Default::default(),
        }
    }

    impl Live for Recorded {
        fn subscribe(&self) -> watch::Receiver<BenchState> {
            self.state.subscribe()
        }

        fn output(&self, _: JobId) -> Option<Arc<dyn crate::JobOutput>> {
            None
        }
    }

    /// The record of a job, as the server's sink publishes it: its end
    /// with its outcome.
    struct Publish(Arc<Recorded>);

    impl ProgressSink for Publish {
        fn publish(&mut self, current: &Current) {
            self.0.state.send_modify(|state| {
                let job = state.job.get_or_insert_with(|| JobView {
                    id: 1,
                    label: current.command.word().into(),
                    log: String::new(),
                    accepted: current.started,
                    record: None,
                    upcoming: Vec::new(),
                    cancel: CancelState::None,
                    end: None,
                });
                job.update(current);
                job.end = (current.outcome != Outcome::Running).then(|| JobEnd {
                    code: 0,
                    error: None,
                    finished: current.finished.unwrap_or(current.started),
                });
            });
        }
    }

    impl Recorded {
        /// A progress recorder whose record the page then reads.
        pub(crate) fn status(self: &Arc<Self>, sources: &Sources) -> Status {
            Status::new(&sources.root, sources.status())
                .with_sink(Box::new(Publish(Arc::clone(self))))
        }
    }

    pub(crate) fn sources() -> (tempfile::TempDir, Sources) {
        let (tmp, sources, _) = live_sources();
        (tmp, sources)
    }

    /// Sources with a live state the test sets.
    pub(crate) fn live_sources() -> (tempfile::TempDir, Sources, Arc<Recorded>) {
        let tmp = tempfile::tempdir().unwrap();
        let live = Arc::new(Recorded::default());
        let sources = Sources {
            root: tmp.path().to_path_buf(),
            flash_bin: tmp.path().join("wellfed-flash"),
            live: Arc::clone(&live) as Arc<dyn Live>,
        };
        (tmp, sources, live)
    }

    /// A view as the page receives it.
    pub(super) fn json(view: impl Serialize) -> Value {
        serde_json::to_value(view).unwrap()
    }

    #[test]
    fn empty() {
        let (_tmp, sources) = sources();

        let view = json(evidence(&sources));
        assert_eq!(view["history"], json!([]));
        assert!(view["rx"].is_null());
        assert!(view["script"].is_null());
        assert!(view["qa"].is_null());
        assert_eq!(json(sessions(&sources))["sessions"], json!([]));
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
}
