//! `wellfed_install` on the phone: the platform traits answered from Dart,
//! and the thread the run lives on.
//!
//! The run's future is not `Send`, so it is driven with `block_on` on a
//! current-thread runtime on a thread of its own, and stock's server and
//! image server run as tasks on that runtime. Nothing here depends on the
//! bridge's own runtime, and the bridge is only a closure that delivers an
//! update, so these tests run it with no Flutter in sight.
//!
//! Every wait on Dart is an [`InstallAsk`] answered by id. A stream with
//! nobody on it fails the ask at once, which ends the run through its own
//! cleanup rather than leaving it waiting for a person who is gone.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot, watch};
use wellfed_client::install::{
    DnsOverride, InstallError, Phase, Provision, STOCK_BROKER_PORT, STOCK_PAYLOAD_LEN, SetupFrame,
    manifest_build, manifest_version,
};
use wellfed_client::setup::{Credentials, Deframer, frame};
use wellfed_formats::bench::prompt::Prompt;
use wellfed_formats::canonical::sha256;
use wellfed_formats::manifest::Provisioning;
use wellfed_install::event::{Enrollment as KeyEnrollment, PlansResult, Wait};
use wellfed_install::listen::{self, Listened, UNCERTAIN_WAIT};
use wellfed_install::{
    Announcement, Api, Bind, Bluetooth, ClockPolicy, Discovery, Error, Event, Keys, Plan, Platform,
    PlatformError, Prompter, Refusal, Rejection, Report, Review, SetupLink, Signer, Start,
    StockOptions, Stop, Unjoined, Unsettled,
};
use wellfed_session::noise::Keypair;

use crate::api::install::{
    DnsResult, ImageCheck, ImageProvisioning, InstallAnswer, InstallAsk, InstallEnd, InstallEvent,
    InstallSpec, InstallStart, InstallUpdate, ListenUpdate, PendingInstall, PlansOutcome,
    RequestRefusal, Resume, StopCase, UncertainWrite, WaitReason,
};
use crate::api::setup::CONTROL_PIECE;
use crate::api::store::{KnownFeeder, load_store, save_store};

/// The run's future is deep; a debug build's frames are large.
pub(crate) const STACK: usize = 16 * 1024 * 1024;
/// How long Dart's NSD lookup may take before the run looks elsewhere.
const LOCATE_WAIT: Duration = Duration::from_secs(15);
/// A link the feeder already dropped may stall hanging up.
const CLOSE_WAIT: Duration = Duration::from_secs(3);
/// The numeric account the setup frame carries. Stock keeps it beside the
/// broker and checks only its form; no vendor account exists to name here.
const FRAME_ACCOUNT: &str = "1";
/// How long the broker trusts the first stock feeder that connects.
const ENROLL: Duration = Duration::from_secs(15 * 60);

/// Where the installer keeps its files under its data directory.
const CONFIG: &str = "installer.json";
pub(crate) const BACKUPS: &str = "backups";

/// The image's own files, in the folder the APK's were unpacked into.
pub(crate) const UPDATE_MANIFEST: &str = "update.manifest";
const REVIEW_FILE: &str = "review.json";

/// The image a build installs: its review (which binds the image and its
/// build manifest by their digests), the signed update manifest bank 0
/// promotes with, and that manifest's version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pin {
    pub review_sha256: &'static str,
    pub manifest_sha256: &'static str,
    pub version: u32,
    /// A release's pin, which takes a consumer image and nothing else.
    pub release: bool,
}

/// The release's pin, set when a release image exists; it can only be
/// signed with the offline release key, which no checkout holds.
const RELEASE_PIN: Option<Pin> = None;

/// This build's pin: the release's, or (for a bench build, whose image is a
/// build-time input) `WELLFED_INSTALLER_PIN` as `make app-installer-input`
/// prints it. Compiled in, so an image swapped in the APK afterwards fails
/// the check; a committed release pin is never overridden.
pub fn pin() -> Option<Pin> {
    RELEASE_PIN.or_else(|| parse_pin(option_env!("WELLFED_INSTALLER_PIN")?))
}

/// `review_sha256:manifest_sha256:version`.
pub fn parse_pin(text: &'static str) -> Option<Pin> {
    let mut parts = text.split(':');
    let (review_sha256, manifest_sha256, version) = (parts.next()?, parts.next()?, parts.next()?);
    let hashes = [review_sha256, manifest_sha256];
    if parts.next().is_some()
        || !hashes
            .iter()
            .all(|hash| wellfed_formats::hex::is_sha256(hash))
    {
        return None;
    }
    Some(Pin {
        review_sha256,
        manifest_sha256,
        version: version.parse().ok()?,
        release: false,
    })
}

/// The image in `dir` against `pin`, and against its own review.
pub fn check_image(dir: &Path, pin: Option<&Pin>) -> ImageCheck {
    let read = |name: &str| std::fs::read(dir.join(name)).ok();
    let (Some(review), Some(manifest)) = (read(REVIEW_FILE), read(UPDATE_MANIFEST)) else {
        return ImageCheck::Absent;
    };
    let Some(pin) = pin else {
        return ImageCheck::NotPinned;
    };

    let mismatch = |detail: String| ImageCheck::Mismatch { detail };
    if sha256(&review) != pin.review_sha256 {
        return mismatch("review.json is not the pinned one".to_string());
    }
    if sha256(&manifest) != pin.manifest_sha256 {
        return mismatch("the update manifest is not the pinned one".to_string());
    }
    if manifest_version(&manifest) != Some(pin.version) {
        return mismatch(format!(
            "the update manifest is not version {}",
            pin.version
        ));
    }
    // The image server's own load: the image and the build manifest against
    // the review's digests.
    if let Err(error) = Review::check_served(dir) {
        return mismatch(text(&error));
    }
    let opened = match Review::open(dir) {
        Ok(opened) => opened,
        Err(error) => return mismatch(text(&error)),
    };
    // Refused here rather than at the promotion, with stock already overwritten
    // in bank 0 and a manifest no feeder would take for this build.
    let names = manifest_build(&manifest).map(|build| wellfed_formats::hex::encode(&build));
    if names.as_deref() != Some(opened.build()) {
        return mismatch("the update manifest names another build".to_string());
    }
    let provisioning = match opened.provisioning() {
        Provisioning::Consumer => ImageProvisioning::Consumer,
        Provisioning::Bench if pin.release => {
            return mismatch("a release installs a consumer image, not a bench one".to_string());
        }
        Provisioning::Bench => ImageProvisioning::Bench,
    };
    ImageCheck::Ready {
        provisioning,
        build: opened.build().to_string(),
        version: pin.version,
    }
}

