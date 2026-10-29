//! When `serve` stops, and what may not stop it mid-write.
//!
//! A stop cancels the running job, and a cancelled `wellfed-flash` stops
//! its write terminally: the feeder then holds a half-written bank that
//! only another UART write recovers. So while the flash client holds the
//! UART, nothing stops the server by accident:
//! - the operator's Ctrl+C (SIGINT in a line console, a key in the
//!   full-screen one) and the full-screen console's `q` stop only when
//!   pressed twice within [`CONFIRM_WINDOW`] during a write or a loader
//!   session;
//! - SIGTERM and SIGHUP during a write are deferred, however many come:
//!   the job finishes and the server then stops as usual. SIGKILL remains
//!   the operator's last resort; the lease and orphan checks handle it;
//! - a full-screen console that ends on its own (a terminal error, a
//!   panic) stops nothing: its lines go back to the terminal, and Ctrl+C
//!   or SIGTERM stops the server from then on.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedReceiver;
use wellfed_formats::bench::live::{BenchState, JobId, Uart};

use crate::state::Store;

/// How long a first press during a write waits for the second.
pub const CONFIRM_WINDOW: Duration = Duration::from_secs(5);

/// What a first press during a write shows, in the full-screen console's
/// footer.
pub const CONFIRM_PROMPT: &str = "A UART write is under way; press again within 5 s to cancel it";

/// What asks the server to stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    /// SIGINT: the operator's Ctrl+C in a line console.
    Interrupt,
    /// SIGTERM, or the terminal hanging up.
    Terminate,
    /// The full-screen console's `q` or Ctrl+C, confirmed there.
    Quit,
    /// The full-screen console ended without being asked to.
    ConsoleEnded,
}

/// What came of one press of Ctrl+C or `q`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    Stop,
    /// The first during a write: shown [`CONFIRM_PROMPT`], it stops nothing.
    Confirm,
}

/// A press that stops at once, unless the flash client holds the UART:
/// then only a second within [`CONFIRM_WINDOW`] of the first does.
#[derive(Debug, Default)]
pub struct Confirm {
    armed: Option<Instant>,
}

impl Confirm {
    /// A press at `at`, with the UART held or not.
    pub fn press(&mut self, held: Held, at: Instant) -> Press {
        if held == Held::Free {
            return Press::Stop;
        }
        if self.pending(at) {
            self.armed = None;
            return Press::Stop;
        }
        self.armed = Some(at);
        Press::Confirm
    }

    /// Whether a first press waits for its second at `at`.
    pub fn pending(&self, at: Instant) -> bool {
        self.armed
            .is_some_and(|armed| at.saturating_duration_since(armed) <= CONFIRM_WINDOW)
    }
}

/// Whether the flash client holds the UART, as a stop sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    Free,
    /// A loader session: a stop asks to be pressed twice.
    Loader,
    /// A write: a stop asks to be pressed twice, and a signal waits.
    Writing,
}

impl Held {
    pub fn of(uart: &Uart) -> Self {
        match uart {
            Uart::Writing { .. } => Self::Writing,
            Uart::Loader => Self::Loader,
            Uart::Idle | Uart::Capturing { .. } => Self::Free,
        }
    }
}

type Say = Arc<dyn Fn(String) + Send + Sync>;

/// Decides when the server stops, from its asks and its state.
pub struct Stopper {
    store: Store,
    say: Say,
    /// Puts the console's lines back on the terminal.
    console_ended: Arc<dyn Fn() + Send + Sync>,
}

