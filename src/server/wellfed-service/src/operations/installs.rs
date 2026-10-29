//! Installs: an owner's request to install one of the server's releases on
//! one feeder, or to restore its stock firmware from the server's backup
//! (`stock.rs`), run in the background on the feeder's long-operation lane.
//!
//! One install runs at a time per server, held by a [`Claim`] on the
//! service's one slot: a second request while it runs is
//! [`InstallError::Running`]. The slot also holds the running install's
//! progress, which `GET` reads and the stream carries; the store keeps each
//! install when it starts and when it ends, so a restart finds the one that
//! was running and marks it failed (`interrupted`), and the claim, being in
//! memory, is free again. An end the store would not take, even a second
//! time, is kept in memory for `GET` until the server restarts and marks
//! its row interrupted.
//!
//! Nothing is offered again on the owner's behalf. The driver resumes a
//! transfer across a dropped session (chunks are idempotent by offset), but
//! an offer the feeder refuses, an install it stops, or a feeder that never
//! comes back ends the install; another is another request.

use std::sync::{Arc, Mutex};

use tracing::{Instrument, info, info_span, warn};
use wellfed_api::{
    ClientRole, Connection as Link, ErrorCode, FeederId, FeederRefusal, Install, InstallFailure,
    InstallId, InstallRequest, InstallState, InstallTarget, StreamEvent,
};
use wellfed_mirror::{PushError, Pushed, Restore, Update, UpdateError};
use wellfed_model::{Refusal, UpdateView};
use wellfed_releases::{STOCK_VERSION, StockBackup};

use crate::{Caller, CommandError, Error, Resource, Result, Service, lock};

/// Installs, one at a time per server, a restore of stock among them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    #[error("another install is running: one runs at a time")]
    Running,
    #[error("the restore does not offer the kept backup: {0}")]
    RestoreMismatch(wellfed_releases::RestoreError),
    #[error(
        "the feeder runs from bank 0 with stock in bank 1, and no verified backup of it is kept: \
         read it from the feeder or upload the installer's copy before installing"
    )]
    StockBackupNeeded,
}

impl InstallError {
    pub(crate) fn code(self) -> ErrorCode {
        match self {
            Self::Running => ErrorCode::InstallRunning,
            Self::RestoreMismatch(_) => ErrorCode::RestoreMismatch,
            Self::StockBackupNeeded => ErrorCode::StockBackupNeeded,
        }
    }
}

/// Progress reaches the stream each time this many more bytes are written.
const PROGRESS_STEP: u32 = 32 * 1024;

/// What an install pushes.
pub(crate) enum Offer {
    /// A release file as the cache keeps it, and the build it carries.
    Release { file: Vec<u8>, build: [u8; 32] },
    /// The kept stock backup, with the manifest and statement the owner
    /// signed for it.
    Stock {
        manifest: Vec<u8>,
        downgrade: Vec<u8>,
        backup: StockBackup,
    },
}

/// The server's one install slot, with the ends the store did not take.
pub(crate) type Slot = Arc<Mutex<Installs>>;

/// What the service keeps of installs in memory.
#[derive(Debug, Default)]
pub(crate) struct Installs {
    /// The running install, as it stands.
    running: Option<Install>,
    /// Installs that ended while the store's row still says `running`:
    /// their end was not stored.
    unsaved: Vec<Install>,
}

impl Service {
    /// Start installing `request.release` on the feeder, and return the
    /// install as it starts. It runs in the background; its progress and
    /// its end reach the stream, and [`Self::install_state`] reads it.
    ///
    /// The server installs only where its key is an owner's: the feeder
    /// refuses updates from any other role, so that is refused here first,
    /// as the feeder would. A feeder the server has no session with is
    /// refused too, rather than waited for.
    pub async fn install(
        &self,
        caller: &Caller,
        id: FeederId,
        request: InstallRequest,
    ) -> Result<Install> {
        let row = self.row(id).await?;
        if row.role != ClientRole::Owner {
            return Err(Error::Refused(FeederRefusal::Forbidden));
        }
        let (release, file) = self.release_file(request.release).await?;
        if self.mirror.link(id) != Some(Link::Online) {
            return Err(CommandError::Offline.into());
        }
        self.keeps_stock(id, row.identity).await?;

        let install = self.new_install(
            caller,
            id,
            InstallTarget::Release {
                release: release.id,
            },
            release.version,
            release.image_len,
        )?;
        let offer = Offer::Release {
            file,
            build: release.build_id.0,
        };
        self.begin(install, offer).await
    }