pub async fn check_dns(name: &str, phone_ip: &str) -> DnsResult {
    let Ok(ip) = phone_ip.parse::<IpAddr>() else {
        return DnsResult::Unresolved;
    };
    let Ok(dns) = DnsOverride::new(name, SocketAddr::new(ip, STOCK_BROKER_PORT)) else {
        return DnsResult::BadName;
    };
    let lookup = format!("{}:{STOCK_BROKER_PORT}", dns.name());
    let resolved: Vec<IpAddr> = tokio::net::lookup_host(lookup)
        .await
        .map(|addresses| addresses.map(|address| address.ip()).collect())
        .unwrap_or_default();
    dns.check(&resolved).into()
}

pub fn pending(data_dir: &Path) -> PendingInstall {
    if wellfed_install::storage::resumable(&data_dir.join(BACKUPS)) {
        PendingInstall::Paired
    } else if wellfed_install::stock::open_request(&data_dir.join(CONFIG)) {
        PendingInstall::RequestOpen
    } else {
        PendingInstall::Nothing
    }
}

pub fn kept_backup(data_dir: &Path, identity: &[u8]) -> Option<String> {
    let identity: [u8; 32] = identity.try_into().ok()?;
    let path = wellfed_install::storage::kept_backup(&data_dir.join(BACKUPS), &identity)?;
    Some(path.display().to_string())
}

pub fn listen_seconds() -> u32 {
    u32::try_from(UNCERTAIN_WAIT.as_secs()).unwrap_or(u32::MAX)
}

pub const fn listen_wait() -> Duration {
    UNCERTAIN_WAIT
}