impl Stopper {
    pub(crate) fn new(store: Store, say: Say, console_ended: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            store,
            say,
            console_ended,
        }
    }

    /// Complete once the server should stop; give it to [`crate::Server::run`].
    /// With every sender of `asks` gone, nothing more can ask, and it
    /// stops only once a deferred stop is due.
    pub async fn until_stop(self, mut asks: UnboundedReceiver<Ask>) {
        let mut confirm = Confirm::default();
        // The job a signal's stop waits for, once a write is under way.
        let mut deferred: Option<Option<JobId>> = None;
        let mut states = self.store.subscribe();
        let mut open = true;

        loop {
            if let Some(job) = deferred
                && finished(&states.borrow_and_update(), job)
            {
                (self.say)("The write has ended and its job with it: stopping".into());
                return;
            }

            tokio::select! {
                ask = asks.recv(), if open => {
                    let Some(ask) = ask else {
                        open = false;
                        continue;
                    };
                    let state = self.store.read();
                    let held = Held::of(&state.uart);
                    match ask {
                        Ask::Quit => return,
                        Ask::ConsoleEnded => {
                            (self.console_ended)();
                            (self.say)(
                                "The full-screen console ended; Ctrl+C or SIGTERM now stops the server"
                                    .into(),
                            );
                        }
                        Ask::Interrupt => match confirm.press(held, Instant::now()) {
                            Press::Stop => return,
                            Press::Confirm => (self.say)(format!(
                                "A UART write is under way; press Ctrl+C again within {} s to cancel it",
                                CONFIRM_WINDOW.as_secs()
                            )),
                        },
                        Ask::Terminate if held == Held::Writing => {
                            let running = running(&state);
                            if deferred.is_none() {
                                deferred = Some(running);
                            }
                            (self.say)(
                                "A UART write is under way: stopping once the write ends (SIGKILL is the last resort)"
                                    .into(),
                            );
                        }
                        // A write's job that runs on after its write still
                        // ends before the stop: the signal came during it.
                        Ask::Terminate if deferred.is_some() => {}
                        Ask::Terminate => return,
                    }
                }
                changed = states.changed() => {
                    if changed.is_err() {
                        std::future::pending::<()>().await;
                    }
                }
            }
        }
    }
}

/// The running job's id.
fn running(state: &BenchState) -> Option<JobId> {
    state
        .job
        .as_ref()
        .filter(|job| job.end.is_none())
        .map(|job| job.id)
}

