//! The receive-only UART capture the server holds while no job runs and
//! relay 1 is on: the rig's capture chain (`wellfed-flash read observe`,
//! segment after segment), its lines read as they grow into the state's
//! tail, and the records the bench acts on parsed as they come.
//!
//! - `PLFW1 start build= bank=` checks the bank ledger, as the banner's
//!   witness.
//! - `PLOTA1` is kept as the last update record seen.
//!
//! A job never shares the port: the capture is stopped (its observe killed,
//! which only receives) before every job, and started again after it. A
//! chain that fails on its own (a missing port, a taken segment name)
//! stays stopped until the next job ends or the relays are read again.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use wellfed_core::record::{Event, Record, Tag};
use wellfed_formats::bench::banks::Witness;
use wellfed_formats::bench::live::{StartRecord, Uart, UartRecord, trim_tail};
use wellfed_formats::bench::{CAPTURES, now, relative};
use wellfed_rig::Cancel;
use wellfed_rig::tools::{Children, Processes, Tools};

use crate::banks::Banks;
use crate::jobs::ToolPaths;
use crate::state::Store;

/// Each observe's length; the flash client caps one at 180 s.
const SEGMENT: Duration = Duration::from_secs(180);
/// How often the growing segment is read.
const READ_EVERY: Duration = Duration::from_millis(250);
/// A longer run without a line break is noise, cut into a line.
const MAX_LINE: usize = 4096;

/// The capture, started and stopped by the server.
#[derive(Clone)]
pub(crate) struct CaptureService {
    inner: Arc<Inner>,
}

struct Inner {
    root: PathBuf,
    port: String,
    tools: ToolPaths,
    store: Store,
    banks: Banks,
    running: Mutex<Option<Running>>,
}

struct Running {
    stop: Cancel,
    thread: JoinHandle<()>,
}

impl CaptureService {
    pub(crate) fn new(
        root: PathBuf,
        port: String,
        tools: ToolPaths,
        store: Store,
        banks: Banks,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                root,
                port,
                tools,
                store,
                banks,
                running: Mutex::new(None),
            }),
        }
    }

    fn running(&self) -> MutexGuard<'_, Option<Running>> {
        self.inner
            .running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Start capturing unless a capture runs already. One that stopped on
    /// its own is started afresh.
    pub(crate) fn start(&self) {
        let mut running = self.running();
        if running
            .as_ref()
            .is_some_and(|running| !running.thread.is_finished())
        {
            return;
        }
        if let Some(finished) = running.take() {
            let _ = finished.thread.join();
        }

        let stop = Cancel::new();
        let inner = Arc::clone(&self.inner);
        let stopping = stop.clone();
        let thread = std::thread::Builder::new()
            .name("uart capture".into())
            .spawn(move || inner.capture(&stopping));
        match thread {
            Ok(thread) => *running = Some(Running { stop, thread }),
            Err(error) => self.inner.failed(format!("the capture's thread: {error}")),
        }
    }

    /// Stop the capture and wait until its observe has let go of the port.
    pub(crate) fn stop(&self) {
        let Some(running) = self.running().take() else {
            return;
        };
        running.stop.cancel();
        let _ = running.thread.join();
    }

    #[cfg(test)]
    pub(crate) fn is_running(&self) -> bool {
        self.running()
            .as_ref()
            .is_some_and(|running| !running.thread.is_finished())
    }
}

impl Inner {
    fn capture(&self, stop: &Cancel) {
        let mut tools = Processes::new(
            self.root.clone(),
            self.tools.flash_bin.clone(),
            self.tools.stock_bin.clone(),
            self.tools.build_bin.clone(),
            stop.clone(),
            Children::new(),
        );
        // Named by the start, so a restart never meets its own segments.
        let prefix = format!(
            "bench-{}",
            wellfed_formats::bench::since_epoch().as_millis()
        );
        let captures = self.root.join(CAPTURES);
        let segment = move |n: u32| captures.join(format!("{prefix}-uart-{n:02}"));
        let mut chain = match tools.capture(&self.port, SEGMENT, Box::new(segment)) {
            Ok(chain) => chain,
            Err(error) => return self.failed(error.to_string()),
        };
        self.store.update(|state| state.capture.failure = None);

        let mut reader = Reader::default();
        loop {
            self.read(&mut reader, &chain.segments());
            if let Some(failure) = chain.failure() {
                chain.stop();
                self.read(&mut reader, &chain.segments());
                self.failed(failure.to_string());
                return;
            }
            if wellfed_rig::sleep(stop, READ_EVERY).is_err() {
                break;
            }
        }
        chain.stop();
        self.read(&mut reader, &chain.segments());
        self.ended();
    }

