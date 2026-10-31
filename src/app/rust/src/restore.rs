//! `wellfed_install::restore` on the phone: the platform answered from Dart
//! through the install's adapters ([`crate::install`]), and the thread the
//! run lives on, for the same reasons as an install's. Each ask and event is
//! an install's, and only the end is a restore's own.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use wellfed_client::install::STOCK_VERSION;
use wellfed_client::update_key::EnrollError;
use wellfed_connect::Connection;
use wellfed_install::restore::{
    Offering, Plan, Platform, Restored, Signing, Target, Uncertainty, forget_key as forget, restore,
};
use wellfed_install::{Error, storage};
use wellfed_model::{Answer, Op, UpdatePhase};
use wellfed_session::noise::Keypair;

use crate::api::install::{InstallAnswer, InstallUpdate};
use crate::api::keys::{ClientKey, random};
use crate::api::restore::{
    BackupCheck, RestoreEnd, RestoreSeen, RestoreSpec, RestoreStop, RestoreUpdate, UncertainWhy,
};
use crate::install::{
    AppDiscovery, AppPrompter, AppReport, Asks, BACKUPS, CONFIG, STACK, guarded, stopped, text,
};

pub fn check_backup(path: &Path) -> BackupCheck {
    match storage::load_backup(path) {
        Ok(Some(_)) => BackupCheck::Ready {
            version: STOCK_VERSION.to_string(),
        },
        Ok(None) => BackupCheck::Missing,
        Err(error) => BackupCheck::NotStock {
            detail: text(&error),
        },
    }
}

/// [`forget`], under the installer's folder. A key that stays is found or
/// replaced by the next run, so a failure is left at that.
pub fn forget_key(data_dir: &Path, identity: &[u8]) {
    if let Ok(identity) = <[u8; 32]>::try_from(identity) {
        let _ = forget(&data_dir.join(BACKUPS), &identity);
    }
}

/// How long one read of a feeder may take, session and answer together.
const READ_WAIT: Duration = Duration::from_secs(10);

/// See [`read_restore_feeder`](crate::api::restore::read_restore_feeder).
pub async fn read_feeder(key: &ClientKey, identity: &[u8], address: &str) -> RestoreSeen {
    let (Ok(identity), Ok(address)) = (
        <[u8; 32]>::try_from(identity),
        wellfed_connect::parse_address(address),
    ) else {
        return RestoreSeen::Unread;
    };
    let read = async {
        let mut connection = Connection::open(key.keypair(), identity, address, random().ok()?)
            .await
            .ok()?;
        connection.ask(random().ok()?, &Op::UpdateState).await.ok()
    };
    match tokio::time::timeout(READ_WAIT, read).await {
        Ok(Some(Answer::Update(view))) if view.phase == UpdatePhase::Idle => RestoreSeen::Idle,
        Ok(Some(Answer::Update(_))) => RestoreSeen::Installing,
        _ => RestoreSeen::Unread,
    }
}

/// How a run's error reads on the screens.
pub const fn stop_of(error: &Error) -> RestoreStop {
    match error {
        Error::RestoreEnroll(EnrollError::Unconfirmed) => RestoreStop::NoPress,
        Error::RestoreEnroll(EnrollError::Uncertain | EnrollError::ListUnanswered) => {
            RestoreStop::EnrollmentUnknown
        }
        Error::RestoreEnroll(EnrollError::NotOwner { .. } | EnrollError::Forbidden) => {
            RestoreStop::NotOwner
        }
        Error::RestoreEnroll(EnrollError::NoUpdates) | Error::RestoreNoUpdates => {
            RestoreStop::NoUpdates
        }
        Error::RestoreEnroll(_) => RestoreStop::KeyRefused,
        Error::Connection(_) | Error::RestoreNotFound | Error::NotAnswering { .. } => {
            RestoreStop::NotAnswering
        }
        Error::BackupDamaged { .. } | Error::Read { .. } => RestoreStop::BackupRefused,
        Error::RestoreKey(_) | Error::Create { .. } | Error::Remove { .. } => {
            RestoreStop::KeyFailed
        }
        Error::RestoreKeyGone => RestoreStop::KeyGone,
        Error::RestoreInstallRunning { .. } => RestoreStop::InstallRunning,
        Error::RestoreCancelUnknown => RestoreStop::CancelUnknown,
        Error::RestoreStopped(_) => RestoreStop::Cancelled,
        _ => RestoreStop::Other,
    }
}

const fn stopped_with(case: RestoreStop, detail: String) -> RestoreEnd {
    RestoreEnd::Stopped { case, detail }
}

/// `spec`'s plan, its copy of stock checked before the feeder is touched.
fn plan(spec: RestoreSpec) -> Result<Plan, RestoreEnd> {
    let refused = |detail| stopped_with(RestoreStop::BackupRefused, detail);
    let backup = storage::load_backup(Path::new(&spec.backup))
        .map_err(|error| refused(text(&error)))?
        .ok_or_else(|| refused(format!("no file at {}", spec.backup)))?;
    let identity: [u8; 32] = spec.identity.as_slice().try_into().map_err(|_| {
        stopped_with(
            RestoreStop::Other,
            "a feeder's identity is 32 bytes".to_string(),
        )
    })?;

    Plan::new(
        &backup,
        Target {
            identity,
            address: spec
                .address
                .as_deref()
                .and_then(|address| wellfed_connect::parse_address(address).ok()),
            backups: PathBuf::from(spec.data_dir).join(BACKUPS),
            signing: Signing::Fresh,
            version: None,
        },
    )
    .map_err(|error| refused(text(&error)))
}