/// Whether a stop deferred for `job` is due: the write is over and `job`
/// no longer runs.
fn finished(state: &BenchState, job: Option<JobId>) -> bool {
    Held::of(&state.uart) != Held::Writing && (job.is_none() || running(state) != job)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tokio::sync::mpsc::unbounded_channel;
    use wellfed_formats::bench::live::{CancelState, JobEnd, JobView};

    use super::*;

    #[test]
    fn a_press_during_a_write_asks_for_a_second_within_five_seconds() {
        let start = Instant::now();
        let mut confirm = Confirm::default();

        assert_eq!(confirm.press(Held::Free, start), Press::Stop);
        assert_eq!(confirm.press(Held::Writing, start), Press::Confirm);
        assert!(confirm.pending(start + Duration::from_secs(4)));
        assert_eq!(
            confirm.press(Held::Writing, start + Duration::from_secs(4)),
            Press::Stop
        );

        // Too late: the second counts as a first.
        assert_eq!(confirm.press(Held::Loader, start), Press::Confirm);
        let late = start + CONFIRM_WINDOW + Duration::from_secs(1);
        assert!(!confirm.pending(late));
        assert_eq!(confirm.press(Held::Loader, late), Press::Confirm);
    }

    struct Fixture {
        store: Store,
        said: Arc<Mutex<Vec<String>>>,
        fell_back: Arc<Mutex<bool>>,
        stopper: Stopper,
    }

    fn fixture() -> Fixture {
        let store = Store::new(crate::tests::idle_state());
        let said = Arc::new(Mutex::new(Vec::new()));
        let fell_back = Arc::new(Mutex::new(false));
        let lines = Arc::clone(&said);
        let back = Arc::clone(&fell_back);
        let stopper = Stopper::new(
            store.clone(),
            Arc::new(move |line| lines.lock().unwrap().push(line)),
            Arc::new(move || *back.lock().unwrap() = true),
        );
        Fixture {
            store,
            said,
            fell_back,
            stopper,
        }
    }

    fn writing(state: &mut BenchState, id: JobId) {
        state.uart = Uart::Writing {
            done: 3,
            total: 19,
            counted: true,
        };
        state.job = Some(JobView {
            id,
            label: "flash-execute unit-07".into(),
            log: String::new(),
            accepted: 0.0,
            record: None,
            upcoming: Vec::new(),
            cancel: CancelState::None,
            end: None,
        });
    }

    /// Whether `stopping` is still pending after a moment.
    async fn still_serving(stopping: &mut tokio::task::JoinHandle<()>) -> bool {
        tokio::time::timeout(Duration::from_millis(150), stopping)
            .await
            .is_err()
    }

    #[tokio::test]
    async fn a_console_that_ends_stops_nothing_and_its_lines_go_back_to_the_terminal() {
        let Fixture {
            said,
            fell_back,
            stopper,
            ..
        } = fixture();
        let (asks, asked) = unbounded_channel();
        let mut stopping = tokio::spawn(stopper.until_stop(asked));

        asks.send(Ask::ConsoleEnded).unwrap();
        drop(asks);

        assert!(still_serving(&mut stopping).await);
        assert!(*fell_back.lock().unwrap());
        assert!(
            said.lock().unwrap()[0].contains("Ctrl+C or SIGTERM now stops the server"),
            "{said:?}"
        );
        stopping.abort();
    }

    #[tokio::test]
    async fn sigterm_during_a_write_waits_for_the_job_however_often_it_comes() {
        let Fixture {
            store,
            said,
            stopper,
            ..
        } = fixture();
        store.update(|state| writing(state, 7));
        let (asks, asked) = unbounded_channel();
        let mut stopping = tokio::spawn(stopper.until_stop(asked));

        asks.send(Ask::Terminate).unwrap();
        asks.send(Ask::Terminate).unwrap();
        assert!(still_serving(&mut stopping).await);
        assert!(
            said.lock().unwrap()[0].contains("stopping once the write ends"),
            "{said:?}"
        );

        // The write ends, its job runs on: still waiting, and a SIGTERM
        // now still waits for the job.
        store.update(|state| state.uart = Uart::Idle);
        asks.send(Ask::Terminate).unwrap();
        assert!(still_serving(&mut stopping).await);

        store.update(|state| {
            if let Some(job) = state.job.as_mut() {
                job.end = Some(JobEnd {
                    code: 0,
                    error: None,
                    finished: 1.0,
                });
            }
        });
        tokio::time::timeout(Duration::from_secs(2), stopping)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn ctrl_c_during_a_write_stops_only_when_pressed_twice() {
        let Fixture {
            store,
            said,
            stopper,
            ..
        } = fixture();
        store.update(|state| writing(state, 3));
        let (asks, asked) = unbounded_channel();
        let mut stopping = tokio::spawn(stopper.until_stop(asked));

        asks.send(Ask::Interrupt).unwrap();
        assert!(still_serving(&mut stopping).await);
        assert!(
            said.lock().unwrap()[0].contains("press Ctrl+C again within 5 s"),
            "{said:?}"
        );

        asks.send(Ask::Interrupt).unwrap();
        tokio::time::timeout(Duration::from_secs(2), stopping)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn with_no_write_any_signal_or_quit_stops_at_once() {
        for ask in [Ask::Interrupt, Ask::Terminate, Ask::Quit] {
            let Fixture { stopper, .. } = fixture();
            let (asks, asked) = unbounded_channel();
            let stopping = tokio::spawn(stopper.until_stop(asked));

            asks.send(ask).unwrap();

            tokio::time::timeout(Duration::from_secs(2), stopping)
                .await
                .unwrap()
                .unwrap();
        }
    }
}