/// `error` and its causes on one line.
pub(crate) fn text(error: &(dyn std::error::Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// Hands an update to Dart; `false` once nobody listens.
pub(crate) type Deliver = Box<dyn Fn(InstallUpdate) -> bool + Send>;

/// One outstanding set of asks, shared between the run's adapters and the
/// bridge's answers.
pub(crate) struct Asks {
    /// Taken at the end of the run, which closes Dart's stream.
    deliver: Mutex<Option<Deliver>>,
    waiting: Mutex<HashMap<u32, oneshot::Sender<InstallAnswer>>>,
    next: AtomicU32,
    /// The open link's notifications; `None` when none is open.
    link: Mutex<Option<mpsc::UnboundedSender<Option<Vec<u8>>>>>,
}

impl Asks {
    pub(crate) fn new() -> Self {
        Self {
            deliver: Mutex::new(None),
            waiting: Mutex::new(HashMap::new()),
            next: AtomicU32::new(1),
            link: Mutex::new(None),
        }
    }

    /// The delivery, even after a panic while it was held: the last update
    /// is what tells the screen that happened.
    pub(crate) fn deliver(&self) -> MutexGuard<'_, Option<Deliver>> {
        self.deliver.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Hand `update` to Dart; `false` once nobody listens.
    fn update(&self, update: InstallUpdate) -> bool {
        crate::trace::update(&update);
        self.deliver()
            .as_ref()
            .is_some_and(|deliver| deliver(update))
    }

    /// Ask Dart, and wait for its answer. The run's stop drops this future,
    /// which leaves nothing behind but an answer nobody takes.
    async fn ask(&self, ask: InstallAsk) -> Result<InstallAnswer, PlatformError> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (answer, answered) = oneshot::channel();
        self.waiting
            .lock()
            .map_err(|_| "the app's state was lost")?
            .insert(id, answer);
        if !self.update(InstallUpdate::Ask { id, ask }) {
            return Err("nobody is on the app's screen to answer".into());
        }
        answered
            .await
            .map_err(|_| "nobody is on the app's screen to answer".into())
    }

    async fn done(&self, ask: InstallAsk) -> Result<(), PlatformError> {
        match self.ask(ask).await? {
            InstallAnswer::Done => Ok(()),
            InstallAnswer::Failed { reason } => Err(reason.into()),
            other => Err(format!("the app answered {other:?}").into()),
        }
    }

    pub(crate) fn answer(&self, id: u32, answer: InstallAnswer) {
        crate::trace::answer(id, &answer);
        let waiting = self.waiting.lock().ok().and_then(|mut w| w.remove(&id));
        if let Some(waiting) = waiting {
            // The run stopped waiting: nothing to tell.
            let _ = waiting.send(answer);
        }
    }

    fn link_event(&self, event: Option<Vec<u8>>) {
        if let Ok(link) = self.link.lock()
            && let Some(link) = link.as_ref()
        {
            // The link closed on the run's side: nothing to tell.
            let _ = link.send(event);
        }
    }
}

/// The person at the feeder, on Dart's screens.
pub(crate) struct AppPrompter(pub(crate) Arc<Asks>);

impl Prompter for AppPrompter {
    async fn done(&mut self, prompt: Prompt) -> Result<(), PlatformError> {
        let ask = match prompt {
            Prompt::SetOverride { name, broker } => InstallAsk::SetOverride {
                name,
                broker: broker.to_string(),
            },
            Prompt::RestartFeeder => InstallAsk::RestartFeeder,
            Prompt::OpenRecoveryWindow => InstallAsk::OpenRecoveryWindow,
            Prompt::SetupMode => InstallAsk::JoinFeederNetwork,
            Prompt::JoinBack => InstallAsk::LeaveFeederNetwork,
            // The feeder times the code and the Dispense press, so those are
            // never waited on.
            other => return Err(format!("the app does not show {other:?}").into()),
        };
        self.0.done(ask).await
    }

    async fn yes(&mut self, prompt: Prompt) -> Result<bool, PlatformError> {
        let ask = match prompt {
            Prompt::RecognizeFeeder { device, address } => {
                InstallAsk::RecognizeFeeder { device, address }
            }
            Prompt::EraseStockPlans => InstallAsk::EraseStockPlans,
            other => return Err(format!("the app does not ask {other:?}").into()),
        };
        match self.0.ask(ask).await? {
            InstallAnswer::Yes { yes } => Ok(yes),
            InstallAnswer::Failed { reason } => Err(reason.into()),
            other => Err(format!("the app answered {other:?}").into()),
        }
    }

    async fn announce(&mut self, announcement: Announcement) -> Result<(), PlatformError> {
        let event = match announcement {
            Announcement::SetupCode { code, seconds } => InstallEvent::Code {
                code: code
                    .presses()
                    .iter()
                    .map(|&button| wellfed_client::setup::panel_position(button))
                    .collect(),
                seconds,
            },
            Announcement::PressDispense { .. } => InstallEvent::PressDispense,
        };
        self.0.update(InstallUpdate::Event { event });
        Ok(())
    }

    async fn settle(&mut self) -> Result<(), PlatformError> {
        Ok(())
    }
}

/// A connected feeder's control characteristic, written by Dart.
struct AppLink {
    asks: Arc<Asks>,
    notifications: mpsc::UnboundedReceiver<Option<Vec<u8>>>,
    deframer: Deframer,
}

impl SetupLink for AppLink {
    async fn send(&mut self, message: &[u8]) -> Result<(), PlatformError> {
        let framed = frame(message)?;
        let pieces = framed.chunks(CONTROL_PIECE).map(<[u8]>::to_vec).collect();
        self.asks.done(InstallAsk::WriteLink { pieces }).await
    }

    async fn receive(&mut self, wait: Duration) -> Result<Vec<u8>, PlatformError> {
        let next = async {
            loop {
                if let Some(message) = self.deframer.next_message()? {
                    return Ok::<_, PlatformError>(message);
                }
                match self.notifications.recv().await.flatten() {
                    Some(bytes) => self.deframer.push(&bytes),
                    None => return Err("the feeder dropped the link".into()),
                }
            }
        };
        tokio::time::timeout(wait, next)
            .await
            .map_err(|_| PlatformError::from("no answer from the feeder in time"))?
    }

    async fn close(self) {
        if let Ok(mut link) = self.asks.link.lock() {
            *link = None;
        }
        // Bounded, and nothing is left to do if it fails.
        let closed = self.asks.done(InstallAsk::CloseLink);
        let _ = tokio::time::timeout(CLOSE_WAIT, closed).await;
    }
}

struct AppBluetooth(Arc<Asks>);

impl Bluetooth for AppBluetooth {
    type Link = AppLink;

    async fn open(&mut self) -> Result<AppLink, PlatformError> {
        // Before Dart connects: the feeder answers as soon as it can.
        let (link, notifications) = mpsc::unbounded_channel();
        *self.0.link.lock().map_err(|_| "the app's state was lost")? = Some(link);
        self.0.done(InstallAsk::OpenLink).await?;
        Ok(AppLink {
            asks: self.0.clone(),
            notifications,
            deframer: Deframer::default(),
        })
    }
}

pub(crate) struct AppDiscovery(pub(crate) Arc<Asks>);

impl Discovery for AppDiscovery {
    async fn locate(&self, identity: [u8; 32]) -> Option<SocketAddr> {
        let identity = wellfed_formats::hex::encode(&identity);
        let asked = self.0.ask(InstallAsk::Locate { identity });
        match tokio::time::timeout(LOCATE_WAIT, asked).await {
            Ok(Ok(InstallAnswer::Located {
                address: Some(address),
            })) => wellfed_connect::parse_address(&address).ok(),
            _ => None,
        }
    }
}

/// The phone's key; a feeder that enrolled it is saved in the app's file
/// before the run goes on, under the name Dart gave.
struct AppKeys {
    keypair: Keypair,
    label: String,
    feeders: Vec<[u8; 32]>,
    /// Where Home last reached a known feeder.
    addresses: HashMap<[u8; 32], SocketAddr>,
    store: PathBuf,
    name: String,
}

impl Keys for AppKeys {
    fn keypair(&self) -> &Keypair {
        &self.keypair
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn feeder(&self, prefix: Option<&str>) -> Option<[u8; 32]> {
        let matches = |identity: &&[u8; 32]| {
            prefix.is_none_or(|prefix| wellfed_formats::hex::encode(*identity).starts_with(prefix))
        };
        let mut known = self.feeders.iter().filter(matches);
        match (known.next(), known.next()) {
            (Some(only), None) => Some(*only),
            _ => None,
        }
    }

    fn last_address(&self, identity: &[u8; 32]) -> Option<SocketAddr> {
        self.addresses.get(identity).copied()
    }

    fn remember(&mut self, identity: &[u8; 32]) -> Result<(), PlatformError> {
        if !self.feeders.contains(identity) {
            self.feeders.push(*identity);
        }

        let store = self.store.to_string_lossy().into_owned();
        let mut saved = load_store(&store)?;
        let hex = wellfed_formats::hex::encode(identity);
        if !saved.feeders.iter().any(|known| known.identity == hex) {
            saved.feeders.push(KnownFeeder {
                identity: hex,
                name: self.name.clone(),
                address: None,
            });
            save_store(&store, &saved)?;
        }
        Ok(())
    }
}

pub(crate) struct AppReport(pub(crate) Arc<Asks>);

impl Report for AppReport {
    fn event(&self, event: Event) {
        let event = InstallEvent::from(event);
        // Progress nobody reads changes nothing about the run.
        let _ = self.0.update(InstallUpdate::Event { event });
    }
}

impl From<Event> for InstallEvent {
    /// Every event, so one added to the run fails this build until the
    /// screens have decided what it means.
    fn from(event: Event) -> Self {
        let note = |text: String| Self::Note { text };
        match event {
            Event::DnsChecked { check, .. } => Self::DnsChecked {
                result: check.into(),
            },
            Event::StockServerStarted => Self::StockServerStarted,
            Event::AwaitingStock => Self::AwaitingStock,
            Event::Waiting(wait) => Self::Waiting {
                why: match wait {
                    Wait::NoFeeder => WaitReason::NoFeeder,
                    Wait::Unreadable { .. } => WaitReason::Unreadable,
                    Wait::Stock(wellfed_client::install::StockNotReady::Offline) => {
                        WaitReason::Offline
                    }
                    Wait::Stock(wellfed_client::install::StockNotReady::NoVersion) => {
                        WaitReason::NoVersion
                    }
                    Wait::Stock(wellfed_client::install::StockNotReady::NotOnAdapter) => {
                        WaitReason::NotOnAdapter
                    }
                },
            },
            Event::VersionLost => Self::VersionLost,
            Event::StockServerEnded { error } => Self::StockServerEnded { error },
            Event::Requesting { build, .. } => Self::Requesting { build },
            Event::StockProgress(percent) => Self::StockProgress { percent },
            Event::StockRestarting => Self::StockRestarting,
            Event::KeyKnown => Self::KeyKnown,
            Event::Rejoining { identity } => Self::Paired {
                identity: identity.to_vec(),
                address: None,
                rebooting: true,
            },
            Event::KeyKnownAt { identity, address } => Self::Paired {
                identity: identity.to_vec(),
                address: Some(address.to_string()),
                rebooting: false,
            },
            Event::Recovered { identity, .. } => Self::Paired {
                identity: identity.to_vec(),
                address: None,
                rebooting: false,
            },
            Event::SetUp { identity, .. } => Self::Paired {
                identity: identity.to_vec(),
                address: None,
                rebooting: true,
            },
            Event::SetupStarting => Self::SetupStarting,
            Event::NoSetupFeeder { error } => Self::NoSetupFeeder { error },
            Event::Joined { address } => Self::Joined {
                address: address.to_string(),
            },
            Event::ReplacementAnswers { build, bank } => Self::ReplacementAnswers { build, bank },
            Event::ReadingBank1 { .. } => Self::ReadingBackup {
                total: STOCK_PAYLOAD_LEN,
            },
            Event::Bank1Read(bytes) => Self::BackupRead {
                bytes: u64::try_from(bytes).unwrap_or(u64::MAX),
                total: STOCK_PAYLOAD_LEN,
            },
            Event::BackupKept { path }
            | Event::BackupFound { path }
            | Event::GivenBackupKept { path } => Self::BackupKept {
                path: path.display().to_string(),
            },
            Event::StockPlans { import, .. } => Self::StockPlans {
                converted: count(import.plans.iter().flatten().count()),
                refused: count(import.refused().count()),
            },
            Event::WithoutStockPlans { .. } => Self::WithoutStockPlans,
            Event::Promoting => Self::Promoting,
            Event::Written { written, length } => Self::Written { written, length },
            Event::SessionDropped { .. } => Self::SessionDropped,
            Event::AlreadyActivated | Event::Activated => Self::Activated,
            Event::PuttingPlans => Self::PuttingPlans,
            Event::EarlierPut { base } => note(format!(
                "an earlier put of the plans went against generation {base}"
            )),
            Event::Installed { build } => Self::Installed { build },
            Event::Plans(result) => Self::Plans {
                result: match result {
                    PlansResult::Set => PlansOutcome::Set,
                    PlansResult::None => PlansOutcome::NoneHeld,
                    PlansResult::NoneConverted => PlansOutcome::NoneConverted,
                    PlansResult::NotRead => PlansOutcome::NotRead,
                    PlansResult::Skipped(_) => PlansOutcome::Skipped,
                },
            },
            Event::FrameSent { .. } => note("the setup frame went out".to_string()),
            Event::DevicePinned { device, .. } => note(format!("device {device} was pinned")),
            Event::KnownKeyRefused { error }
            | Event::TrialOpen { error }
            | Event::EnrollUnanswered { error }
            | Event::PressNotAnnounced { error }
            | Event::SignersUnread { error }
            | Event::FloorUnread { error }
            | Event::PlansUnread { error }
            | Event::ResumeRecordKept { error } => note(error),
            Event::TrialClosed => note("the stock server closed its record".to_string()),
            Event::KeyEnrollment { outcome, .. } => note(match outcome {
                KeyEnrollment::Enrolled(_) => "the update key is enrolled".to_string(),
                KeyEnrollment::Unanswered => {
                    "the update key's enrollment went unanswered".to_string()
                }
            }),
            Event::Restoring => Self::Promoting,
            Event::RestoreSigned { version, .. } => {
                note(format!("stock signed at version {version}"))
            }
            Event::RestoreKeyKept { error } => note(error),
            Event::Floor(floor) => note(format!("the feeder's floor is {floor}")),
            Event::StaleInstallCancelled => {
                note("an earlier install left pending was cancelled".to_string())
            }
            Event::FeederTried { address, answered } => note(format!(
                "tried the feeder at {address}, answered: {answered}"
            )),
            Event::Signed { version, .. } => note(format!("signed at version {version}")),
            Event::PlansKept { .. } => note("stock's plans were kept".to_string()),
            Event::PlansSetAside { .. } => note("the applied plans were set aside".to_string()),
        }
    }
}

/// A count that cannot exceed the plan table's few slots.
fn count(n: usize) -> u8 {
    u8::try_from(n).unwrap_or(u8::MAX)
}

/// How a run's error reads on the screens. `from_stock` says whether this
/// run could have sent stock's install request.
pub fn stop_of(error: &Error, from_stock: bool) -> (StopCase, Resume) {
    use Resume::{FromBank0, FromStart, Nothing};

    match error {
        Error::Steps(steps) => match steps {
            InstallError::UnsupportedVersion(version) => (
                StopCase::OtherFirmware {
                    version: version.clone(),
                },
                Nothing,
            ),
            InstallError::Contested => (StopCase::TwoFeeders, FromStart),
            InstallError::FeederRejected => (StopCase::NotYourFeeder, FromStart),
            InstallError::RequestRefused => (StopCase::EarlierRequestOpen, FromStart),
            InstallError::RequestUncertain => (StopCase::Uncertain, Nothing),
            InstallError::StockRefused { code } => {
                (StopCase::StockRefused { code: *code }, FromStart)
            }
            InstallError::StockClockUnset => (StopCase::ClockUnset, FromStart),
            InstallError::TimedOut(phase) => timed_out(*phase, from_stock),
            InstallError::StockPlans(_) | InstallError::StockPlansUnread(_) => {
                (StopCase::PlansUnreadable, FromBank0)
            }
            InstallError::UpdateKey(_)
            | InstallError::UpdateKeyUncertain
            | InstallError::NotAboveFloor { .. }
            | InstallError::NoVersionAbove { .. }
            | InstallError::PromotionUnknown
            | InstallError::Promotion(_)
            | InstallError::WrongPromotion { .. } => (StopCase::PromotionFailed, FromBank0),
            InstallError::FellBack => (StopCase::FellBack, FromBank0),
            InstallError::PlansAlreadySet { .. }
            | InstallError::PlansNotWritable
            | InstallError::PlansStale { .. }
            | InstallError::PlansRefused(_)
            | InstallError::PlansUnread => (StopCase::PlansNotApplied, Nothing),
            InstallError::PlansUncertain => (StopCase::PlansUnknown, Nothing),
            InstallError::FrameNotTaken => (StopCase::FrameNotTaken, FromStart),
            InstallError::ResetDeclined | InstallError::WrongReplacement { .. } => {
                (StopCase::Other, Nothing)
            }
        },
        Error::Backup(inner) if inner.is_connection_lost() => (StopCase::NotAnswering, FromBank0),
        Error::Handover(_) | Error::FrameSend { .. } | Error::UdpSocket(_) => {
            (StopCase::HandoverFailed, FromStart)
        }
        Error::Backup(_)
        | Error::NotStock(_)
        | Error::Unverified
        | Error::BackupDamaged { .. }
        | Error::BackupExists { .. }
        | Error::KeepGiven(_) => (StopCase::BackupMismatch, FromBank0),
        Error::Setup(inner) => {
            let refusal = match inner.as_ref() {
                Error::SetupRefused(refusal) => Some(*refusal),
                _ => None,
            };
            (StopCase::SetupFailed { refusal }, FromBank0)
        }
        Error::RequestRefused(refusal) => (request_refused(*refusal), FromStart),
        Error::Frame(_) => (StopCase::WifiRefused, FromStart),
        Error::OtherFeeder { .. } => (StopCase::OtherFeeder, FromBank0),
        // The bench route's pairing, which no Bluetooth setup wraps.
        Error::SetupRefused(refusal) => (
            StopCase::SetupFailed {
                refusal: Some(*refusal),
            },
            FromBank0,
        ),
        Error::RecoveryClosed
        | Error::RecoveryTimeout
        | Error::NoProof
        | Error::SetupSession(_)
        | Error::Connect { .. } => (StopCase::SetupFailed { refusal: None }, FromBank0),
        // No session opened with a feeder that paired: it runs the replacement,
        // and a new run reads where it got to.
        Error::Connection(_) | Error::NoSession(_) | Error::Answered { .. } => {
            (StopCase::Other, FromBank0)
        }
        Error::NotAnswering { .. } | Error::NoAddress => (StopCase::NotAnswering, FromBank0),
        Error::NoHomeNetwork
        | Error::KeepPlans(_)
        | Error::PlansNotSent(_)
        | Error::KeepPutBase(_) => (StopCase::Other, FromBank0),
        Error::StockServer(_)
        | Error::Images(_)
        | Error::StockServerGone
        | Error::StockServerFailed(_) => (StopCase::ServerFailed, FromStart),
        Error::Stopped(stopped) => match stopped.unsettled {
            Some(Unsettled::Request(_)) => (StopCase::Uncertain, Nothing),
            Some(Unsettled::UpdateKey) => uncertain_write(UncertainWrite::UpdateKey),
            Some(Unsettled::Promotion) => uncertain_write(UncertainWrite::Promotion),
            Some(Unsettled::Plans) => uncertain_write(UncertainWrite::Plans),
            None if stopped.phase <= Phase::Request && from_stock => {
                (StopCase::Cancelled, FromStart)
            }
            None => (StopCase::Cancelled, FromBank0),
        },
        _ => (StopCase::Other, Nothing),
    }
}

/// The server's refusal, in the screens' words: a contest and an open request
/// are cases of their own, with a way on of their own.
const fn request_refused(refusal: Refusal) -> StopCase {
    let why = match refusal {
        Refusal::Contested => return StopCase::TwoFeeders,
        Refusal::Rejected(Rejection::OtaActive) => return StopCase::EarlierRequestOpen,
        Refusal::Rejected(Rejection::InvalidOta) => RequestRefusal::Stale,
        Refusal::Rejected(Rejection::Offline) => RequestRefusal::Offline,
        Refusal::Rejected(Rejection::ImageServer) => RequestRefusal::ImageServer,
        Refusal::Rejected(_) | Refusal::Unanswered => RequestRefusal::Other,
    };
    StopCase::RequestRefused { why }
}

const fn uncertain_write(write: UncertainWrite) -> (StopCase, Resume) {
    (StopCase::UncertainWrite { write }, Resume::FromBank0)
}

/// A wait that ran out. Before the request nothing was sent. After it, with
/// no replacement answering yet, the request may have done anything, unless the
/// replacement already took its Wi-Fi.
const fn timed_out(phase: Phase, from_stock: bool) -> (StopCase, Resume) {
    match phase {
        Phase::Provision | Phase::AwaitStock | Phase::ConfirmFeeder | Phase::Request => {
            (StopCase::NoFeederCame, Resume::FromStart)
        }
        // It took the Wi-Fi over Bluetooth, so a new run has only that to do
        // again, and asks stock nothing.
        Phase::Join => (StopCase::NotJoined, Resume::FromBank0),
        Phase::AwaitConfirm => (StopCase::NotConfirmed, Resume::FromBank0),
        Phase::AwaitInstall | Phase::Pair | Phase::ReadStatus if from_stock => {
            (StopCase::Uncertain, Resume::Nothing)
        }
        Phase::Pair | Phase::ReadStatus | Phase::AwaitInstall => {
            (StopCase::NoSetupFeeder, Resume::FromBank0)
        }
        _ => (StopCase::Other, Resume::FromBank0),
    }
}

/// The installer's config, under its data directory.
fn installer_config(dir: &Path, phone: IpAddr) -> Result<PathBuf, Error> {
    std::fs::create_dir_all(dir).map_err(|source| Error::Create {
        path: dir.to_path_buf(),
        source,
    })?;
    let path = dir.join(CONFIG);
    let zone = jiff::tz::TimeZone::system();
    let listen = SocketAddr::new(phone, STOCK_BROKER_PORT);
    wellfed_install::stock::installer_config(&path, listen, zone.iana_name())?;
    Ok(path)
}

/// Why a run never started.
enum NotPlanned {
    /// The image is not the one this build may install. The screen refuses
    /// it first; a run does not take the screen's word for it.
    Image(ImageCheck),
    /// What the run was given cannot start it, which a new one can fix.
    Spec {
        case: StopCase,
        detail: String,
    },
    Run(Error),
}

impl From<Error> for NotPlanned {
    fn from(error: Error) -> Self {
        Self::Run(error)
    }
}

/// Everything a run needs, read and checked before the feeder is touched.
fn plan(
    spec: InstallSpec,
    keypair: Keypair,
    pin: Option<&Pin>,
) -> Result<(Plan, AppKeys, bool), NotPlanned> {
    match check_image(Path::new(&spec.image_dir), pin) {
        ImageCheck::Ready { .. } => {}
        refused => return Err(NotPlanned::Image(refused)),
    }
    let review = Review::open(Path::new(&spec.image_dir))?;
    let manifest_path = Path::new(&spec.image_dir).join(UPDATE_MANIFEST);
    let manifest = std::fs::read(&manifest_path).map_err(|source| Error::Read {
        path: manifest_path,
        source,
    })?;
    let version = manifest_version(&manifest).ok_or(Error::NoManifest)?;

    let reset = matches!(spec.start, InstallStart::Reset | InstallStart::FrameTaken);
    let sends_frame = matches!(spec.start, InstallStart::Reset);
    let from_stock = reset || matches!(spec.start, InstallStart::Dns { .. });
    let unjoined = if spec.setup_again {
        Unjoined::SetUpAgain
    } else {
        Unjoined::Wait
    };

    // A bench image joins its build network, and takes no Wi-Fi. A consumer
    // one has only what setup gives it, so nothing that may send it there goes
    // without: stock's request above all, which cannot be taken back.
    let consumer = review.provisioning() == Provisioning::Consumer;
    let wants_home = matches!(spec.start, InstallStart::Reset)
        || (consumer && (from_stock || unjoined == Unjoined::SetUpAgain));
    if wants_home && spec.home.is_none() {
        return Err(NotPlanned::Spec {
            case: StopCase::NoWifiGiven,
            detail: Error::NoHomeNetwork.to_string(),
        });
    }
    // What the frame says, before Bluetooth setup takes the same network.
    let frame_wifi = spec
        .home
        .as_ref()
        .map(|wifi| (wifi.ssid.clone(), wifi.passphrase.clone()));
    let home = match spec.home {
        Some(wifi) if consumer => Some(
            Credentials::new(wifi.ssid.into_bytes(), wifi.passphrase).map_err(|error| {
                NotPlanned::Spec {
                    case: StopCase::WifiRefused,
                    detail: text(&error),
                }
            })?,
        ),
        _ => None,
    };

    let data = Path::new(&spec.data_dir);
    let phone: IpAddr = spec.phone_ip.parse().map_err(|_| NotPlanned::Spec {
        case: StopCase::PhoneOffWifi,
        detail: format!("this phone's address is {:?}", spec.phone_ip),
    })?;
    let config = installer_config(data, phone)?;
    // Only for a new request: a run from bank 0 finds its own record through
    // the feeder this broker trusted.
    if spec.forget_enrollment && from_stock {
        wellfed_install::stock::forget_enrollment(&config)?;
    }

    let start = match &spec.start {
        InstallStart::Dns { name } => {
            let broker = SocketAddr::new(phone, STOCK_BROKER_PORT);
            let dns =
                DnsOverride::new(name, broker).map_err(|error| Error::StockServer(error.into()))?;
            Start::Stock(Provision::Dns(dns))
        }
        InstallStart::Reset => {
            let (ssid, password) = frame_wifi.ok_or(NotPlanned::Spec {
                case: StopCase::NoWifiGiven,
                detail: Error::NoHomeNetwork.to_string(),
            })?;
            Start::Stock(Provision::Frame(SetupFrame {
                ssid,
                password,
                host: phone.to_string(),
                port: STOCK_BROKER_PORT,
                id: FRAME_ACCOUNT.to_string(),
            }))
        }
        InstallStart::FrameTaken => Start::Stock(Provision::Done),
        InstallStart::Bank0 => Start::Bank0,
    };

    let plan = Plan {
        review,
        start,
        // The promotion is signed elsewhere: this phone never holds a key a
        // feeder trusts, and the image builds the signer's in.
        signer: Signer::Manifest { manifest, version },
        home,
        backup: None,
        without_stock_plans: spec.without_stock_plans,
        expect_device: None,
        feeder: None,
        address: spec
            .address
            .as_deref()
            .and_then(|address| wellfed_connect::parse_address(address).ok()),
        unjoined,
        backups: data.join(BACKUPS),
        stock: StockOptions {
            config,
            enroll: ENROLL,
            // A reset feeder has no plans, so no meal comes due in the jump.
            clock: if spec.calibrate_clock || reset {
                ClockPolicy::Always
            } else {
                ClockPolicy::Bounded
            },
            api: Api::Off,
            // Only a phone leaving for the feeder's network loses the address.
            bind: if sends_frame {
                Bind::Any
            } else {
                Bind::Configured
            },
        },
    };
    let known: Vec<([u8; 32], Option<SocketAddr>)> = spec
        .known_feeders
        .iter()
        .filter_map(|feeder| {
            let identity = wellfed_formats::hex::decode::<32>(&feeder.identity)?;
            let address = feeder
                .address
                .as_deref()
                .and_then(|address| wellfed_connect::parse_address(address).ok());
            Some((identity, address))
        })
        .collect();
    let keys = AppKeys {
        keypair,
        label: spec.label,
        feeders: known.iter().map(|(identity, _)| *identity).collect(),
        addresses: known
            .iter()
            .filter_map(|(identity, address)| Some((*identity, (*address)?)))
            .collect(),
        store: PathBuf::from(spec.store_path),
        name: spec.feeder_name,
    };
    Ok((plan, keys, from_stock))
}

/// One run's thread, and the handles that reach it.
pub struct Run {
    asks: Arc<Asks>,
    stop: watch::Sender<bool>,
    pin: Option<Pin>,
}

impl Run {
    /// A run that installs only the image `pin` names.
    pub fn new(pin: Option<Pin>) -> Self {
        Self {
            asks: Arc::new(Asks::new()),
            stop: watch::channel(false).0,
            pin,
        }
    }

    /// Start `spec` on its own thread. `deliver` hears the run to its end
    /// and returns `false` once nobody listens. A run starts once: a second
    /// call, with no spec left, reports that and ends.
    pub fn start(
        &self,
        spec: Option<InstallSpec>,
        keys: Keypair,
        deliver: impl Fn(InstallUpdate) -> bool + Send + 'static,
    ) {
        *self.asks.deliver() = Some(Box::new(deliver));
        let asks = self.asks.clone();
        let stop = self.stop.subscribe();
        let pin = self.pin;

        let thread = std::thread::Builder::new()
            .name("wellfed-install".into())
            .stack_size(STACK)
            .spawn(move || {
                let end = guarded(
                    || match spec {
                        Some(spec) => {
                            run_on_this_thread(spec, keys, pin.as_ref(), asks.clone(), stop)
                        }
                        None => InstallEnd::Stopped {
                            case: StopCase::Other,
                            resume: Resume::Nothing,
                            detail: "this install already ran".to_string(),
                        },
                    },
                    |detail| InstallEnd::Stopped {
                        case: StopCase::Other,
                        resume: Resume::Nothing,
                        detail,
                    },
                );
                asks.update(InstallUpdate::Ended { end });
                // Dropping the sink closes Dart's stream.
                *asks.deliver() = None;
            });
        if let Err(error) = thread {
            self.asks.update(InstallUpdate::Ended {
                end: InstallEnd::Stopped {
                    case: StopCase::Other,
                    resume: Resume::FromStart,
                    detail: format!("no thread for the install: {error}"),
                },
            });
        }
    }

    pub fn answer(&self, id: u32, answer: InstallAnswer) {
        self.asks.answer(id, answer);
    }

    pub fn notified(&self, bytes: Vec<u8>) {
        self.asks.link_event(Some(bytes));
    }

    pub fn link_lost(&self) {
        self.asks.link_event(None);
    }

    pub fn stop(&self) {
        self.stop.send_replace(true);
    }
}

/// `work`, or `on_panic` with what it said if it panicked: a thread that just
/// died would leave its screen waiting for an update that never comes.
pub(crate) fn guarded<T>(work: impl FnOnce() -> T, on_panic: impl FnOnce(String) -> T) -> T {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).unwrap_or_else(|panic| {
        let said = panic
            .downcast_ref::<&str>()
            .map(ToString::to_string)
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "no message".to_string());
        on_panic(format!("the install's thread panicked: {said}"))
    })
}

