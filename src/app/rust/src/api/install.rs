//! The install from stock, as Dart holds it: `wellfed_install::run` on a
//! thread of its own ([`crate::install`]), with the screens' side of it here.
//! A run tells Dart everything through one stream of [`InstallUpdate`]s;
//! what it needs Dart to do (a person's answer, a radio write, an NSD
//! lookup) arrives as an [`InstallUpdate::Ask`], which Dart answers by its
//! id with [`InstallRun::answer`].
//!
//! Failures travel as values on the stream, never through `add_error`:
//! flutter_rust_bridge decodes a stream's errors as `AnyhowException`
//! whatever Rust encoded, so a typed error sent that way reaches Dart as
//! garbage.

use std::sync::Mutex;

use flutter_rust_bridge::frb;
use wellfed_client::install::DnsCheck;
use wellfed_install::build_info::BUILD;

use crate::api::keys::ClientKey;
use crate::api::model::SetupRefusal;
use crate::api::setup::WifiCredentials;
use crate::frb_generated::StreamSink;
use crate::install::{self, Listener, Run};

/// How the feeder finds this phone.
pub enum InstallStart {
    /// The owner's resolver answers stock's broker `name` with this phone.
    Dns { name: String },
    /// The replacement is already on bank 0 from an earlier run: start at
    /// pairing, and ask stock nothing.
    Bank0,
}

/// Everything a run needs, read and checked before the feeder is touched.
pub struct InstallSpec {
    pub start: InstallStart,
    /// This phone's address on the owner's Wi-Fi: where the broker and the
    /// image server listen.
    pub phone_ip: String,
    /// The network a consumer image is given over Bluetooth setup.
    pub home: Option<WifiCredentials>,
    /// How feeders list this phone.
    pub label: String,
    /// What this phone calls the feeder until the owner renames it.
    pub feeder_name: String,
    /// The feeders this phone knows, by identity (hex).
    pub known_feeders: Vec<String>,
    /// The app's own file, where a feeder that enrolled this key is saved.
    pub store_path: String,
    /// The folder the APK's image was unpacked into ([`check_image`]).
    pub image_dir: String,
    /// The installer's own folder: the stock server's config and records,
    /// and the backups.
    pub data_dir: String,
    /// Have the stock server set stock's clock even when its own reading is
    /// unknown or far off. Stock may then run meals that came due in the
    /// jump, so the owner chooses this.
    pub calibrate_clock: bool,
    /// Promote even if stock's plans cannot be read.
    pub without_stock_plans: bool,
    /// Forget the feeder the broker trusted before: the owner said it was
    /// not theirs, or two answered.
    pub forget_enrollment: bool,
}

/// What a resolver answered for stock's broker name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DnsResult {
    /// It answers with this phone alone.
    Points,
    /// It answers with this phone and `others`: stock may take any of them.
    Mixed { others: Vec<String> },
    /// It answers with `others` only.
    Elsewhere { others: Vec<String> },
    /// It has no answer for the name.
    Unresolved,
    /// The name cannot be a broker's name.
    BadName,
}

impl From<DnsCheck> for DnsResult {
    fn from(check: DnsCheck) -> Self {
        let text =
            |addresses: Vec<std::net::IpAddr>| addresses.iter().map(ToString::to_string).collect();
        match check {
            DnsCheck::Points => Self::Points,
            DnsCheck::Mixed(others) => Self::Mixed {
                others: text(others),
            },
            DnsCheck::Elsewhere(others) => Self::Elsewhere {
                others: text(others),
            },
            DnsCheck::Unresolved => Self::Unresolved,
        }
    }
}

/// Why the run still waits for stock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitReason {
    /// No feeder has reached the broker yet.
    NoFeeder,
    /// The stock server's own records cannot be read.
    Unreadable,
    /// A feeder enrolled but its session is not up.
    Offline,
    /// Stock has not reported its version yet.
    NoVersion,
    /// Stock is not on its mains adapter.
    NotOnAdapter,
}

