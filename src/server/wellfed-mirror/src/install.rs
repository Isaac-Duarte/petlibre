//! An install on the long lane: `wellfed-connect`'s update driver, the
//! one the owner's CLI runs, with each of its steps an [`Operation`] step.
//!
//! The feeder task keeps the session, so the driver never opens one: when
//! the session drops (a cut, or the feeder restarting into bank 0 to take
//! the release), the task reconnects on its own, and the driver waits for
//! the link to come back and reads the feeder's status from the new
//! session. Feeds and status reads keep their own lane throughout, so each
//! waits at most one chunk.

use wellfed_api::Connection as Link;
use wellfed_client::release::MANIFEST_LEN;
use wellfed_client::update::Image;
use wellfed_connect::update::{Opened, Pace, Steps};
use wellfed_model::{Answer, Op, UpdateView};
use wellfed_session::hex::Lower;
use tokio::time::{Duration, Instant, sleep, timeout_at};

use crate::{Operation, RandomFailed};

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
