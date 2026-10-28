//! An install on the long lane: `wellfed-connect`'s update driver, the
//! one the owner's CLI runs, with each of its steps an [`Operation`] step.
//! "Restore stock" is the same push of the feeder's own stock image, with
//! the downgrade statement it needs; reading that image from bank 1 is a
//! page per step.
//!
//! The feeder task keeps the session, so the driver never opens one: when
//! the session drops (a cut, or the feeder restarting into bank 0 to take
//! the release), the task reconnects on its own, and the driver waits for
//! the link to come back and reads the feeder's status from the new
//! session. Feeds and status reads keep their own lane throughout, so each
//! waits at most one chunk.

use tokio::time::{Duration, Instant, sleep, timeout_at};
use wellfed_api::Connection as Link;
use wellfed_client::install::Bank1Reader;
use wellfed_client::release::MANIFEST_LEN;
use wellfed_client::update::Image;
use wellfed_connect::update::{Opened, Pace, Steps};
use wellfed_model::{Answer, Op, UpdateView};
use wellfed_session::hex::Lower;

use crate::{AskError, Operation, RandomFailed};

pub use wellfed_client::install::ReadError;
pub use wellfed_client::update::InstallError as UpdateError;
pub use wellfed_connect::update::Pushed;

/// From a restart until the feeder answers the device API again.
const REOPEN_WAIT: Duration = Duration::from_secs(5 * 60);
/// Between status reads that find no session to answer them.
const POLL: Duration = Duration::from_secs(1);

/// What an install offers: a release file as the server keeps it (the
/// signed manifest, then the image it names) and the build it carries.
#[derive(Clone, Copy, Debug)]
pub struct Update<'a> {
    pub file: &'a [u8],
    pub build: [u8; 32],
}

/// What "Restore stock" offers: the kept backup's bank contents, the
/// manifest the owner signed them with as the stock image, and the
/// downgrade statement that lets this feeder take it.
#[derive(Clone, Copy, Debug)]
pub struct Restore<'a> {
    pub manifest: &'a [u8],
    pub downgrade: &'a [u8],
    pub payload: &'a [u8],
    /// The build the manifest names: the payload's SHA-256.
    pub build: [u8; 32],
}

/// Why reading bank 1 stopped. Reads change nothing on the feeder, so a
/// read that stopped is simply asked for again.
#[derive(Debug, thiserror::Error)]
pub enum BankReadError {
    /// The feeder's answer did not fit (a refusal among them).
    #[error(transparent)]
    Bank(#[from] ReadError),
    /// The session dropped, or a page's answer did not come in time.
    #[error("the feeder stopped answering the read")]
    Lost(#[source] AskError),
    #[error(transparent)]
    Random(#[from] RandomFailed),
}

/// Why an install could not run its steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PushError {
    #[error("the release file is shorter than a manifest")]
    Short,
    #[error(transparent)]
    Random(#[from] RandomFailed),
}

impl Operation {
    /// Install `update` until bank 1 is activated, the install fails, or
    /// the feeder is gone, calling `progress` with each update state the
    /// feeder answers.
    pub async fn install(
        &mut self,
        update: Update<'_>,
        progress: impl FnMut(&UpdateView),
    ) -> Result<Pushed, PushError> {
        let (manifest, payload) = update
            .file
            .split_at_checked(MANIFEST_LEN)
            .ok_or(PushError::Short)?;
        let image = Image {
            manifest,
            downgrade: None,
            payload,
        };

        let mut steps = Lane {
            operation: self,
            build: update.build,
            progress,
        };
        wellfed_connect::update::push(image, &mut steps, Pace::DEFAULT).await
    }

    /// Restore stock: push `restore` until bank 1 is activated, the
    /// install fails, or the feeder is gone. Activated, the feeder
    /// restarts into stock, which serves no device API; an activation
    /// whose answer was lost therefore leaves the feeder gone, never
    /// re-offered.
    pub async fn restore(
        &mut self,
        restore: Restore<'_>,
        progress: impl FnMut(&UpdateView),
    ) -> Result<Pushed, RandomFailed> {
        let image = Image {
            manifest: restore.manifest,
            downgrade: Some(restore.downgrade),
            payload: restore.payload,
        };
        let mut steps = Lane {
            operation: self,
            build: restore.build,
            progress,
        };
        match wellfed_connect::update::push(image, &mut steps, Pace::DEFAULT).await {
            Ok(pushed) => Ok(pushed),
            Err(PushError::Random(failed)) => Err(failed),
            // Only a release file is split; a restore's parts come whole.
            Err(PushError::Short) => Ok(Pushed::Failed(UpdateError::Unexpected)),
        }
    }

    /// Read bank 1 whole from the bank-0 role, a page per step, calling
    /// `progress` with the bytes read so far after each page.
    pub async fn read_bank1(
        &mut self,
        mut progress: impl FnMut(usize),
    ) -> Result<Vec<u8>, BankReadError> {
        let mut reader = Bank1Reader::new();
        loop {
            let id = self.request_id()?;
            let answer = self
                .step(id, reader.next_op())
                .await
                .map_err(BankReadError::Lost)?;
            if let Some(bank) = reader.take(answer)? {
                return Ok(bank);
            }
            progress(reader.read());
        }
    }

    fn request_id(&self) -> Result<crate::RequestId, RandomFailed> {
        let mut id = crate::RequestId::default();
        self.random.fill(&mut id)?;
        Ok(id)
    }
}

/// The long lane, as the driver's steps.
struct Lane<'a, P> {
    operation: &'a mut Operation,
    build: [u8; 32],
    progress: P,
}

impl<P: FnMut(&UpdateView)> Steps for Lane<'_, P> {
    type Error = PushError;

    async fn step(&mut self, op: Op) -> Result<Option<Answer>, PushError> {
        let id = self.operation.request_id()?;
        // Offline, unknown, or the feeder let go: the request may have run,
        // and the driver reopens and reads where the install stands.
        Ok(self.operation.step(id, op).await.ok())
    }

    async fn reopen(&mut self) -> Result<Option<Opened>, PushError> {
        let deadline = Instant::now() + REOPEN_WAIT;
        loop {
            let online = self.operation.link.wait_for(|link| *link == Link::Online);
            match timeout_at(deadline, online).await {
                Ok(Ok(_)) => {}
                // Not back in time, or the feeder's task has ended.
                Err(_) | Ok(Err(_)) => return Ok(None),
            }
            let id = self.operation.request_id()?;
            if let Ok(Answer::Status(status)) = self.operation.step(id, Op::Status).await {
                return Ok(Some(Opened {
                    bank: status.bank,
                    build: status.build,
                }));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            sleep(POLL).await;
        }
    }

    /// Bank 1 can run the release's build only once bank 0 activated it.
    fn activated(&mut self, opened: &Opened) -> bool {
        opened.bank == 1
            && opened
                .build
                .eq_ignore_ascii_case(&Lower(&self.build).to_string())
    }

    fn progress(&mut self, view: &UpdateView) {
        (self.progress)(view);
    }
}