/// One run's thread, and the handles that reach it.
pub struct RestoreThread {
    asks: Arc<Asks>,
    stop: watch::Sender<bool>,
}

impl Default for RestoreThread {
    fn default() -> Self {
        Self::new()
    }
}

impl RestoreThread {
    pub fn new() -> Self {
        Self {
            asks: Arc::new(Asks::new()),
            stop: watch::channel(false).0,
        }
    }

    /// Start `spec` on its own thread. `deliver` hears the run to its end and
    /// returns `false` once nobody listens. A run starts once: a second call,
    /// with no spec left, reports that and ends.
    pub fn start(
        &self,
        spec: Option<RestoreSpec>,
        keys: Keypair,
        deliver: impl Fn(RestoreUpdate) -> bool + Send + Sync + 'static,
    ) {
        let plan = match spec {
            Some(spec) => plan(spec),
            None => Err(stopped_with(
                RestoreStop::Other,
                "this restore already ran".to_string(),
            )),
        };
        self.start_plan(plan, keys, deliver);
    }

    /// [`Self::start`] on a plan already made, or the end that stopped it.
    pub fn start_plan(
        &self,
        plan: Result<Plan, RestoreEnd>,
        keys: Keypair,
        deliver: impl Fn(RestoreUpdate) -> bool + Send + Sync + 'static,
    ) {
        let deliver = Arc::new(deliver);
        let heard = deliver.clone();
        *self.asks.deliver() = Some(Box::new(move |update| match update {
            InstallUpdate::Event { event } => heard(RestoreUpdate::Event { event }),
            InstallUpdate::Ask { id, ask } => heard(RestoreUpdate::Ask { id, ask }),
            // A restore's end is its own, below.
            InstallUpdate::Ended { .. } => false,
        }));

        let asks = self.asks.clone();
        let stop = self.stop.subscribe();
        let last = deliver.clone();
        let offering = Offering::default();
        let thread = std::thread::Builder::new()
            .name("wellfed-restore".into())
            .stack_size(STACK)
            .spawn(move || {
                let shown = offering.clone();
                let end = guarded(
                    || match plan {
                        Ok(plan) => run_on_this_thread(plan, keys, asks.clone(), stop, offering),
                        Err(end) => end,
                    },
                    |detail| {
                        // A panic says nothing about an offer that may be out.
                        if shown.may_be_out() {
                            RestoreEnd::Uncertain {
                                why: UncertainWhy::Failed,
                                detail,
                            }
                        } else {
                            stopped_with(RestoreStop::Other, detail)
                        }
                    },
                );
                last(RestoreUpdate::Ended { end });
                // Dropping the sink closes Dart's stream.
                *asks.deliver() = None;
            });
        if let Err(error) = thread {
            deliver(RestoreUpdate::Ended {
                end: stopped_with(
                    RestoreStop::Other,
                    format!("no thread for the restore: {error}"),
                ),
            });
        }
    }

    pub fn answer(&self, id: u32, answer: InstallAnswer) {
        self.asks.answer(id, answer);
    }

    pub fn stop(&self) {
        self.stop.send_replace(true);
    }
}

fn run_on_this_thread(
    plan: Plan,
    keypair: Keypair,
    asks: Arc<Asks>,
    stop: watch::Receiver<bool>,
    offering: Offering,
) -> RestoreEnd {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            return stopped_with(
                RestoreStop::Other,
                format!("no runtime for the restore: {error}"),
            );
        }
    };

    let platform = Platform {
        prompter: AppPrompter(asks.clone()),
        discovery: AppDiscovery(asks.clone()),
        keypair,
        report: AppReport(asks),
        offering,
    };
    let (identity, backups) = plan.target();
    let (identity, backups) = (*identity, backups.to_path_buf());
    match runtime.block_on(restore(plan, platform, stopped(stop))) {
        Ok(Restored::Activated) => {
            // Stock is going back, so nothing else will say what became of a
            // trial this feeder's build already answered for, and its next
            // report must not read as a request that never left stock.
            if let Some(data) = backups.parent() {
                let config = data.join(CONFIG);
                let _ = wellfed_install::stock::feeder_restored(&config, &backups, &identity);
            }
            RestoreEnd::Activated
        }
        Ok(Restored::Uncertain(why)) => match why {
            Uncertainty::Gone => RestoreEnd::Uncertain {
                why: UncertainWhy::Gone,
                detail: "the feeder stopped answering after the offer went out".to_string(),
            },
            Uncertainty::Stopped(by) => RestoreEnd::Uncertain {
                why: UncertainWhy::Stopped,
                detail: format!("stopped on {by} as the offer was going out, or after it had"),
            },
            Uncertainty::Failed(detail) => RestoreEnd::Uncertain {
                why: UncertainWhy::Failed,
                detail,
            },
        },
        Ok(Restored::Refused(error)) => stopped_with(RestoreStop::Refused, text(&error)),
        Err(error) => stopped_with(stop_of(&error), text(&error)),
    }
}
