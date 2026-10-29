//! The server's own stdout and stderr, which a job's output goes through.
//!
//! The rig's sequences print with `say!` and `eprintln!`, and the tools they
//! run inherit the process's stderr. Rather than change any of that, file
//! descriptors 1 and 2 are pointed at one pipe, and a pump thread reads it
//! line by line: each line goes to the console (the terminal `serve` was
//! started in) and, while a job runs, into that job's stream.
//!
//! The pump never stops draining. A full pipe would block whichever thread
//! or child wrote next, which during an install is the flash client. So the
//! console is written from a thread of its own through a bounded queue, and
//! a console that cannot keep up loses lines rather than holding the pipe.

use std::fs::File;
use std::io::{self, BufRead, BufReader, PipeReader, PipeWriter, Write};
use std::os::fd::AsFd;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::state::JobLog;

/// Lines the console may fall behind by before it loses some.
const CONSOLE_QUEUE: usize = 4096;
/// The longest [`Output::sync`] waits for the pump to catch up.
const SYNC_WAIT: Duration = Duration::from_secs(2);
/// A line the pump consumes rather than forwards: [`Output::sync`]'s mark.
/// It starts with a control character no tool prints.
const MARK: &str = "\u{1}wellfed-bench-server sync ";

/// The installed pipe, for the process's life.
pub(crate) struct Output {
    console: SyncSender<String>,
    /// The job whose stream the lines join, while one runs.
    route: Mutex<Option<Arc<JobLog>>>,
    /// The pipe's own write end, for marks.
    writer: Mutex<PipeWriter>,
    next_mark: AtomicU64,
    seen: Mutex<u64>,
    caught_up: Condvar,
}

/// Installed once per process: descriptors 1 and 2 are process-wide.
static INSTALLED: Mutex<Option<Arc<Output>>> = Mutex::new(None);

/// Point descriptors 1 and 2 at the pump, once; later calls get the same
/// pipe.
pub(crate) fn install() -> io::Result<Arc<Output>> {
    let mut installed = lock(&INSTALLED);
    if let Some(output) = &*installed {
        return Ok(Arc::clone(output));
    }

    // The console keeps the terminal the server was started on; the
    // duplicate is close-on-exec, so no child holds it.
    let console = File::from(io::stdout().as_fd().try_clone_to_owned()?);
    let (reader, writer) = io::pipe()?;
    io::stdout().flush()?;
    rustix::stdio::dup2_stdout(&writer)?;
    rustix::stdio::dup2_stderr(&writer)?;

    let (send, queue) = mpsc::sync_channel(CONSOLE_QUEUE);
    std::thread::Builder::new()
        .name("console".into())
        .spawn(move || write_console(console, &queue))?;
    let output = Arc::new(Output {
        console: send,
        route: Mutex::new(None),
        writer: Mutex::new(writer),
        next_mark: AtomicU64::new(0),
        seen: Mutex::new(0),
        caught_up: Condvar::new(),
    });
    let pumped = Arc::clone(&output);
    std::thread::Builder::new()
        .name("output pump".into())
        .spawn(move || pump(reader, &pumped))?;

    *installed = Some(Arc::clone(&output));
    Ok(output)
}

impl Output {
    /// A line for the console only: the server's own messages, which are
    /// no job's output.
    pub(crate) fn say(&self, line: String) {
        self.to_console(line);
    }

    /// Send every line from here on to `log`, or to the console alone.
    pub(crate) fn route(&self, log: Option<Arc<JobLog>>) {
        *lock(&self.route) = log;
    }

    /// Wait until the pump has read everything written to descriptors 1 and
    /// 2 before this call, so a finished job's last lines are in its stream
    /// before the stream ends.
    pub(crate) fn sync(&self) {
        let _ = io::stdout().flush();
        let mark = self.next_mark.fetch_add(1, Ordering::SeqCst) + 1;
        if writeln!(lock(&self.writer), "{MARK}{mark}").is_err() {
            return;
        }

        let deadline = Instant::now() + SYNC_WAIT;
        let mut seen = lock(&self.seen);
        while *seen < mark {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            seen = self
                .caught_up
                .wait_timeout(seen, left)
                .map_or_else(|poisoned| poisoned.into_inner().0, |(guard, _)| guard);
        }
    }

    fn to_console(&self, line: String) {
        // A full queue drops the line: the job's stream still has it.
        let _ = self.console.try_send(line);
    }

    fn marked(&self, mark: u64) {
        let mut seen = lock(&self.seen);
        *seen = (*seen).max(mark);
        self.caught_up.notify_all();
    }
}

/// Read the pipe until every writer is gone, which while descriptors 1 and
/// 2 point at it is never.
fn pump(reader: PipeReader, output: &Output) {
    let mut reader = BufReader::new(reader);
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        match reader.read_until(b'\n', &mut bytes) {
            Ok(0) => return,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return,
        }

        let line = String::from_utf8_lossy(&bytes)
            .trim_end_matches(['\n', '\r'])
            .to_string();
        if let Some(mark) = line.strip_prefix(MARK).and_then(|n| n.parse().ok()) {
            output.marked(mark);
            continue;
        }
        if let Some(log) = &*lock(&output.route) {
            log.push(line.clone());
        }
        output.to_console(line);
    }
}

fn write_console(mut console: File, queue: &mpsc::Receiver<String>) {
    for line in queue {
        // A closed terminal loses the console, never the server.
        let _ = writeln!(console, "{line}");
    }
}

/// What these locks guard stays whole if a holder panics.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
