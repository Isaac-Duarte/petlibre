//! Putting stock back, as Dart holds it: `wellfed_install::restore` on a
//! thread of its own ([`crate::restore`]). A run tells Dart everything through
//! one stream of [`RestoreUpdate`]s, its failures as values on it, as an
//! install's do ([`crate::api::install`]), and shares an install's events and
//! asks. A run asks only where the feeder is, once it stops answering.

use std::sync::Mutex;

use flutter_rust_bridge::frb;

use crate::api::install::{InstallAnswer, InstallAsk, InstallEvent};
use crate::api::keys::ClientKey;
use crate::frb_generated::StreamSink;
use crate::restore::{self, RestoreThread};

/// What a restore needs, read and checked before the feeder is touched.
pub struct RestoreSpec {
    /// The feeder's identity.
    pub identity: Vec<u8>,
    /// Where it last answered (`ip:port`), tried before NSD.
    pub address: Option<String>,
    /// The installer's own folder, where the backups are kept and the
    /// restore's key lives until the restore ends.
    pub data_dir: String,
    /// The copy of stock to put back: the kept backup, or a file the owner
    /// picked. It goes through the same checks either way.
    pub backup: String,
}

/// Whether a file is a copy of stock this app can put back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackupCheck {
    /// The pinned image, byte for byte.
    Ready { version: String },
    /// No file there.
    Missing,
    /// Not stock's pinned image: damaged, partial or foreign.
    NotStock { detail: String },
}

/// Check the copy of stock at `path`, as a run will.
#[frb(sync)]
pub fn check_backup(path: String) -> BackupCheck {
    restore::check_backup(std::path::Path::new(&path))
}

/// Forget the restore's key for the feeder with `identity`, as an owner
/// leaving the screen does. A failure changes nothing they can act on: the
/// next run finds the key, or replaces it.
#[frb(sync)]
pub fn forget_restore_key(data_dir: String, identity: Vec<u8>) {
    restore::forget_key(std::path::Path::new(&data_dir), &identity);
}

/// Why a restore stopped with nothing put back, in the cases the screens tell
/// apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreStop {
    /// Nobody pressed Dispense in the feeder's window. Nothing was offered,
    /// and a new run may go again.
    NoPress,
    /// The enrollment went unanswered and the feeder's list did not show the
    /// key. Nothing was offered, and a new run finds out without resending it.
    EnrollmentUnknown,
    /// The feeder would not take the key: another waits for its press, or its
    /// client slots are full. Nothing was offered.
    KeyRefused,
    /// Only the feeder's owner can do this, and this phone is not.
    NotOwner,
    /// The feeder does not answer this phone. Nothing was offered.
    NotAnswering,
    /// This firmware takes no updates over the device API.
    NoUpdates,
    /// The copy is not stock's pinned image, or could not be read.
    BackupRefused,
    /// This phone could not make or keep the restore's key: its own storage,
    /// not the backup. Nothing was offered.
    KeyFailed,
    /// The restore's key file went away just before the offer, as when an
    /// earlier restore screen was left and forgot it. Nothing was offered,
    /// and a new run starts clean.
    KeyGone,
    /// An install is running on the feeder now, and it was left alone. Nothing
    /// was offered.
    InstallRunning,
    /// An earlier install was left pending, and cancelling it did not clearly
    /// work. Nothing was offered.
    CancelUnknown,
    /// The feeder refused the offer, or the install stopped. It is not
    /// offered again, and the feeder keeps running Wellfed.
    Refused,
    /// The owner left, with nothing offered.
    Cancelled,
    Other,
}

/// Why a restore's end is not known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UncertainWhy {
    /// The feeder stopped answering after the offer went out.
    Gone,
    /// The owner stopped it as the offer was going out, or after it had.
    Stopped,
    /// This run failed as the offer was going out, or after it had.
    Failed,
}

/// How a restore ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestoreEnd {
    /// Bank 1 is activated; the feeder restarts into stock.
    Activated,
    /// It may run stock already, or still Wellfed. Never offered again by
    /// this run.
    Uncertain { why: UncertainWhy, detail: String },
    Stopped {
        case: RestoreStop,
        /// The run's own words, for the screen's small print.
        detail: String,
    },
}

pub enum RestoreUpdate {
    Event {
        event: InstallEvent,
    },
    Ask {
        id: u32,
        ask: InstallAsk,
    },
    /// The last update of a run.
    Ended {
        end: RestoreEnd,
    },
}

/// One restore, run once.
#[frb(opaque)]
pub struct RestoreRun {
    run: RestoreThread,
    spec: Mutex<Option<RestoreSpec>>,
    keys: wellfed_session::noise::Keypair,
}

impl RestoreRun {
    /// Nothing starts until [`Self::run`].
    #[frb(sync)]
    pub fn new(key: &ClientKey, spec: RestoreSpec) -> Self {
        Self {
            run: RestoreThread::new(),
            spec: Mutex::new(Some(spec)),
            keys: key.keypair().clone(),
        }
    }

    /// Start the run on its own thread; `updates` hears it to its end,
    /// [`RestoreUpdate::Ended`] last.
    pub async fn run(&self, updates: StreamSink<RestoreUpdate>) {
        let spec = self.spec.lock().ok().and_then(|mut spec| spec.take());
        self.run.start(spec, self.keys.clone(), move |update| {
            updates.add(update).is_ok()
        });
    }

    /// Answer the ask `id`. An answer nobody waits for any more is dropped.
    #[frb(sync)]
    pub fn answer(&self, id: u32, answer: InstallAnswer) {
        self.run.answer(id, answer);
    }

    /// End the run. Once an offer may be out it ends uncertain, and nothing
    /// is sent again.
    #[frb(sync)]
    pub fn stop(&self) {
        self.run.stop();
    }
}
