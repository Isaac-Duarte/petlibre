//! Pushing a signed update over the device API until bank 1 is activated:
//! the installer's promotion and "Restore stock" both run it.
//!
//! The steps are `wellfed_client::update::Installer`'s. A dropped session
//! loses nothing: chunks are idempotent by offset and bank 0 waits a minute
//! for the next, so a lost answer or a restart reopens the session and the
//! install resumes where bank 0 says. An offer is never sent again behind
//! the caller's back: a refusal ends the push.

use std::time::{Duration, Instant};

use anyhow::Result;
use wellfed_client::update::{Image, InstallError as UpdateError, Installer, Next, Sent};
use wellfed_model::{Answer, Op};

/// How often a reopen is tried.
const POLL: Duration = Duration::from_secs(5);
/// From a restart until the feeder answers the device API again.
pub(crate) const REOPEN_WAIT: Duration = Duration::from_secs(5 * 60);
/// Sessions a push reopens at most: a feeder that keeps dropping them is
/// not coming back.
const REOPEN_MAX: u32 = 20;

/// How a push ended.
#[derive(Debug)]
pub(crate) enum Pushed {
    /// Bank 1 is activated; the feeder restarts into it.
    Activated,
    /// The install's own error, for good.
    Failed(UpdateError),
    /// The feeder stopped answering the device API. With `offered`, the
    /// offer went out: bank 1 may be partly written, or activated with its
    /// answer lost, and only the feeder can tell.
    Gone { offered: bool },
}

/// Push `image` until bank 1 is activated. `connect` opens a session;
/// `activated` says whether a reopened session, answering from `bank` with
/// `build`, shows the activation already happened (its answer lost).
pub(crate) async fn push(
    image: Image<'_>,
    connect: impl AsyncFn() -> Result<wellfed_connect::Connection>,
    activated: impl Fn(u8, &str) -> bool,
) -> Result<Pushed> {
    let mut installer = Installer::new(image);
    let mut connection = None;
    let mut reopened = 0;
    let mut written = 0;
    let mut offered = false;
    loop {
        let session = match &mut connection {
            Some(session) => session,
            None if reopened >= REOPEN_MAX => return Ok(Pushed::Gone { offered }),
            None => {
                reopened += 1;
                match reopen(&mut installer, &connect, &activated).await? {
                    Reopened::Session(session) => connection.insert(*session),
                    Reopened::Activated => {
                        eprintln!("Bank 1 already runs the build: it was activated.");
                        return Ok(Pushed::Activated);
                    }
                    Reopened::Gone => return Ok(Pushed::Gone { offered }),
                }
            }
        };

        match installer.poll() {
            Next::Send(sent, op) => {
                offered |= sent == Sent::Offer;
                match session.ask(crate::device::random()?, &op).await {
                    // An answer the installer rejects ends the install; the
                    // next poll says so.
                    Ok(answer) => {
                        let _ = installer.answer(sent, &answer);
                    }
                    Err(error) => {
                        eprintln!("  The session dropped ({error}); resuming on a new one.");
                        installer.lost();
                        connection = None;
                    }
                }
                if let Some(view) = installer.view()
                    && view.written / 65_536 != written / 65_536
                {
                    written = view.written;
                    eprintln!("  {} of {} bytes written", view.written, view.image_len);
                }
            }
            Next::Wait => tokio::time::sleep(Duration::from_millis(200)).await,
            Next::Activated => {
                eprintln!("Activated: the feeder restarts into bank 1.");
                return Ok(Pushed::Activated);
            }
            Next::Failed(error) => return Ok(Pushed::Failed(error)),
            // Bank 1 gave way (a feeder that answered from bank 1):
            // reconnect once it has restarted into bank 0.
            Next::Reconnect => {
                installer.lost();
                connection = None;
                tokio::time::sleep(POLL).await;
            }
            Next::Cancelled => return Ok(Pushed::Failed(UpdateError::Lost)),
        }
    }
}

/// What reopening a push's session found.
enum Reopened {
    Session(Box<wellfed_connect::Connection>),
    /// The activation already happened.
    Activated,
    /// The feeder did not come back in time.
    Gone,
}

/// A new session, with the installer told which bank answers.
async fn reopen(
    installer: &mut Installer<'_>,
    connect: &impl AsyncFn() -> Result<wellfed_connect::Connection>,
    activated: &impl Fn(u8, &str) -> bool,
) -> Result<Reopened> {
    let deadline = Instant::now() + REOPEN_WAIT;
    loop {
        if let Ok(mut session) = connect().await
            && let Ok(Answer::Status(status)) =
                session.ask(crate::device::random()?, &Op::Status).await
        {
            if activated(status.bank, &status.build) {
                return Ok(Reopened::Activated);
            }
            installer.opened(status.bank);
            return Ok(Reopened::Session(Box::new(session)));
        }
        if Instant::now() >= deadline {
            return Ok(Reopened::Gone);
        }
        tokio::time::sleep(POLL).await;
    }
}
