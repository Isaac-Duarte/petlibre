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
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot, watch};
use wellfed_client::install::{
    DnsOverride, InstallError, Phase, Provision, STOCK_BROKER_PORT, STOCK_PAYLOAD_LEN,
    manifest_version,
};
use wellfed_client::setup::{Credentials, Deframer, frame};
use wellfed_formats::bench::prompt::Prompt;
use wellfed_formats::canonical::sha256;
use wellfed_formats::manifest::Provisioning;
use wellfed_install::event::{Enrollment as KeyEnrollment, PlansResult, Wait};
use wellfed_install::listen::{self, Listened, UNCERTAIN_WAIT};
use wellfed_install::{
    Announcement, Api, Bluetooth, ClockPolicy, Discovery, Error, Event, Keys, Plan, Platform,
    PlatformError, Prompter, Report, Review, SetupLink, Signer, Start, StockOptions, Stop,
    Unsettled,
};
use wellfed_session::noise::Keypair;

use crate::api::install::{
    DnsResult, ImageCheck, ImageProvisioning, InstallAnswer, InstallAsk, InstallEnd, InstallEvent,
    InstallSpec, InstallStart, InstallUpdate, ListenUpdate, PlansOutcome, Resume, StopCase,
    UncertainWrite, WaitReason,
};
use crate::api::setup::CONTROL_PIECE;
use crate::api::store::{KnownFeeder, load_store, save_store};

/// The run's future is deep; a debug build's frames are large.
const STACK: usize = 16 * 1024 * 1024;
/// How long Dart's NSD lookup may take before the run looks elsewhere.
const LOCATE_WAIT: Duration = Duration::from_secs(15);
/// A link the feeder already dropped may stall hanging up.
const CLOSE_WAIT: Duration = Duration::from_secs(3);
/// How long the broker trusts the first stock feeder that connects.
const ENROLL: Duration = Duration::from_secs(15 * 60);

/// Where the installer keeps its files under its data directory.
const CONFIG: &str = "installer.json";
const BACKUPS: &str = "backups";

/// The image's own files, in the folder the APK's were unpacked into.
const UPDATE_MANIFEST: &str = "update.manifest";
const REVIEW_FILE: &str = "review.json";

