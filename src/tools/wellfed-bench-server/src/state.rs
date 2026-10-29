//! The server's memory: the live state every reader follows, and each
//! job's output for its stream.
//!
//! Both are written from plain threads (a job, the output pump, the relay
//! board) and read from the socket's tasks, through tokio `watch` channels,
//! whose senders never wait on a reader.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::watch;
use wellfed_formats::bench::live::{BenchState, JobEnd, JobId};
use wellfed_formats::bench::now;

/// The live state, shared.
#[derive(Clone)]
pub struct Store {
    sender: Arc<watch::Sender<BenchState>>,
}

impl Store {
    pub(crate) fn new(state: BenchState) -> Self {
        Self {
            sender: Arc::new(watch::Sender::new(state)),
        }
    }

    /// The state as of now.
    pub fn read(&self) -> BenchState {
        let mut state = self.sender.borrow().clone();
        state.now = now();
        state
    }

    /// A receiver that wakes on every change.
    pub fn subscribe(&self) -> watch::Receiver<BenchState> {
        self.sender.subscribe()
    }

    pub(crate) fn update(&self, change: impl FnOnce(&mut BenchState)) {
        self.sender.send_modify(change);
    }
}

/// The most lines one job's stream keeps; older ones are dropped, so a
/// reattaching client sees the newest.
const KEPT_LINES: usize = 50_000;

/// One job's output, in order, and how it ended.
pub(crate) struct JobLog {
    inner: Mutex<Lines>,
    /// Bumped on every line and at the end.
    changed: watch::Sender<u64>,
}

#[derive(Default)]
struct Lines {
    /// The index of `lines[0]` since the job began.
    first: usize,
    lines: VecDeque<String>,
    end: Option<JobEnd>,
}

/// What a reader at `cursor` has not seen yet.
pub(crate) struct Unread {
    pub(crate) lines: Vec<String>,
    /// Where the reader is after these lines.
    pub(crate) cursor: usize,
    /// Set once every line is read and the job has ended.
    pub(crate) end: Option<JobEnd>,
}

impl JobLog {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(Lines::default()),
            changed: watch::Sender::new(0),
        }
    }

    pub(crate) fn push(&self, line: String) {
        {
            let mut inner = self.lock();
            inner.lines.push_back(line);
            if inner.lines.len() > KEPT_LINES {
                inner.lines.pop_front();
                inner.first += 1;
            }
        }
        self.changed.send_modify(|n| *n += 1);
    }

    pub(crate) fn finish(&self, end: JobEnd) {
        self.lock().end = Some(end);
        self.changed.send_modify(|n| *n += 1);
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    pub(crate) fn read_from(&self, cursor: usize) -> Unread {
        let inner = self.lock();
        let skip = cursor.saturating_sub(inner.first);
        let lines: Vec<String> = inner.lines.iter().skip(skip).cloned().collect();
        let cursor = inner.first + skip + lines.len();
        Unread {
            lines,
            cursor,
            end: inner.end.clone(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Lines> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// How many finished jobs keep their output for `attach`.
const KEPT_JOBS: usize = 8;

/// The output of the running job and of the last few, by id.
#[derive(Default)]
pub(crate) struct Logs {
    jobs: VecDeque<(JobId, Arc<JobLog>)>,
}

impl Logs {
    pub(crate) fn add(&mut self, id: JobId) -> Arc<JobLog> {
        let log = Arc::new(JobLog::new());
        self.jobs.push_back((id, Arc::clone(&log)));
        while self.jobs.len() > KEPT_JOBS {
            self.jobs.pop_front();
        }
        log
    }

    pub(crate) fn get(&self, id: JobId) -> Option<Arc<JobLog>> {
        self.jobs
            .iter()
            .find(|(job, _)| *job == id)
            .map(|(_, log)| Arc::clone(log))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reader_gets_each_line_once_then_the_end() {
        let log = JobLog::new();
        log.push("one".into());
        log.push("two".into());

        let first = log.read_from(0);
        assert_eq!((first.lines.len(), first.cursor), (2, 2));
        assert!(first.end.is_none());

        log.push("three".into());
        log.finish(JobEnd {
            code: 0,
            error: None,
            finished: 1.0,
        });
        let rest = log.read_from(first.cursor);
        assert_eq!(rest.lines, ["three"]);
        assert_eq!(rest.end.map(|end| end.code), Some(0));
    }

    #[test]
    fn a_reader_that_fell_behind_resumes_at_the_oldest_kept_line() {
        let log = JobLog::new();
        for n in 0..KEPT_LINES + 2 {
            log.push(n.to_string());
        }

        let unread = log.read_from(0);

        assert_eq!(unread.lines.first().map(String::as_str), Some("2"));
        assert_eq!(unread.cursor, KEPT_LINES + 2);
    }

    #[test]
    fn only_the_newest_jobs_keep_their_output() {
        let mut logs = Logs::default();
        for id in 1..=10 {
            logs.add(id);
        }

        assert!(logs.get(2).is_none());
        assert!(logs.get(3).is_some() && logs.get(10).is_some());
    }
}
