//! `serve`'s full-screen console, drawn on `/dev/tty` from the server's
//! live state, with the console's lines (its own and every job's output)
//! in a scrolling log.
//!
//! It runs on a thread of its own and reads only the state and a channel,
//! so a slow terminal never holds the output pump: the pump drops console
//! lines rather than wait, and the job's stream keeps them. While the
//! flash client writes, it redraws at most twice a second, so the bench
//! host spends no more than that on it during a write. `q` and Ctrl+C
//! (which raw mode delivers as a key, not a signal) ask for the same stop
//! as SIGINT: cancel the job, wait for its cleanup, then release the bench.

mod view;

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc::UnboundedSender;
use wellfed_bench_server::Store;
use wellfed_formats::bench::live::Uart;

use view::Screen;

/// The console lines kept for scrolling.
const KEPT: usize = 5_000;
/// Between frames while nothing writes flash.
const FRAME: Duration = Duration::from_millis(250);
/// Between frames during a write: about 2 Hz.
const WRITING_FRAME: Duration = Duration::from_millis(500);
/// The log lines printed back on the terminal after the screen closes.
const TAIL_AFTER: usize = 30;

/// Whether the full-screen console can run: a terminal on stdout and a
/// `/dev/tty` to draw on, unless `--plain` asked for lines.
pub(crate) fn available(plain: bool) -> bool {
    !plain && io::stdout().is_terminal() && tty().is_ok()
}

fn tty() -> io::Result<File> {
    File::options().read(true).write(true).open("/dev/tty")
}

/// The running console.
pub(crate) struct Tui {
    done: Arc<AtomicBool>,
    thread: JoinHandle<io::Result<()>>,
}

impl Tui {
    /// Take over the terminal and draw `store` until [`Self::finish`];
    /// `q` or Ctrl+C sends on `quit`.
    pub(crate) fn start(
        store: Store,
        lines: Receiver<String>,
        quit: UnboundedSender<()>,
    ) -> io::Result<Self> {
        let tty = tty()?;
        let done = Arc::new(AtomicBool::new(false));
        let finished = Arc::clone(&done);
        let thread = std::thread::Builder::new()
            .name("console".into())
            .spawn(move || run(tty, &store, &lines, &quit, &finished))?;
        Ok(Self { done, thread })
    }

    /// Close the screen, after the server has stopped, and give the
    /// terminal back with the log's last lines on it.
    pub(crate) fn finish(self) -> io::Result<()> {
        self.done.store(true, Ordering::SeqCst);
        self.thread
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("the console thread panicked")))
    }
}

/// Raw mode and the alternate screen, left on every way out, a panic
/// included.
struct Screenful {
    tty: File,
}

impl Screenful {
    fn enter(mut tty: File) -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        crossterm::execute!(tty, EnterAlternateScreen, crossterm::cursor::Hide)?;
        Ok(Self { tty })
    }
}

impl Drop for Screenful {
    fn drop(&mut self) {
        let _ = crossterm::execute!(self.tty, LeaveAlternateScreen, crossterm::cursor::Show);
        let _ = terminal::disable_raw_mode();
    }
}

fn run(
    tty: File,
    store: &Store,
    lines: &Receiver<String>,
    quit: &UnboundedSender<()>,
    done: &AtomicBool,
) -> io::Result<()> {
    let mut after = tty.try_clone()?;
    let screen = Screenful::enter(tty.try_clone()?)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(tty))?;
    let mut log = VecDeque::new();
    let mut scroll = 0;
    let mut drawn: Option<Instant> = None;

    loop {
        while let Ok(line) = lines.try_recv() {
            push(&mut log, line);
            // Scrolled up, the view holds still as lines arrive.
            if scroll > 0 {
                scroll = (scroll + 1).min(log.len());
            }
        }
        let state = store.read();
        let frame = if matches!(state.uart, Uart::Writing { .. }) {
            WRITING_FRAME
        } else {
            FRAME
        };
        if drawn.is_none_or(|at| at.elapsed() >= frame) {
            terminal.draw(|frame| {
                view::draw(
                    frame,
                    &Screen {
                        state: &state,
                        log: &log,
                        scroll,
                    },
                );
            })?;
            drawn = Some(Instant::now());
        }
        if done.load(Ordering::SeqCst) {
            break;
        }

        let wait = drawn.map_or(frame, |at| frame.saturating_sub(at.elapsed()));
        if !event::poll(wait)? {
            continue;
        }
        let page = usize::from(terminal.size()?.height / 2).max(1);
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Char('q') => {
                    let _ = quit.send(());
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let _ = quit.send(());
                }
                KeyCode::PageUp | KeyCode::Up => {
                    let step = if key.code == KeyCode::Up { 1 } else { page };
                    scroll = (scroll + step).min(log.len());
                }
                KeyCode::PageDown | KeyCode::Down => {
                    let step = if key.code == KeyCode::Down { 1 } else { page };
                    scroll = scroll.saturating_sub(step);
                }
                KeyCode::End => scroll = 0,
                KeyCode::Home => scroll = log.len(),
                _ => {}
            },
            // Redrawn at once at the new size.
            Event::Resize(..) => drawn = None,
            _ => {}
        }
    }

    drop(terminal);
    drop(screen);
    let tail = log.len().saturating_sub(TAIL_AFTER);
    for line in log.range(tail..) {
        writeln!(after, "{line}")?;
    }
    Ok(())
}

fn push(log: &mut VecDeque<String>, line: String) {
    if log.len() == KEPT {
        log.pop_front();
    }
    log.push_back(line);
}