/// What became of stock's plans at the end of the run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlansOutcome {
    /// Set on the replacement.
    Set,
    /// Stock held none.
    NoneHeld,
    /// None converted: nothing was set.
    NoneConverted,
    /// Neither read by this run nor kept by an earlier one.
    NotRead,
    /// Left behind, as the owner chose.
    Skipped,
}

/// One step of progress, for the screens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallEvent {
    DnsChecked {
        result: DnsResult,
    },
    /// The broker listens, with its enrollment window open.
    StockServerStarted,
    /// The feeder restarts; the run waits for stock at the broker.
    AwaitingStock,
    Waiting {
        why: WaitReason,
    },
    /// Stock connected but its startup reports were lost: the owner
    /// restarts it once more.
    VersionLost,
    /// Stock's server stopped on its own, so stock is not waited for.
    StockServerEnded {
        error: String,
    },
    /// The one install request is going out.
    Requesting {
        build: String,
    },
    /// Stock's download, in percent.
    StockProgress {
        percent: u8,
    },
    /// Stock went quiet: it restarts into the replacement.
    StockRestarting,
    /// The replacement knows this phone's key already.
    KeyKnown,
    /// The replacement has no network yet, so Bluetooth setup starts.
    SetupStarting,
    /// No feeder was found in Bluetooth setup mode within the wait.
    NoSetupFeeder {
        error: String,
    },
    /// Key `code` (panel positions, 1 Lock to 4 Reset) in within `seconds`.
    Code {
        code: Vec<u8>,
        seconds: u16,
    },
    /// Press Dispense, within the feeder's window, to enroll an update key.
    PressDispense,
    /// This phone is the feeder's owner. With `rebooting`, it restarts onto
    /// the owner's Wi-Fi first; with `address`, it already answers there.
    Paired {
        identity: Vec<u8>,
        address: Option<String>,
        rebooting: bool,
    },
    /// The replacement joined the owner's network.
    Joined {
        address: String,
    },
    /// The replacement answers with `build` from `bank`.
    ReplacementAnswers {
        build: String,
        bank: u8,
    },
    /// Bank 1 is being read from the feeder; it holds `total` bytes.
    ReadingBackup {
        total: u32,
    },
    /// Bytes of bank 1 read so far.
    BackupRead {
        bytes: u64,
        total: u32,
    },
    /// A checked copy of stock is kept at `path`.
    BackupKept {
        path: String,
    },
    /// Stock's plans, as the replacement will run them.
    StockPlans {
        converted: u8,
        refused: u8,
    },
    /// The owner chose to go on without stock's plans.
    WithoutStockPlans,
    /// Bank 0 writes the replacement into bank 1.
    Promoting,
    /// The push's progress.
    Written {
        written: u32,
        length: u32,
    },
    /// The push's session dropped; it resumes on a new one.
    SessionDropped,
    /// Bank 1 is activated; the feeder restarts into it.
    Activated,
    /// Stock's converted plans go to the replacement, once.
    PuttingPlans,
    /// The install is confirmed on bank 1.
    Installed {
        build: String,
    },
    /// What became of stock's plans.
    Plans {
        result: PlansOutcome,
    },
    /// Something only a log needs: a read the run tries again, a record it
    /// could not remove.
    Note {
        text: String,
    },
}

/// What a run needs Dart to do. Each is answered once, by its id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallAsk {
    /// Answered done once the owner's resolver answers `name` with `broker`.
    SetOverride { name: String, broker: String },
    /// Answered done once the owner was told: the broker already listens.
    RestartFeeder,
    /// Trust on first use admitted `device`, connecting from `address`; the
    /// owner answers whether it is theirs.
    RecognizeFeeder { device: String, address: String },
    /// A bench image pairs through the recovery window: answered done once
    /// the owner opened it.
    OpenRecoveryWindow,
    /// Find the first feeder advertising Bluetooth setup, connect, and turn
    /// notifications on. Answered done when ready, or failed with why not;
    /// each notification then goes to [`InstallRun::notified`].
    OpenLink,
    /// Write these pieces in order, each with a write request.
    WriteLink { pieces: Vec<Vec<u8>> },
    /// Hang up.
    CloseLink,
    /// Where the feeder with `identity` (hex) advertises now, if it does.
    Locate { identity: String },
}

