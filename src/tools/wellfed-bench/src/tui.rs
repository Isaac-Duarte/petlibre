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
//! While the flash client holds the UART, the first press only shows the
//! confirmation in the footer, and a second within 5 s stops.
//!
//! Should the console end on its own (a terminal error, a panic), it tells
//! the server, which puts its lines back on the terminal and keeps serving.

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
use wellfed_bench_server::stopping::{Ask, CONFIRM_PROMPT, Confirm, Held, Press};
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

/// The running console. Dropped without [`Self::finish`], as when the
/// main thread panics, it still closes the screen and gives the terminal
/// back out of raw mode.
pub(crate) struct Tui {
    done: Arc<AtomicBool>,
    thread: Option<JoinHandle<io::Result<()>>>,
}

impl Tui {
    /// Take over the terminal and draw `store` until [`Self::finish`];
    /// a confirmed `q` or Ctrl+C sends [`Ask::Quit`] on `asks`, and a
    /// console that ends on its own [`Ask::ConsoleEnded`].
    pub(crate) fn start(
        store: Store,
        lines: Receiver<String>,
        asks: UnboundedSender<Ask>,
    ) -> io::Result<Self> {
        let tty = tty()?;
        let done = Arc::new(AtomicBool::new(false));
        let finished = Arc::clone(&done);
        let thread = std::thread::Builder::new()
            .name("console".into())
            .spawn(move || {
                // Dropped last, after the screen is closed, a panic included.
                let _ended = Ended {
                    asks: asks.clone(),
                    done: Arc::clone(&finished),
                };
                run(tty, &store, &lines, &asks, &finished)
            })?;
        Ok(Self {
            done,
            thread: Some(thread),
        })
    }

    /// Close the screen, after the server has stopped, and give the
    /// terminal back with the log's last lines on it.
    pub(crate) fn finish(mut self) -> io::Result<()> {
        self.close()
    }

    fn close(&mut self) -> io::Result<()> {
        self.done.store(true, Ordering::SeqCst);
        match self.thread.take() {
            Some(thread) => thread
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("the console thread panicked"))),
            None => Ok(()),
        }
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

/// Tells the server when the console thread ends without being asked to.
struct Ended {
    asks: UnboundedSender<Ask>,
    done: Arc<AtomicBool>,
}

impl Drop for Ended {
    fn drop(&mut self) {
        if !self.done.load(Ordering::SeqCst) {
            let _ = self.asks.send(Ask::ConsoleEnded);
        }
    }
}

/// What a press of `q` or Ctrl+C does, and the confirmation to show.
#[derive(Default)]
struct Quitting {
    confirm: Confirm,
}

impl Quitting {
    /// A press, with the UART as the state has it: [`Ask::Quit`] to send,
    /// or nothing while it waits for the second.
    fn press(&mut self, uart: &Uart, at: Instant) -> Option<Ask> {
        match self.confirm.press(Held::of(uart), at) {
            Press::Stop => Some(Ask::Quit),
            Press::Confirm => None,
        }
    }

    /// The footer's confirmation, while a first press waits.
    fn notice(&self, at: Instant) -> Option<&'static str> {
        self.confirm.pending(at).then_some(CONFIRM_PROMPT)
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
    asks: &UnboundedSender<Ask>,
    done: &AtomicBool,
) -> io::Result<()> {
    let mut after = tty.try_clone()?;
    let screen = Screenful::enter(tty.try_clone()?)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(tty))?;
    let mut log = VecDeque::new();
    let mut scroll = 0;
    let mut drawn: Option<Instant> = None;
    let mut quitting = Quitting::default();
    let mut noticed = false;

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
        let notice = quitting.notice(Instant::now());
        // The confirmation shows, and goes, at once.
        if notice.is_some() != noticed {
            drawn = None;
            noticed = notice.is_some();
        }
        if drawn.is_none_or(|at| at.elapsed() >= frame) {
            terminal.draw(|frame| {
                view::draw(
                    frame,
                    &Screen {
                        state: &state,
                        log: &log,
                        scroll,
                        notice,
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
            Event::Key(key)
                if key.kind == KeyEventKind::Press && quits(key.code, key.modifiers) =>
            {
                let uart = store.read().uart;
                match quitting.press(&uart, Instant::now()) {
                    Some(ask) => {
                        let _ = asks.send(ask);
                    }
                    None => drawn = None,
                }
            }
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
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

/// `q`, or Ctrl+C, which raw mode delivers as a key.
fn quits(code: KeyCode, modifiers: KeyModifiers) -> bool {
    code == KeyCode::Char('q')
        || (code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL))
}

fn push(log: &mut VecDeque<String>, line: String) {
    if log.len() == KEPT {
        log.pop_front();
    }
    log.push_back(line);
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    const WRITING: Uart = Uart::Writing {
        done: 3,
        total: 19,
        counted: true,
    };

    #[test]
    fn q_during_a_write_shows_the_confirmation_and_only_a_second_press_quits() {
        let start = Instant::now();
        let mut quitting = Quitting::default();

        assert_eq!(quitting.press(&WRITING, start), None);
        assert_eq!(quitting.notice(start), Some(CONFIRM_PROMPT));
        assert_eq!(
            quitting.press(&WRITING, start + Duration::from_secs(2)),
            Some(Ask::Quit)
        );
        assert_eq!(quitting.notice(start + Duration::from_secs(2)), None);

        // In the loader too; a second press after 5 s only asks again.
        assert_eq!(quitting.press(&Uart::Loader, start), None);
        let late = start + Duration::from_secs(6);
        assert_eq!(quitting.notice(late), None);
        assert_eq!(quitting.press(&Uart::Loader, late), None);

        // With the UART free, one press quits.
        let mut idle = Quitting::default();
        assert_eq!(idle.press(&Uart::Idle, start), Some(Ask::Quit));
    }

    #[test]
    fn a_console_that_ends_unasked_tells_the_server_and_a_finished_one_does_not() {
        let (asks, mut asked) = tokio::sync::mpsc::unbounded_channel();
        let done = Arc::new(AtomicBool::new(false));

        drop(Ended {
            asks: asks.clone(),
            done: Arc::clone(&done),
        });
        assert_eq!(asked.try_recv().ok(), Some(Ask::ConsoleEnded));

        done.store(true, Ordering::SeqCst);
        drop(Ended { asks, done });
        assert!(asked.try_recv().is_err());
    }
}