/// The image a build installs: its review (which binds the image and its
/// build manifest by their digests), the signed update manifest bank 0
/// promotes with, and that manifest's version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pin {
    pub review_sha256: &'static str,
    pub manifest_sha256: &'static str,
    pub version: u32,
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
    match Review::open(dir) {
        Ok(opened) => ImageCheck::Ready {
            provisioning: match opened.provisioning() {
                Provisioning::Consumer => ImageProvisioning::Consumer,
                Provisioning::Bench => ImageProvisioning::Bench,
            },
            build: opened.build().to_string(),
            version: pin.version,
        },
        Err(error) => mismatch(text(&error)),
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

pub fn pending(data_dir: &Path) -> bool {
    wellfed_install::storage::resumable(&data_dir.join(BACKUPS))
}

pub fn listen_seconds() -> u32 {
    u32::try_from(UNCERTAIN_WAIT.as_secs()).unwrap_or(u32::MAX)
}

pub const fn listen_wait() -> Duration {
    UNCERTAIN_WAIT
}

/// `error` and its causes on one line.
fn text(error: &(dyn std::error::Error + 'static)) -> String {
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
type Deliver = Box<dyn Fn(InstallUpdate) -> bool + Send>;

/// One outstanding set of asks, shared between the run's adapters and the
/// bridge's answers.
struct Asks {
    /// Taken at the end of the run, which closes Dart's stream.
    deliver: Mutex<Option<Deliver>>,
    waiting: Mutex<HashMap<u32, oneshot::Sender<InstallAnswer>>>,
    next: AtomicU32,
    /// The open link's notifications; `None` when none is open.
    link: Mutex<Option<mpsc::UnboundedSender<Option<Vec<u8>>>>>,
}

impl Asks {
    fn new() -> Self {
        Self {
            deliver: Mutex::new(None),
            waiting: Mutex::new(HashMap::new()),
            next: AtomicU32::new(1),
            link: Mutex::new(None),
        }
    }

    /// Hand `update` to Dart; `false` once nobody listens.
    fn update(&self, update: InstallUpdate) -> bool {
        let Ok(deliver) = self.deliver.lock() else {
            return false;
        };
        deliver.as_ref().is_some_and(|deliver| deliver(update))
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

    fn answer(&self, id: u32, answer: InstallAnswer) {
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
struct AppPrompter(Arc<Asks>);

impl Prompter for AppPrompter {
    async fn done(&mut self, prompt: Prompt) -> Result<(), PlatformError> {
        let ask = match prompt {
            Prompt::SetOverride { name, broker } => InstallAsk::SetOverride {
                name,
                broker: broker.to_string(),
            },
            Prompt::RestartFeeder => InstallAsk::RestartFeeder,
            Prompt::OpenRecoveryWindow => InstallAsk::OpenRecoveryWindow,
            // The app takes the DNS route alone, and the feeder times the
            // code and the Dispense press, so these are never waited on.
            other => return Err(format!("the app does not show {other:?}").into()),
        };
        self.0.done(ask).await
    }

    async fn yes(&mut self, prompt: Prompt) -> Result<bool, PlatformError> {
        let Prompt::RecognizeFeeder { device, address } = prompt else {
            return Err(format!("the app does not ask {prompt:?}").into());
        };
        match self
            .0
            .ask(InstallAsk::RecognizeFeeder { device, address })
            .await?
        {
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

struct AppDiscovery(Arc<Asks>);

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

struct AppReport(Arc<Asks>);

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
            Event::Floor(floor) => note(format!("the feeder's floor is {floor}")),
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
            | InstallError::PlansUncertain
            | InstallError::PlansUnread => (StopCase::PlansNotApplied, Nothing),
            InstallError::ResetDeclined | InstallError::WrongReplacement { .. } => {
                (StopCase::Other, Nothing)
            }
        },
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
        Error::NotAnswering { .. } => (StopCase::NotAnswering, FromBank0),
        Error::NoHomeNetwork | Error::KeepPlans(_) | Error::PlansNotSent(_) => {
            (StopCase::Other, FromBank0)
        }
        Error::StockServer(_) | Error::Images(_) | Error::StockServerGone => {
            (StopCase::ServerFailed, FromStart)
        }
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

const fn uncertain_write(write: UncertainWrite) -> (StopCase, Resume) {
    (StopCase::UncertainWrite { write }, Resume::FromBank0)
}

/// A wait that ran out. Before the request nothing was sent. After it, with
/// no replacement answering yet, the request may have done anything.
const fn timed_out(phase: Phase, from_stock: bool) -> (StopCase, Resume) {
    match phase {
        Phase::Provision | Phase::AwaitStock | Phase::ConfirmFeeder | Phase::Request => {
            (StopCase::NoFeederCame, Resume::FromStart)
        }
        Phase::AwaitInstall | Phase::Pair | Phase::Join | Phase::ReadStatus if from_stock => {
            (StopCase::Uncertain, Resume::Nothing)
        }
        Phase::Pair | Phase::Join | Phase::ReadStatus | Phase::AwaitInstall => {
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

/// Everything a run needs, read and checked before the feeder is touched.
fn plan(spec: InstallSpec, keypair: Keypair) -> Result<(Plan, AppKeys, bool), Error> {
    let review = Review::open(Path::new(&spec.image_dir))?;
    let manifest_path = Path::new(&spec.image_dir).join(UPDATE_MANIFEST);
    let manifest = std::fs::read(&manifest_path).map_err(|source| Error::Read {
        path: manifest_path,
        source,
    })?;
    let version = manifest_version(&manifest).ok_or(Error::NoManifest)?;

    let data = Path::new(&spec.data_dir);
    let phone: IpAddr = spec.phone_ip.parse().map_err(|_| Error::NoAddress)?;
    let config = installer_config(data, phone)?;
    if spec.forget_enrollment {
        wellfed_install::stock::forget_enrollment(&config)?;
    }

    let from_stock = matches!(spec.start, InstallStart::Dns { .. });
    let start = match &spec.start {
        InstallStart::Dns { name } => {
            let broker = SocketAddr::new(phone, STOCK_BROKER_PORT);
            let dns =
                DnsOverride::new(name, broker).map_err(|error| Error::StockServer(error.into()))?;
            Start::Stock(Provision::Dns(dns))
        }
        InstallStart::Bank0 => Start::Bank0,
    };

    // A bench image joins its build network, and takes no Wi-Fi.
    let home = match (review.provisioning(), spec.home) {
        (Provisioning::Consumer, Some(wifi)) => Some(
            Credentials::new(wifi.ssid.into_bytes(), wifi.passphrase)
                .map_err(Error::SetupSession)?,
        ),
        _ => None,
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
        address: None,
        backups: data.join(BACKUPS),
        stock: StockOptions {
            config,
            enroll: ENROLL,
            clock: if spec.calibrate_clock {
                ClockPolicy::Always
            } else {
                ClockPolicy::Bounded
            },
            api: Api::Off,
        },
    };
    let keys = AppKeys {
        keypair,
        label: spec.label,
        feeders: spec
            .known_feeders
            .iter()
            .filter_map(|hex| wellfed_formats::hex::decode::<32>(hex))
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
}

impl Default for Run {
    fn default() -> Self {
        Self::new()
    }
}

impl Run {
    pub fn new() -> Self {
        Self {
            asks: Arc::new(Asks::new()),
            stop: watch::channel(false).0,
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
        if let Ok(mut slot) = self.asks.deliver.lock() {
            *slot = Some(Box::new(deliver));
        }
        let asks = self.asks.clone();
        let stop = self.stop.subscribe();

        let thread = std::thread::Builder::new()
            .name("wellfed-install".into())
            .stack_size(STACK)
            .spawn(move || {
                let end = match spec {
                    Some(spec) => run_on_this_thread(spec, keys, asks.clone(), stop),
                    None => InstallEnd::Stopped {
                        case: StopCase::Other,
                        resume: Resume::Nothing,
                        detail: "this install already ran".to_string(),
                    },
                };
                asks.update(InstallUpdate::Ended { end });
                // Dropping the sink closes Dart's stream.
                if let Ok(mut slot) = asks.deliver.lock() {
                    *slot = None;
                }
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

fn run_on_this_thread(
    spec: InstallSpec,
    keypair: Keypair,
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

    let (plan, keys, from_stock) = match plan(spec, keypair) {
        Ok(planned) => planned,
        Err(error) => {
            let (case, _) = stop_of(&error, false);
            return InstallEnd::Stopped {
                case,
                resume: Resume::Nothing,
                detail: text(&error),
            };
        }
    };
    let config = plan.stock.config.clone();
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
            InstallEnd::Stopped {
                case,
                resume,
                detail: text(&error),
            }
        }
    }
}

/// Resolves when `stop` says so, or when whoever could say so is gone.
async fn stopped(mut stop: watch::Receiver<bool>) -> Stop {
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
                let last = listen_on_this_thread(&config, wait, on_thread.as_ref(), stop);
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
    }

    /// A wait that ran out says nothing was sent only before the request;
    /// after it, a run that sent one is uncertain and never asks again.
    #[test]
    fn a_timeout_is_uncertain_only_once_the_request_went() {
        assert_eq!(
            timed_out(Phase::AwaitStock, true),
            (StopCase::NoFeederCame, Resume::FromStart)
        );
        for phase in [
            Phase::AwaitInstall,
            Phase::Pair,
            Phase::Join,
            Phase::ReadStatus,
        ] {
            assert_eq!(
                timed_out(phase, true),
                (StopCase::Uncertain, Resume::Nothing)
            );
            assert_eq!(
                timed_out(phase, false),
                (StopCase::NoSetupFeeder, Resume::FromBank0)
            );
        }
    }

    /// A stop keeps what it had not settled: the request is listened for,
    /// a later write resumes from bank 0 and is not sent again.
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