    /// An install of `target` on the feeder `id`, as it starts.
    pub(crate) fn new_install(
        &self,
        caller: &Caller,
        id: FeederId,
        target: InstallTarget,
        version: u32,
        image_len: u32,
    ) -> Result<Install> {
        Ok(Install {
            id: InstallId(self.shared.new_id()?),
            feeder: id,
            target,
            version,
            by: caller.user.id,
            started: self.shared.clock.now(),
            state: InstallState::Running,
            written: 0,
            image_len,
            failure: None,
            finished: None,
        })
    }

    /// Take the slot for `install` (refused while another runs), keep it,
    /// and push `offer` in the background.
    pub(crate) async fn begin(&self, install: Install, offer: Offer) -> Result<Install> {
        let claim = Claim::take(&self.operations.installing, install)?;
        self.shared.store.save_install(&install).await?;
        match install.target {
            InstallTarget::Release { release } => info!(
                feeder = %install.feeder,
                %release,
                "install of version {} started",
                install.version
            ),
            InstallTarget::Stock { sha256 } => info!(
                feeder = %install.feeder,
                backup = %sha256,
                "restore of stock {STOCK_VERSION} started, as version {}",
                install.version
            ),
        }
        self.publish_install(install);

        let service = self.clone();
        let cancel = self.cancel.child_token();
        let span = info_span!(parent: None, "install", feeder = %install.feeder);
        self.tasks.spawn(
            async move {
                // On shutdown the push stops and the row stays `running`
                // until the next start marks it interrupted, as after a
                // crash. Cancellation is looked at first: shutting down also
                // ends the mirror's session, and a push polled after that
                // would end `unreachable` and store it, though the server
                // stopped it.
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => {}
                    () = service.run_install(claim, offer) => {}
                }
            }
            .instrument(span),
        );
        Ok(install)
    }

    /// The running install, if it is the feeder `id`'s.
    pub(crate) fn running_on(&self, id: FeederId) -> Option<Install> {
        lock(&self.operations.installing)
            .running
            .filter(|install| install.feeder == id)
    }

    /// `last` as it ended, if its end is held in memory rather than in the
    /// store.
    pub(crate) fn unsaved(&self, last: Install) -> Install {
        let installs = lock(&self.operations.installing);
        let unsaved = installs.unsaved.iter().find(|ended| ended.id == last.id);
        unsaved.copied().unwrap_or(last)
    }

    /// The feeder's running install, or else its last one.
    pub async fn install_state(&self, id: FeederId) -> Result<Install> {
        self.row(id).await?;
        if let Some(install) = self.running_on(id) {
            return Ok(install);
        }

        let last = self.shared.store.latest_install(id).await?;
        let last = last.ok_or(Error::NotFound(Resource::Install))?;
        Ok(self.unsaved(last))
    }

    /// Mark the install a stopped server left running failed.
    pub(crate) async fn interrupt_installs(&self) -> Result<()> {
        let now = self.shared.clock.now();
        let interrupted = self.shared.store.interrupt_installs(now).await?;
        if interrupted > 0 {
            warn!("an install was running when the server stopped: marked interrupted");
        }
        Ok(())
    }

    /// Run the install to its end, keep that, and free the slot.
    async fn run_install(&self, mut claim: Claim, offer: Offer) {
        let feeder = claim.install.feeder;
        let ended = self.push(&mut claim, feeder, &offer).await;
        let failed = |failure| (InstallState::Failed, Some(failure));
        let (state, failure) = match ended {
            Ok(Pushed::Activated) => (InstallState::Activated, None),
            Ok(Pushed::AlreadyRunning) => (InstallState::AlreadyRunning, None),
            Ok(Pushed::Failed(error)) => failed(failure(error)),
            Ok(Pushed::Gone { offered }) => failed(InstallFailure::Unreachable { offered }),
            // The cache verified the file just now, so it is never short.
            Err(PushError::Short) => failed(InstallFailure::Unexpected),
            Err(PushError::Random(error)) => {
                warn!("the install stopped: {error}");
                failed(InstallFailure::Server)
            }
        };

        let now = self.shared.clock.now();
        let install = claim.update(|install| {
            install.state = state;
            install.failure = failure;
            install.finished = Some(now);
        });
        match (state, failure) {
            (_, Some(failure)) => warn!("the install failed: {failure:?}"),
            (InstallState::AlreadyRunning, None) => info!(
                "the feeder already runs version {}: nothing was sent",
                install.version
            ),
            (_, None) => match install.target {
                InstallTarget::Release { .. } => info!(
                    "activated: the feeder restarts into version {}",
                    install.version
                ),
                InstallTarget::Stock { .. } => info!(
                    "activated: the feeder restarts into stock {STOCK_VERSION}, which serves no \
                     device API"
                ),
            },
        }

        self.save_end(&claim, install).await;
        self.publish_install(install);
        drop(claim);
    }

    /// Store the install's end, trying twice; an end the store would not
    /// take stays in memory, so `GET` does not read the row's `running`.
    async fn save_end(&self, claim: &Claim, install: Install) {
        for attempt in 1..=2 {
            match self.shared.store.save_install(&install).await {
                Ok(()) => return,
                Err(error) => {
                    warn!("the install's end was not stored (attempt {attempt}): {error}");
                }
            }
        }
        claim.unsaved(install);
    }

    /// Wait for the feeder's long-operation turn and push the offer over
    /// it, publishing the progress as it goes.
    async fn push(
        &self,
        claim: &mut Claim,
        feeder: FeederId,
        offer: &Offer,
    ) -> Result<Pushed, PushError> {
        let Ok(mut operation) = self.mirror.operation(feeder).await else {
            // The only refusal: the feeder was let go before the turn came.
            return Ok(Pushed::Gone { offered: false });
        };

        let mut published = 0;
        let progress = |view: &UpdateView| {
            let install = claim.update(|install| install.written = view.written);
            let step = |written: u32| written / PROGRESS_STEP;
            let done = view.written == install.image_len;
            if step(view.written) != step(published) || (done && published != view.written) {
                published = view.written;
                self.publish_install(install);
            }
        };
        match offer {
            Offer::Release { file, build } => {
                let update = Update {
                    file,
                    build: *build,
                };
                operation.install(update, progress).await
            }
            Offer::Stock {
                manifest,
                downgrade,
                backup,
            } => {
                let restore = Restore {
                    manifest,
                    downgrade,
                    payload: backup.payload(),
                    build: backup.payload_sha256(),
                };
                Ok(operation.restore(restore, progress).await?)
            }
        }
    }

    fn publish_install(&self, install: Install) {
        self.shared.hub.publish(StreamEvent::Install {
            feeder: install.feeder,
            install,
        });
    }
}

