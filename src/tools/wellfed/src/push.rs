//! Pushing a signed update over the device API until bank 1 is activated:
//! the installer's promotion and "Restore stock" both run it.
//!
//! The driver is `wellfed_connect::update::push`, which the server's
//! installs share; this module gives it the CLI's own sessions, which it
//! opens and reopens itself, and prints the progress.

use std::time::{Duration, Instant};

use anyhow::{Error, Result};
use wellfed_client::update::Image;
use wellfed_connect::update::{Opened, Pace, Steps};
use wellfed_model::{Answer, Op, UpdateView};

pub(crate) use wellfed_connect::update::Pushed;

/// How often a reopen is tried.
const POLL: Duration = Duration::from_secs(5);
/// From a restart until the feeder answers the device API again.
pub(crate) const REOPEN_WAIT: Duration = Duration::from_secs(5 * 60);

/// Push `image` until bank 1 is activated. `connect` opens a session;
/// `activated` says whether a reopened session, answering from `bank` with
/// `build`, shows the activation already happened (its answer lost).
pub(crate) async fn push<C, A>(image: Image<'_>, connect: C, activated: A) -> Result<Pushed>
where
    C: AsyncFn() -> Result<wellfed_connect::Connection>,
    A: Fn(u8, &str) -> bool,
{
    let mut session = Session {
        connect,
        activated,
        connection: None,
        written: 0,
        found: false,
    };
    let pushed = wellfed_connect::update::push(image, &mut session, Pace::DEFAULT).await?;

    // A reopened session that found the build running already said so.
    if pushed == Pushed::Activated && !session.found {
        eprintln!("Activated: the feeder restarts into bank 1.");
    }
    Ok(pushed)
}

/// The CLI's sessions with the feeder, one at a time.
struct Session<C, A> {
    connect: C,
    activated: A,
    connection: Option<wellfed_connect::Connection>,
    /// Bytes written at the last progress line.
    written: u32,
    /// A reopened session found bank 1 running the build.
    found: bool,
}

impl<C, A> Steps for Session<C, A>
where
    C: AsyncFn() -> Result<wellfed_connect::Connection>,
    A: Fn(u8, &str) -> bool,
{
    type Error = Error;

    async fn step(&mut self, op: Op) -> Result<Option<Answer>> {
        let Some(connection) = &mut self.connection else {
            return Ok(None);
        };
        match connection.ask(crate::device::random()?, &op).await {
            Ok(answer) => Ok(Some(answer)),
            Err(error) => {
                eprintln!("  The session dropped ({error}); resuming on a new one.");
                self.connection = None;
                Ok(None)
            }
        }
    }

    async fn reopen(&mut self) -> Result<Option<Opened>> {
        self.connection = None;
        let deadline = Instant::now() + REOPEN_WAIT;
        loop {
            if let Ok(mut session) = (self.connect)().await
                && let Ok(Answer::Status(status)) =
                    session.ask(crate::device::random()?, &Op::Status).await
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
            eprintln!("Bank 1 already runs the build: it was activated.");
        }
        self.found
    }

    fn progress(&mut self, view: &UpdateView) {
        if view.written / 65_536 != self.written / 65_536 {
            self.written = view.written;
            eprintln!("  {} of {} bytes written", view.written, view.image_len);
        }
    }
}