/// Dart's answer to an [`InstallAsk`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallAnswer {
    Done,
    /// A person's yes or no.
    Yes {
        yes: bool,
    },
    /// Where the feeder is (`ip:port`), or `None`.
    Located {
        address: Option<String>,
    },
    /// It could not be done; `reason` reaches the person as written.
    Failed {
        reason: String,
    },
}

/// A write whose outcome is not settled. It may have run, and it is never
/// sent again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UncertainWrite {
    /// The update key's enrollment.
    UpdateKey,
    /// The signed update, pushed into bank 1.
    Promotion,
    /// Stock's converted plans.
    Plans,
}

/// Why a run stopped, in the cases the screens tell apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopCase {
    /// The install request may have reached stock: never resent, and
    /// [`InstallListener`] listens for what became of it.
    Uncertain,
    /// A later write whose outcome is not settled.
    UncertainWrite {
        write: UncertainWrite,
    },
    /// Stock reports a version other than 1.1.15. Nothing was sent.
    OtherFirmware {
        version: String,
    },
    /// A second device tried the broker. Nothing was sent.
    TwoFeeders,
    /// No verified copy of stock: the promotion, which overwrites the
    /// feeder's only copy, is refused.
    BackupMismatch,
    /// The owner said the feeder is not theirs. Nothing was sent.
    NotYourFeeder,
    /// No feeder reached the broker in time. Nothing was sent.
    NoFeederCame,
    /// A request from an earlier run has not been resolved, so the server
    /// sent none; [`InstallListener`] can resolve it.
    EarlierRequestOpen,
    /// Stock refused or failed the install, and keeps running stock.
    StockRefused {
        code: Option<u32>,
    },
    /// Stock refused before downloading, most likely because its clock is
    /// not set.
    ClockUnset,
    /// No feeder in Bluetooth setup mode was found.
    NoSetupFeeder,
    /// Bluetooth setup failed: `refusal` when the feeder said why.
    SetupFailed {
        refusal: Option<SetupRefusal>,
    },
    /// The feeder this install paired with does not answer.
    NotAnswering,
    /// Stock's plans could not be read, so nothing was promoted.
    PlansUnreadable,
    /// The promotion was refused or failed; the feeder runs bank 0.
    PromotionFailed,
    /// Bank 1's trial boot fell back to bank 0.
    FellBack,
    /// The replacement is installed and confirmed, but stock's plans were
    /// not applied.
    PlansNotApplied,
    /// The stock server could not start: its port is taken, say.
    ServerFailed,
    /// The owner stopped the run, with nothing unsettled.
    Cancelled,
    Other,
}

/// What a new run may do after a stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resume {
    /// Start over: stock still runs.
    FromStart,
    /// Pick up at pairing: the replacement runs from bank 0.
    FromBank0,
    Nothing,
}

/// How a run ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallEnd {
    Done,
    Stopped {
        case: StopCase,
        resume: Resume,
        /// The run's own words, for the screen's small print.
        detail: String,
    },
}

pub enum InstallUpdate {
    Event {
        event: InstallEvent,
    },
    Ask {
        id: u32,
        ask: InstallAsk,
    },
    /// The last update of a run.
    Ended {
        end: InstallEnd,
    },
}

/// One install, run once.
#[frb(opaque)]
pub struct InstallRun {
    run: Run,
    spec: Mutex<Option<InstallSpec>>,
    keys: wellfed_session::noise::Keypair,
}

impl InstallRun {
    /// Nothing starts until [`Self::run`].
    #[frb(sync)]
    pub fn new(key: &ClientKey, spec: InstallSpec) -> Self {
        Self {
            run: Run::new(),
            spec: Mutex::new(Some(spec)),
            keys: key.keypair().clone(),
        }
    }