fn run_on_this_thread(
    spec: InstallSpec,
    keypair: Keypair,
    pin: Option<&Pin>,
    asks: Arc<Asks>,
    stop: watch::Receiver<bool>,
) -> InstallEnd {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            return InstallEnd::Stopped {
                case: StopCase::Other,
                resume: Resume::FromStart,
                detail: format!("no runtime for the install: {error}"),
            };
        }
    };

    let again = if matches!(spec.start, InstallStart::Dns { .. }) {
        Resume::FromStart
    } else {
        Resume::FromBank0
    };
    let (plan, keys, from_stock) = match plan(spec, keypair, pin) {
        Ok(planned) => planned,
        Err(NotPlanned::Spec { case, detail }) => {
            return InstallEnd::Stopped {
                case,
                resume: again,
                detail,
            };
        }
        Err(NotPlanned::Image(refused)) => {
            return InstallEnd::Stopped {
                case: StopCase::Other,
                resume: Resume::Nothing,
                detail: format!("this build will not install its image: {refused:?}"),
            };
        }
        Err(NotPlanned::Run(error)) => {
            let (case, _) = stop_of(&error, false);
            return InstallEnd::Stopped {
                case,
                resume: Resume::Nothing,
                detail: text(&error),
            };
        }
    };
    let (config, backups) = (plan.stock.config.clone(), plan.backups.clone());
    let platform = Platform {
        prompter: AppPrompter(asks.clone()),
        bluetooth: AppBluetooth(asks.clone()),
        discovery: AppDiscovery(asks.clone()),
        keys,
        report: AppReport(asks),
    };

    let stopped = stopped(stop);
    match runtime.block_on(wellfed_install::run(plan, platform, stopped)) {
        Ok(()) => {
            // This feeder is done with stock's broker, and the next install
            // may be another feeder's. Nothing depends on the removal.
            let _ = wellfed_install::stock::forget_enrollment(&config);
            InstallEnd::Done
        }
        Err(error) => {
            let (case, resume) = stop_of(&error, from_stock);
            // Installed and confirmed, with only its meals left behind: there
            // is nothing to pick up, and the owner was told they are not sent
            // again. A failure leaves what still stops a put.
            if matches!(case, StopCase::PlansNotApplied | StopCase::PlansUnknown) {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |now| u64::try_from(now.as_millis()).unwrap_or(u64::MAX));
                let _ = wellfed_install::storage::give_up_plans(&backups, now_ms);
                let _ = wellfed_install::stock::forget_enrollment(&config);
            }
            InstallEnd::Stopped {
                case,
                resume,
                detail: text(&error),
            }
        }
    }
}

