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
//! Should the pump itself stop, the terminal (or else `/dev/null`) goes back
//! on descriptors 1 and 2, so no writer is left on a pipe nobody reads.

use std::fs::File;
use std::io::{self, BufRead, BufReader, PipeReader, PipeWriter, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::state::JobLog;

/// Lines the console may fall behind by before it loses some.
const CONSOLE_QUEUE: usize = 4096;
/// The longest [`Output::sync`] waits for the pump to catch up.
const SYNC_WAIT: Duration = Duration::from_secs(2);
/// What the pump consumes rather than forwards: [`Output::sync`]'s mark,
/// found anywhere in a line, so one written after a partial line still
/// counts. It starts with a control character no tool prints.
const MARK: &str = "\u{1}wellfed-bench-server sync ";

/// The installed pipe, for the process's life.
pub(crate) struct Output {
    /// Taken by [`Output::close`], which ends the console thread.
    console: Mutex<Option<SyncSender<String>>>,
    console_thread: Mutex<Option<JoinHandle<()>>>,
    /// The terminal the server started on, to put back on 1 and 2.
    terminal: OwnedFd,
    /// The job whose stream the lines join, while one runs.
    route: Mutex<Option<Arc<JobLog>>>,
    /// The pipe's own write end, for marks.
    writer: Mutex<PipeWriter>,
    next_mark: AtomicU64,
    seen: Mutex<u64>,
    caught_up: Condvar,
    /// Cleared if the pump stops, so nothing waits on it.
    pumping: AtomicBool,
    closed: AtomicBool,
}

/// Installed once per process: descriptors 1 and 2 are process-wide.
static INSTALLED: Mutex<Option<Arc<Output>>> = Mutex::new(None);

/// Point descriptors 1 and 2 at the pump, once; later calls get the same
/// pipe.
pub(crate) fn install() -> io::Result<Arc<Output>> {
    let mut installed = lock(&INSTALLED);
    if let Some(output) = &*installed
        && !output.closed.load(Ordering::SeqCst)
    {
        return Ok(Arc::clone(output));
    }

    let output = attach()?;
    *installed = Some(Arc::clone(&output));
    Ok(output)
}

/// Point descriptors 1 and 2 at a new pipe and start its pump.
fn attach() -> io::Result<Arc<Output>> {
    // Duplicates are close-on-exec, so no child holds the terminal.
    let terminal = io::stdout().as_fd().try_clone_to_owned()?;
    let console = File::from(terminal.try_clone()?);
    let (reader, writer) = io::pipe()?;
    io::stdout().flush()?;
    rustix::stdio::dup2_stdout(&writer)?;
    rustix::stdio::dup2_stderr(&writer)?;

    let (send, queue) = mpsc::sync_channel(CONSOLE_QUEUE);
    let console_thread = std::thread::Builder::new()
        .name("console".into())
        .spawn(move || write_console(console, &queue))?;
    let output = Arc::new(Output {
        console: Mutex::new(Some(send)),
        console_thread: Mutex::new(Some(console_thread)),
        terminal,
        route: Mutex::new(None),
        writer: Mutex::new(writer),
        next_mark: AtomicU64::new(0),
        seen: Mutex::new(0),
        caught_up: Condvar::new(),
        pumping: AtomicBool::new(true),
        closed: AtomicBool::new(false),
    });
    let pumped = Arc::clone(&output);
    std::thread::Builder::new()
        .name("output pump".into())
        .spawn(move || {
            let mut reader = BufReader::new(reader);
            let _ = catch_unwind(AssertUnwindSafe(|| pump(&mut reader, &pumped)));
            pumped.pump_stopped();
            // Only now, with nothing left writing to it, may the pipe
            // close: a writer never sees EPIPE.
            drop(reader);
        })?;
    Ok(output)
}

/// Put the terminal back on descriptors 1 and 2, and let the console write
/// out what it holds, if the pipe was installed. Safe to repeat.
pub(crate) fn close_installed() {
    let installed = lock(&INSTALLED).clone();
    if let Some(output) = installed {
        output.close();
    }
}

/// The terminal back on 1 and 2, or `/dev/null` if that fails: either way
/// no writer waits on the pipe.
fn restore(terminal: &OwnedFd) {
    let put =
        rustix::stdio::dup2_stdout(terminal).and_then(|()| rustix::stdio::dup2_stderr(terminal));
    if put.is_err()
        && let Ok(null) = File::options().write(true).open("/dev/null")
    {
        let _ = rustix::stdio::dup2_stdout(&null);
        let _ = rustix::stdio::dup2_stderr(&null);
    }
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
        if !self.pumping.load(Ordering::SeqCst) {
            return;
        }
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

    /// Put the terminal back on 1 and 2, pump through what the pipe still
    /// holds, then let the console thread write out its queue and end. A
    /// line the server prints after this, such as the error it stopped on,
    /// goes straight to the terminal.
    pub(crate) fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = io::stdout().flush();
        restore(&self.terminal);
        self.sync();

        drop(lock(&self.console).take());
        if let Some(thread) = lock(&self.console_thread).take() {
            let _ = thread.join();
        }
    }

    fn to_console(&self, line: String) {
        // A full queue drops the line: the job's stream still has it.
        if let Some(console) = &*lock(&self.console) {
            let _ = console.try_send(line);
        }
    }

    /// The pump has stopped (a read error or a panic): nothing waits on it
    /// any more, and the terminal goes back on 1 and 2 while the pipe is
    /// still open.
    fn pump_stopped(&self) {
        self.pumping.store(false, Ordering::SeqCst);
        self.marked(u64::MAX);
        restore(&self.terminal);
    }

    fn marked(&self, mark: u64) {
        let mut seen = lock(&self.seen);
        *seen = (*seen).max(mark);
        self.caught_up.notify_all();
    }

    fn forward(&self, line: String) {
        if let Some(log) = &*lock(&self.route) {
            log.push(line.clone());
        }
        self.to_console(line);
    }
}