    /// Start the run on its own thread; `updates` hears it to its end,
    /// [`InstallUpdate::Ended`] last.
    pub async fn run(&self, updates: StreamSink<InstallUpdate>) {
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

    /// One notification's bytes from the open link.
    #[frb(sync)]
    pub fn notified(&self, bytes: Vec<u8>) {
        self.run.notified(bytes);
    }

    /// The open link dropped.
    #[frb(sync)]
    pub fn link_lost(&self) {
        self.run.link_lost();
    }

    /// End the run through its cleanup: stock's server stops, and the end
    /// names any write whose outcome had not settled.
    #[frb(sync)]
    pub fn stop(&self) {
        self.run.stop();
    }
}

/// What [`InstallListener`] tells Dart.
pub enum ListenUpdate {
    /// It looked again.
    Tick {
        seconds: u32,
        /// Stock has reported its version since the request.
        stock: bool,
    },
    /// The device API answers at `address`: stock installed the replacement.
    Replacement { address: String },
    /// Stock still runs. With `resolved`, the request is closed and a new
    /// run may ask again.
    StockStayed { resolved: bool },
    /// Nothing came, for the whole wait: still not known.
    Silent,
    /// The listener could not run.
    Failed { detail: String },
    /// The owner, or the replacement advertising setup, stopped it.
    Stopped,
}

/// The listener after an uncertain request: stock's server with no trial
/// enabled, so nothing it does can send an install.
#[frb(opaque)]
pub struct InstallListener {
    listener: Listener,
    data_dir: String,
}

impl InstallListener {
    #[frb(sync)]
    pub fn new(data_dir: String) -> Self {
        Self {
            listener: Listener::new(),
            data_dir,
        }
    }

    /// How long it listens, in seconds.
    #[frb(sync)]
    pub fn wait_seconds() -> u32 {
        install::listen_seconds()
    }

    /// Start listening on its own thread, for the whole wait.
    pub async fn run(&self, updates: StreamSink<ListenUpdate>) {
        self.listener
            .start(&self.data_dir, install::listen_wait(), move |update| {
                updates.add(update).is_ok()
            });
    }

    #[frb(sync)]
    pub fn stop(&self) {
        self.listener.stop();
    }
}

/// How the APK's image pairs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageProvisioning {
    /// A consumer image: Bluetooth setup.
    Consumer,
    /// A bench image: it joins its build network, and pairs through the
    /// recovery window.
    Bench,
}

/// Whether the APK holds an image this build may install.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageCheck {
    Ready {
        provisioning: ImageProvisioning,
        /// The startup identity, 64 hex.
        build: String,
        version: u32,
    },
    /// This APK was built without an image.
    Absent,
    /// No pin names the image this build installs.
    NotPinned,
    /// The image is not the one pinned, or does not match its own review.
    Mismatch { detail: String },
}

/// The image unpacked in `dir`, checked against this build's pin and against
/// its own review.
#[frb(sync)]
pub fn check_image(dir: String) -> ImageCheck {
    install::check_image(std::path::Path::new(&dir), install::pin().as_ref())
}

/// Whether this build may send stock's install at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildCheck {
    Reviewable,
    /// Built from a dirty tree, or without its commit: the sources of what
    /// would send the install cannot be reviewed.
    Unreviewed {
        commit: String,
    },
}

/// Stock's install is held to a build whose sources can be reviewed: a
/// clean commit of the crates that decide and send it.
#[frb(sync)]
pub fn check_build() -> BuildCheck {
    match BUILD.require_reviewed() {
        Ok(()) => BuildCheck::Reviewable,
        Err(unreviewed) => BuildCheck::Unreviewed {
            commit: unreviewed.commit.to_string(),
        },
    }
}

/// Look stock's broker `name` up from this phone and say what it found
/// against `phone_ip`. Advice only: this phone may ask another resolver
/// than the feeder does, so a run waits for stock whatever it says.
pub async fn check_dns(name: String, phone_ip: String) -> DnsResult {
    install::check_dns(&name, &phone_ip).await
}

/// Whether an earlier run left an install to pick up at pairing.
#[frb(sync)]
pub fn pending_install(data_dir: String) -> bool {
    install::pending(std::path::Path::new(&data_dir))
}