    /// Read what the segments gained since the last read.
    fn read(&self, reader: &mut Reader, segments: &[PathBuf]) {
        let Some(newest) = segments.last() else {
            return;
        };
        let segment = relative(&self.root, newest);
        let lines = reader.read(segments);
        // An idle feeder sends nothing for long stretches: the state, and
        // every reader of it, wakes only for news.
        if lines.is_empty() && reader.published.as_ref() == Some(&segment) {
            return;
        }
        reader.published = Some(segment.clone());
        let mut start = None;
        let mut update = None;
        for line in &lines {
            for record in Record::all(line) {
                match record.event() {
                    Some(Event::Start { build, bank }) => {
                        start = Some(StartRecord {
                            build: build.to_ascii_lowercase(),
                            bank,
                        });
                    }
                    _ if record.kind() == Some(Tag::Update) => {
                        update = Some(UartRecord {
                            line: format!("{} {}", record.tag, record.body().trim()),
                            at: now(),
                        });
                    }
                    _ => {}
                }
            }
        }

        if let Some(StartRecord {
            build,
            bank: Some(bank),
        }) = &start
        {
            self.banks.observe(*bank, build, Witness::Banner);
        }
        self.store.update(|state| {
            let capture = &mut state.capture;
            if capture.segment.as_deref() != Some(segment.as_str()) {
                capture.segment = Some(segment.clone());
            }
            capture.lines.extend(lines);
            trim_tail(&mut capture.lines);
            if start.is_some() {
                capture.start = start;
            }
            if update.is_some() {
                capture.update = update;
            }
            if state.job.as_ref().is_none_or(|job| job.end.is_some()) {
                state.uart = Uart::Capturing {
                    path: Some(segment),
                };
            }
        });
    }

    fn failed(&self, failure: String) {
        self.store.update(|state| {
            state.capture.failure = Some(failure);
            state.capture.segment = None;
            if matches!(state.uart, Uart::Capturing { .. }) {
                state.uart = Uart::Idle;
            }
        });
    }

    fn ended(&self) {
        self.store.update(|state| {
            state.capture.segment = None;
            if matches!(state.uart, Uart::Capturing { .. }) {
                state.uart = Uart::Idle;
            }
        });
    }
}

/// Where the chain's segments were read to: the segment, the offset in its
/// `serial-rx.bin`, and the start of a line not yet ended.
#[derive(Default)]
struct Reader {
    index: usize,
    offset: u64,
    pending: Vec<u8>,
    /// The segment the state last showed.
    published: Option<String>,
}

impl Reader {
    /// The whole lines `segments` gained, in order.
    fn read(&mut self, segments: &[PathBuf]) -> Vec<String> {
        let mut lines = Vec::new();
        while self.index < segments.len() {
            let rx = segments[self.index].join("serial-rx.bin");
            self.offset += append(&rx, self.offset, &mut self.pending);
            self.take_lines(&mut lines);
            if self.index + 1 == segments.len() {
                break;
            }
            // The segment is done: what is left of it is a line of its own.
            if !self.pending.is_empty() {
                lines.push(text(&self.pending));
                self.pending.clear();
            }
            self.index += 1;
            self.offset = 0;
        }
        lines
    }

    fn take_lines(&mut self, lines: &mut Vec<String>) {
        while let Some(at) = self.pending.iter().position(|byte| *byte == b'\n') {
            lines.push(text(&self.pending[..at]));
            self.pending.drain(..=at);
        }
        if self.pending.len() > MAX_LINE {
            lines.push(text(&self.pending));
            self.pending.clear();
        }
    }
}

/// Append what `path` holds past `offset` to `out`; how many bytes.
fn append(path: &Path, offset: u64, out: &mut Vec<u8>) -> u64 {
    let Ok(mut file) = File::open(path) else {
        return 0;
    };
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return 0;
    }
    let before = out.len();
    let _ = file.read_to_end(out);
    (out.len() - before) as u64
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_end_matches(['\r', '\n'])
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_read_across_growing_segments() {
        let dir = tempfile::tempdir().unwrap();
        let one = dir.path().join("s-01");
        let two = dir.path().join("s-02");
        std::fs::create_dir_all(&one).unwrap();
        std::fs::create_dir_all(&two).unwrap();
        std::fs::write(one.join("serial-rx.bin"), b"boot\r\nPLFW1 start bu").unwrap();
        let mut reader = Reader::default();

        assert_eq!(reader.read(std::slice::from_ref(&one)), ["boot"]);

        std::fs::write(
            one.join("serial-rx.bin"),
            b"boot\r\nPLFW1 start build=ab bank=1\r\nhalf",
        )
        .unwrap();
        std::fs::write(two.join("serial-rx.bin"), b"next\n").unwrap();
        assert_eq!(
            reader.read(&[one, two]),
            ["PLFW1 start build=ab bank=1", "half", "next"]
        );
    }
}