/// Resolves when `stop` says so, or when whoever could say so is gone.
pub(crate) async fn stopped(mut stop: watch::Receiver<bool>) -> Stop {
    // A dropped sender is a stop too: the screen that held it went away.
    let _ = stop.wait_for(|stop| *stop).await;
    Stop::Terminate
}

/// One listener's thread.
pub struct Listener {
    stop: watch::Sender<bool>,
}

impl Default for Listener {
    fn default() -> Self {
        Self::new()
    }
}

impl Listener {
    pub fn new() -> Self {
        Self {
            stop: watch::channel(false).0,
        }
    }

    /// Listen for up to `wait` on its own thread. `deliver` hears the ticks
    /// and one last update, and its dropping closes Dart's stream.
    pub fn start(
        &self,
        data_dir: &str,
        wait: Duration,
        deliver: impl Fn(ListenUpdate) -> bool + Send + Sync + 'static,
    ) {
        let config = Path::new(data_dir).join(CONFIG);
        let stop = self.stop.subscribe();
        let deliver = Arc::new(deliver);
        let on_thread = deliver.clone();
        let thread = std::thread::Builder::new()
            .name("wellfed-listen".into())
            .stack_size(STACK)
            .spawn(move || {
                let last = guarded(
                    || listen_on_this_thread(&config, wait, on_thread.as_ref(), stop),
                    |detail| ListenUpdate::Failed { detail },
                );
                on_thread(last);
            });
        if let Err(error) = thread {
            deliver(ListenUpdate::Failed {
                detail: format!("no thread for the listener: {error}"),
            });
        }
    }