/// Read the pipe until every writer is gone, which while descriptors 1 and
/// 2 point at it is never.
fn pump(reader: &mut BufReader<PipeReader>, output: &Output) {
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
        match line.find(MARK) {
            Some(at) => {
                let head = &line[..at];
                if !head.is_empty() {
                    output.forward(head.to_string());
                }
                if let Ok(mark) = line[at + MARK.len()..].parse() {
                    output.marked(mark);
                }
            }
            None => output.forward(line),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `body` with a file standing as the terminal on descriptors 1 and
    /// 2, then put the test's own back; what reached the file.
    fn on_a_terminal(body: impl FnOnce()) -> String {
        // Descriptors 1 and 2 are the process's: no server test may run
        // meanwhile.
        let _one = crate::tests::ONE_SERVER.blocking_lock();
        let terminal = tempfile::NamedTempFile::new().unwrap();
        let saved = (
            io::stdout().as_fd().try_clone_to_owned().unwrap(),
            io::stderr().as_fd().try_clone_to_owned().unwrap(),
        );
        rustix::stdio::dup2_stdout(terminal.as_file()).unwrap();
        rustix::stdio::dup2_stderr(terminal.as_file()).unwrap();

        body();

        rustix::stdio::dup2_stdout(&saved.0).unwrap();
        rustix::stdio::dup2_stderr(&saved.1).unwrap();
        std::fs::read_to_string(terminal.path()).unwrap()
    }

    // Written straight to the descriptors: the test harness captures the
    // print macros.
    fn print(line: &str) {
        writeln!(io::stdout(), "{line}").unwrap();
    }

    #[test]
    fn closing_writes_out_every_line_then_prints_straight_to_the_terminal() {
        let written = on_a_terminal(|| {
            let output = attach().unwrap();
            print("a job's last line");
            output.say("the server's last line".into());

            output.close();
            print("Stopped: after the close");
        });

        let lines: Vec<&str> = written.lines().collect();
        for line in [
            "a job's last line",
            "the server's last line",
            "Stopped: after the close",
        ] {
            assert!(lines.contains(&line), "{line:?} missing from {lines:?}");
        }
        assert_eq!(lines.last(), Some(&"Stopped: after the close"));
    }

    #[test]
    fn a_stopped_pump_puts_the_terminal_back_and_waits_on_nothing() {
        let written = on_a_terminal(|| {
            let output = attach().unwrap();

            output.pump_stopped();
            print("straight to the terminal");
            let started = Instant::now();
            output.sync();
            assert!(started.elapsed() < SYNC_WAIT);
        });

        assert!(written.contains("straight to the terminal"), "{written}");
    }

    #[test]
    fn a_mark_after_a_partial_line_still_counts() {
        let written = on_a_terminal(|| {
            let output = attach().unwrap();
            let started = Instant::now();

            write!(io::stdout(), "half a line").unwrap();
            io::stdout().flush().unwrap();
            output.sync();
            assert!(started.elapsed() < SYNC_WAIT, "the mark was missed");
            output.close();
        });

        assert!(
            written.lines().any(|line| line == "half a line"),
            "{written}"
        );
        assert!(!written.contains(MARK), "{written}");
    }
}
