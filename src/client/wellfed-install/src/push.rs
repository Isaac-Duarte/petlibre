//! Pushing a signed update over the device API until bank 1 is activated:
//! the install's promotion and the CLI's "Restore stock" both run it.
//!
//! The driver is `wellfed_connect::update::push`, which the server's
//! installs share; this module gives it sessions the caller opens (and
//! reopens across the feeder's restarts), and reports the progress.

use std::time::{Duration, Instant};

use wellfed_client::update::Image;
use wellfed_connect::update::{Opened, Pace, Steps};
use wellfed_model::{Answer, Op, UpdateView};

pub use wellfed_connect::update::Pushed;

use crate::Error;
use crate::event::Event;
use crate::platform::Report;
use crate::session::random;

/// How often a reopen is tried.
const POLL: Duration = Duration::from_secs(5);
/// From a restart until the feeder answers the device API again.
pub const REOPEN_WAIT: Duration = Duration::from_secs(5 * 60);
/// Report the progress every this many bytes written.
const PROGRESS: u32 = 65_536;

/// Push `image` until bank 1 is activated. `connect` opens a session;
/// `activated` says whether a reopened session, answering from `bank` with
/// `build`, shows the activation already happened (its answer lost).
pub async fn push<C, A>(
    image: Image<'_>,
    connect: C,
    activated: A,
    report: &impl Report,
) -> Result<Pushed, Error>
where
    C: AsyncFn() -> Result<wellfed_connect::Connection, Error>,
    A: Fn(u8, &str) -> bool,
{
    let mut session = Session {
        connect,
        activated,
        report,
        connection: None,
        written: 0,
        found: false,
    };
    let pushed = wellfed_connect::update::push(image, &mut session, Pace::DEFAULT).await?;

    // A reopened session that found the build running already said so.
    if pushed == Pushed::Activated && !session.found {
        report.event(Event::Activated);
    }
    Ok(pushed)
}

/// The caller's sessions with the feeder, one at a time.
struct Session<'r, C, A, R> {
    connect: C,
    activated: A,
    report: &'r R,
    connection: Option<wellfed_connect::Connection>,
    /// Bytes written at the last progress report.
    written: u32,
    /// A reopened session found bank 1 running the build.
    found: bool,
}

impl<C, A, R> Steps for Session<'_, C, A, R>
where
    C: AsyncFn() -> Result<wellfed_connect::Connection, Error>,
    A: Fn(u8, &str) -> bool,
    R: Report,
{
    type Error = Error;

    async fn step(&mut self, op: Op) -> Result<Option<Answer>, Error> {
        let Some(connection) = &mut self.connection else {
            return Ok(None);
        };
        match connection.ask(random()?, &op).await {
            Ok(answer) => Ok(Some(answer)),
            Err(error) => {
                self.report.event(Event::SessionDropped {
                    error: error.to_string(),
                });
                self.connection = None;
                Ok(None)
            }
        }
    }

    async fn reopen(&mut self) -> Result<Option<Opened>, Error> {
        self.connection = None;
        let deadline = Instant::now() + REOPEN_WAIT;
        loop {
            if let Ok(mut session) = (self.connect)().await
                && let Ok(Answer::Status(status)) = session.ask(random()?, &Op::Status).await
            {
                self.connection = Some(session);
                return Ok(Some(Opened {
                    bank: status.bank,
                    build: status.build,
                }));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            tokio::time::sleep(POLL).await;
        }
    }

    fn activated(&mut self, opened: &Opened) -> bool {
        self.found = (self.activated)(opened.bank, &opened.build);
        if self.found {
            self.report.event(Event::AlreadyActivated);
        }
        self.found
    }

    fn progress(&mut self, view: &UpdateView) {
        if view.written / PROGRESS != self.written / PROGRESS {
            self.written = view.written;
            self.report.event(Event::Written {
                written: view.written,
                length: view.image_len,
            });
        }
    }
}