    pub fn stop(&self) {
        self.stop.send_replace(true);
    }
}

fn listen_on_this_thread(
    config: &Path,
    wait: Duration,
    deliver: &impl Fn(ListenUpdate) -> bool,
    stop: watch::Receiver<bool>,
) -> ListenUpdate {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            return ListenUpdate::Failed {
                detail: format!("no runtime for the listener: {error}"),
            };
        }
    };

    let tick = |listening: listen::Listening| {
        deliver(ListenUpdate::Tick {
            seconds: u32::try_from(listening.elapsed.as_secs()).unwrap_or(u32::MAX),
            stock: listening.stock,
        });
    };
    match runtime.block_on(listen::listen(config, wait, tick, stopped(stop))) {
        Ok(Listened::Replacement { address }) => ListenUpdate::Replacement {
            address: address.to_string(),
        },
        Ok(Listened::StockStayed { resolved }) => ListenUpdate::StockStayed { resolved },
        Ok(Listened::Silent) => ListenUpdate::Silent,
        Err(Error::Stopped(_)) => ListenUpdate::Stopped,
        Err(error) => ListenUpdate::Failed {
            detail: text(&error),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pin_is_two_digests_and_a_version() {
        let hash = "ab".repeat(32);
        let text: &'static str = Box::leak(format!("{hash}:{hash}:7").into_boxed_str());
        let pin = parse_pin(text).unwrap();
        assert_eq!(pin.version, 7);

        for bad in ["", "ab:cd:7", "x"] {
            assert_eq!(parse_pin(bad), None, "{bad}");
        }
        let short: &'static str = Box::leak(format!("{hash}:{hash}").into_boxed_str());
        assert_eq!(parse_pin(short), None);
        let long: &'static str = Box::leak(format!("{hash}:{hash}:7:8").into_boxed_str());
        assert_eq!(parse_pin(long), None);
    }

    /// The three listed cases, the uncertain request, and what a new run
    /// may do after each.
    #[test]
    fn the_typed_refusals_map_to_the_screens_cases() {
        let steps = |error| stop_of(&Error::Steps(error), true);

        assert_eq!(
            steps(InstallError::UnsupportedVersion("1.1.14".into())),
            (
                StopCase::OtherFirmware {
                    version: "1.1.14".into()
                },
                Resume::Nothing
            )
        );
        assert_eq!(
            steps(InstallError::Contested),
            (StopCase::TwoFeeders, Resume::FromStart)
        );
        assert_eq!(
            stop_of(&Error::Backup(Box::new(Error::Unverified)), true),
            (StopCase::BackupMismatch, Resume::FromBank0)
        );
        assert_eq!(
            steps(InstallError::RequestUncertain),
            (StopCase::Uncertain, Resume::Nothing)
        );
        // Installed, with the meals unknown: not "not set", and no way on that
        // could send them.
        assert_eq!(
            steps(InstallError::PlansUncertain),
            (StopCase::PlansUnknown, Resume::Nothing)
        );
    }

    /// A wait that ran out says nothing was sent only before the request;
    /// after it, a run that sent one is uncertain and never asks again.
    #[test]
    fn a_timeout_is_uncertain_only_once_the_request_went() {
        assert_eq!(
            timed_out(Phase::AwaitStock, true),
            (StopCase::NoFeederCame, Resume::FromStart)
        );
        for phase in [Phase::AwaitInstall, Phase::Pair, Phase::ReadStatus] {
            assert_eq!(
                timed_out(phase, true),
                (StopCase::Uncertain, Resume::Nothing)
            );
            assert_eq!(
                timed_out(phase, false),
                (StopCase::NoSetupFeeder, Resume::FromBank0)
            );
        }

        // A feeder that took the Wi-Fi and never joined it: the owner can give
        // it another, and nothing goes to stock.
        for from_stock in [true, false] {
            assert_eq!(
                timed_out(Phase::Join, from_stock),
                (StopCase::NotJoined, Resume::FromBank0)
            );
        }
    }

    /// A dropped connection during the backup says nothing about the backup,
    /// and a handover that failed asked nothing of the feeder.
    #[test]
    fn a_lost_connection_is_not_a_bad_backup() {
        let lost = Error::Backup(Box::new(Error::Platform("connection reset".into())));
        assert_eq!(
            stop_of(&lost, true),
            (StopCase::NotAnswering, Resume::FromBank0)
        );
        assert_eq!(
            stop_of(&Error::Backup(Box::new(Error::Unverified)), true),
            (StopCase::BackupMismatch, Resume::FromBank0)
        );
        assert_eq!(
            stop_of(&Error::Handover("no network".into()), true),
            (StopCase::HandoverFailed, Resume::FromStart)
        );
        assert_eq!(
            stop_of(&Error::Steps(InstallError::FrameNotTaken), true),
            (StopCase::FrameNotTaken, Resume::FromStart)
        );
        assert_eq!(
            timed_out(Phase::AwaitConfirm, true),
            (StopCase::NotConfirmed, Resume::FromBank0)
        );
    }

    /// The server's reason stays with the refusal: a stale report is not an
    /// earlier request, and a contest during the prompt is two feeders.
    #[test]
    fn a_refused_request_keeps_its_reason() {
        let refused = |refusal| stop_of(&Error::RequestRefused(refusal), true);

        assert_eq!(
            refused(Refusal::Contested),
            (StopCase::TwoFeeders, Resume::FromStart)
        );
        assert_eq!(
            refused(Refusal::Rejected(Rejection::OtaActive)),
            (StopCase::EarlierRequestOpen, Resume::FromStart)
        );
        for (rejection, why) in [
            (Rejection::InvalidOta, RequestRefusal::Stale),
            (Rejection::Offline, RequestRefusal::Offline),
            (Rejection::ImageServer, RequestRefusal::ImageServer),
            (Rejection::Storage, RequestRefusal::Other),
        ] {
            assert_eq!(
                refused(Refusal::Rejected(rejection)),
                (StopCase::RequestRefused { why }, Resume::FromStart),
                "{rejection:?}"
            );
        }
    }

    /// A dead server is not "no feeder came", and the bench route's pairing
    /// failing leaves a way on, as Bluetooth setup's does.
    #[test]
    fn a_failed_server_and_a_failed_recovery_are_not_dead_ends() {
        assert_eq!(
            stop_of(&Error::StockServerFailed("address in use".into()), true),
            (StopCase::ServerFailed, Resume::FromStart)
        );
        assert_eq!(
            stop_of(&Error::RecoveryClosed, true),
            (StopCase::SetupFailed { refusal: None }, Resume::FromBank0)
        );
        assert_eq!(
            stop_of(&Error::NoAddress, true),
            (StopCase::NotAnswering, Resume::FromBank0)
        );
    }

    /// A panic on the run's thread still ends the stream, in words.
    #[test]
    fn a_panic_becomes_an_end() {
        let end = guarded(|| panic!("the steps broke"), |detail| detail);
        assert!(end.contains("the steps broke"), "{end}");
        assert_eq!(guarded(|| 7, |_| 0), 7);
    }

    /// A stop keeps what it had not settled: the request is listened for,
    /// a later write resumes from bank 0, which reads where it got to.
    #[test]
    fn a_stop_names_the_write_it_left_unsettled() {
        use wellfed_install::Stopped;
        let stopped = |phase, unsettled| {
            Error::Stopped(Stopped {
                by: Stop::Terminate,
                phase,
                unsettled,
            })
        };

        assert_eq!(
            stop_of(
                &stopped(
                    Phase::AwaitInstall,
                    Some(Unsettled::Request(uuid::Uuid::from_u128(1)))
                ),
                true
            ),
            (StopCase::Uncertain, Resume::Nothing)
        );
        assert_eq!(
            stop_of(
                &stopped(Phase::AwaitConfirm, Some(Unsettled::Promotion)),
                true
            ),
            (
                StopCase::UncertainWrite {
                    write: UncertainWrite::Promotion
                },
                Resume::FromBank0
            )
        );
        assert_eq!(
            stop_of(&stopped(Phase::AwaitStock, None), true),
            (StopCase::Cancelled, Resume::FromStart)
        );
        assert_eq!(
            stop_of(&stopped(Phase::Backup, None), true),
            (StopCase::Cancelled, Resume::FromBank0)
        );
    }

    fn keys(store: &Path) -> AppKeys {
        AppKeys {
            keypair: Keypair::from_secret([0x61; 32]),
            label: "phone".into(),
            feeders: Vec::new(),
            addresses: HashMap::new(),
            store: store.to_path_buf(),
            name: "Kitchen".into(),
        }
    }

    /// A feeder that enrolled the key is saved before the run goes on, once,
    /// and is then the one known feeder, as the key file has it on a computer.
    #[test]
    fn a_feeder_that_enrolled_the_key_is_saved_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("app.json");
        let mut keys = keys(&store);
        assert_eq!(keys.feeder(None), None);

        keys.remember(&[0xab; 32]).unwrap();
        keys.remember(&[0xab; 32]).unwrap();
        let saved = load_store(store.to_str().unwrap()).unwrap();
        assert_eq!(saved.feeders.len(), 1);
        assert_eq!(saved.feeders[0].identity, "ab".repeat(32));
        assert_eq!(saved.feeders[0].name, "Kitchen");

        assert_eq!(keys.feeder(None), Some([0xab; 32]));
        assert_eq!(keys.feeder(Some("cd")), None);
        keys.remember(&[0xcd; 32]).unwrap();
        // Two known: only a prefix names one.
        assert_eq!(keys.feeder(None), None);
        assert_eq!(keys.feeder(Some("cd")), Some([0xcd; 32]));
    }
}