/// The install's end as the API names it.
fn failure(error: UpdateError) -> InstallFailure {
    match error {
        UpdateError::Refused(Refusal::Update(refusal)) => InstallFailure::UpdateRefused { refusal },
        UpdateError::Refused(refusal) => InstallFailure::Refused {
            refusal: refusal.into(),
        },
        UpdateError::Stopped(stop) => InstallFailure::Stopped { stop },
        UpdateError::Lost => InstallFailure::Lost,
        UpdateError::Unexpected => InstallFailure::Unexpected,
    }
}

/// The slot, held by the one running install; dropping it frees the slot.
pub(crate) struct Claim {
    slot: Slot,
    install: Install,
}

impl Claim {
    fn take(slot: &Slot, install: Install) -> Result<Self> {
        let mut held = lock(slot);
        if held.running.is_some() {
            return Err(InstallError::Running.into());
        }
        held.running = Some(install);
        Ok(Self {
            slot: Arc::clone(slot),
            install,
        })
    }

    /// Change the running install and return it as it now stands.
    fn update(&mut self, change: impl FnOnce(&mut Install)) -> Install {
        change(&mut self.install);
        lock(&self.slot).running = Some(self.install);
        self.install
    }

    /// Keep the ended `install`, whose end the store did not take.
    fn unsaved(&self, install: Install) {
        lock(&self.slot).unsaved.push(install);
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        lock(&self.slot).running = None;
    }
}
