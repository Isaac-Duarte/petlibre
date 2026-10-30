//! The consumer installer's steps, sans-IO: from a feeder on stock 1.1.15
//! to the replacement confirmed on bank 1 ([consumer installation]). The
//! Android app, `wellfed-server` and the `wellfed` CLI run these same
//! steps; each supplies the IO (joining Wi-Fi, the UDP frame, the broker and
//! image server stock talks to, the device API session).
//!
//! The run, in order ([`Step`] says which is next):
//! 1. **Provision:** stock is pointed at the installer's broker, where it
//!    connects with its own MQTT credentials, which the broker trusts on
//!    first use; nothing captured, nothing redirected. Two routes
//!    ([`Provision`]):
//!    - **The Reset hold**, which needs nothing from the owner's router: the
//!      hold runs stock's factory restore; then one type-3 setup frame
//!      ([`frame`]) to stock's setup access point names the owner's Wi-Fi and
//!      the installer's broker.
//!    - **DNS** ([`dns`]): the owner's resolver answers stock's broker name with the installer's
//!      address, and a restarted stock connects there. Nothing is written to
//!      the feeder.
//! 2. **Stock ready:** stock reports 1.1.15 on its adapter, the same
//!    conditions its own precheck and the server's trial gate hold.
//! 3. **Confirm the feeder:** trust on first use admits whichever stock-
//!    shaped client reached the broker first, so the owner checks the
//!    enrolled device ID and address before anything is sent to it. A second
//!    identity seen inside the enrollment window ends the run.
//! 4. **Request:** one `OTA_UPGRADE`, never resent. A refused precheck can
//!    erase stock's OTA status sector and an uncertain request may still be
//!    downloading, so whatever happens to it ends this run; a new run
//!    starts only after the server has resolved it.
//! 5. **Install:** stock downloads the image, checks its byte-sum, writes
//!    bank 0 and restarts into the replacement, which then answers the
//!    device API from bank 0.
//! 6. **Pair:** the installer becomes the replacement's owner through
//!    pairing, the setup code keyed in on the panel. The route follows the
//!    image's provisioning ([`PairRoute`]): a consumer image has no network
//!    until Bluetooth setup gives it the owner's Wi-Fi, so the installer
//!    runs that setup, then waits for the reboot and finds the feeder on the
//!    LAN; a bench image joins its build network, and the installer uses the
//!    LAN recovery window. A key the build already knows counts on either.
//! 7. **Bank 0:** the device API names the served build on bank 0, and the
//!    stock server is told so it closes its record. A run resumed after an
//!    earlier one activated bank 1 finds the build there and only waits for
//!    its confirmation.
//! 8. **Update key:** an image that does not build in the installer's
//!    update key enrolls it ([`crate::update_key::Enroll`]), confirmed by a
//!    Dispense press. A refusal ends the run before anything is promoted.
//!    The enrollment goes once: one left unanswered is judged by the
//!    feeder's signer list, never by sending it again.
//! 9. **Backup:** promotion overwrites bank 1, the feeder's only copy of
//!    stock, so it waits for a [`VerifiedBackup`]. There is no way past this
//!    step without one. The bank-0 role streams bank 1 ([`Bank1Reader`]),
//!    and [`StockBackup::from_bank`] checks it against the pinned payload
//!    and rebuilds the owner's own stock OTA, which the caller keeps.
//! 10. **Version:** the feeder's floor (`UpdateState`) decides it. A
//!     manifest signed before the run must be above it; otherwise the
//!     installer signs the build itself, above both the floor and any
//!     version it signed before ([`Promotion`]).
//! 11. **Promote:** the signed update ([`crate::update::Installer`]) writes
//!     bank 1 from bank 0, and its trial boot confirms on bank 1.
//!
//! No route carries meals over: the replacement starts with none, and the
//! owner sets them up again in Wellfed.
//!
//! A step that fails ends the run ([`Step::Failed`]); nothing is retried
//! here, and the caller bounds every wait ([`Install::gave_up`]).
//!
//! [consumer installation]: ../../../../docs/firmware/consumer-installation.md

pub mod backup;
pub mod dns;
pub mod frame;

pub use backup::{
    BackupError, Bank1Reader, CONTAINER_HEADER, DOWNGRADE_LEN, ReadError, RestoreError,
    RestoreOffer, STOCK_OTA_SHA256, STOCK_PAYLOAD_LEN, STOCK_PAYLOAD_SHA256, STOCK_VERSION,
    StockBackup, StockPins, VerifiedBackup,
};
pub use dns::{DnsCheck, DnsError, DnsOverride, STOCK_BROKER_PORT};
pub use frame::{FrameError, SETUP_ADDRESS, SetupFrame};

use wellfed_model::{Client, ClientRole, Readiness};

use crate::update::InstallError as UpdateError;
use crate::update_key::{EnrollError, Enrolled};

/// How the installer becomes the replacement's owner, decided by the
/// image's provisioning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairRoute {
    /// A consumer image: with no Wi-Fi stored it starts Bluetooth setup at
    /// every boot, and that setup enrolls the installer as its owner and
    /// gives it the owner's Wi-Fi; it then reboots and joins.
    Setup,
    /// A bench image joins its build network and knows no client key: the
    /// owner opens the LAN recovery window on the panel.
    Recovery,
}

/// Whether the promotion's signer must be enrolled on the replacement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateKey {
    /// The image builds the signer's key in, or the run promotes a
    /// manifest signed elsewhere: nothing is enrolled.
    BuiltIn,
    /// The installer's own key (its public half), which the image does not
    /// build in: enrolled after pairing, with a Dispense press.
    Enroll([u8; 32]),
}

/// The signed update the promotion offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Promotion {
    /// A manifest signed before the run, at this version
    /// ([`manifest_version`]).
    Signed(u32),
    /// The installer signs the build itself once it has read the feeder's
    /// floor, one above the floor and above `above`: the highest version
    /// it signed before, so no two manifests it made share a version.
    Sign { above: u32 },
}

/// What a run installs with, beside the build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub route: PairRoute,
    pub update_key: UpdateKey,
    pub promotion: Promotion,
}

/// The version a signed update manifest promotes to, which the run waits
/// to see confirmed; `None` for bytes that are not a manifest.
pub fn manifest_version(manifest: &[u8]) -> Option<u32> {
    wellfed_core::ota::offer::parse_manifest(manifest)
        .ok()
        .map(|signed| signed.manifest.version)
}

/// The build a signed update manifest names, which bank 1 must then run;
/// `None` for bytes that are not a manifest.
pub fn manifest_build(manifest: &[u8]) -> Option<[u8; 32]> {
    wellfed_core::ota::offer::parse_manifest(manifest)
        .ok()
        .map(|signed| signed.manifest.build_id)
}

/// Stock 1.1.15's error for an `OTA_UPGRADE` it refused at once, with no
/// download, while its clock stood days behind and unset by the server
/// (physical, one run; the link to its time-valid precheck is likely, not
/// confirmed statically).
pub const STOCK_CLOCK_UNSET: u32 = 1214;

/// How stock is pointed at the installer's broker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Provision {
    /// The owner's resolver answers stock's broker name with the installer's
    /// broker, and stock, restarted, connects there. Nothing is written to
    /// the feeder.
    Dns(DnsOverride),
    /// The owner holds Reset, which runs stock's factory restore and opens its setup access point; then this
    /// frame goes there.
    Frame(SetupFrame),
    /// Stock already connects to the installer's broker (provisioned by an
    /// earlier frame, or on the bench).
    Done,
}

/// An encoded setup frame. It carries the Wi-Fi passphrase in plaintext,
/// so its `Debug` shows only its length.
#[derive(Clone, PartialEq, Eq)]
pub struct FrameBytes(Vec<u8>);

impl FrameBytes {
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

impl core::fmt::Debug for FrameBytes {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "FrameBytes({} bytes)", self.0.len())
    }
}

/// What stock reports for its power source (`powerType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StockPower {
    /// `powerType` 1: the mains adapter, which stock's precheck requires.
    Adapter,
    Other(u64),
}

impl StockPower {
    pub fn from_type(power_type: u64) -> Self {
        match power_type {
            1 => Self::Adapter,
            other => Self::Other(other),
        }
    }
}

/// How the installer's broker knows the feeder it serves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeederTrust {
    /// Its credentials came from a capture (the bench).
    Captured,
    /// Trusted on first use: its own client ID (its device ID) and the
    /// address it connected from, for the owner to check.
    Enrolled { device: String, address: String },
    /// Another stock-shaped identity tried the broker inside the enrollment
    /// window: which one is the owner's feeder cannot be told.
    Contested,
}

/// The installer's view of stock, from its broker's session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StockView {
    /// Stock's telemetry is current.
    pub online: bool,
    /// The version it last reported, if it reported one recently.
    pub version: Option<String>,
    pub power: Option<StockPower>,
    pub trust: FeederTrust,
}

/// Why stock may not be asked yet; the install waits.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StockNotReady {
    #[error("stock has not connected to the installer's broker")]
    Offline,
    #[error("stock has not reported its version yet")]
    NoVersion,
    #[error("stock is not on its mains adapter")]
    NotOnAdapter,
}

/// What became of the one install request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requested {
    /// The server persisted it and handed it to stock's session.
    Sent,
    /// The server refused it, and its ledger shows no record of it.
    Refused,
    /// It may or may not have reached stock.
    Uncertain,
}

/// The replacement as its device API reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Replacement<'a> {
    /// The build identity, 64 hex.
    pub build: &'a str,
    pub bank: u8,
}

/// Which step a wait was given up on, or the run is in. The variants come
/// in the order a run reaches them, so a later phase compares greater.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    Provision,
    AwaitStock,
    ConfirmFeeder,
    Request,
    AwaitInstall,
    Pair,
    /// After Bluetooth setup: the feeder's reboot onto the owner's Wi-Fi.
    Join,
    ReadStatus,
    CloseTrial,
    UpdateKey,
    Backup,
    /// The floor read and the version chosen or checked, and the signing.
    Sign,
    Promote,
    AwaitConfirm,
    Done,
}

/// Why an install ended early. Each names what the owner faces: where the
/// feeder is left, and whether a new run may start.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    #[error("stock reports version {0}; only {STOCK_VERSION} is installed from")]
    UnsupportedVersion(String),
    #[error(
        "a second device tried the installer's broker during enrollment; nothing was sent: remove the enrollment and run again on a network only the feeder joins"
    )]
    Contested,
    #[error("the owner did not recognize the enrolled feeder; nothing was sent to it")]
    FeederRejected,
    /// Stock saves a frame only once it reaches the broker the frame named,
    /// and otherwise opens its setup network again, so nothing is lost.
    #[error(
        "stock never reached the broker after the setup frame: it keeps nothing, and reopens its setup network for another try. Check the Wi-Fi's password and that it has a 2.4 GHz band"
    )]
    FrameNotTaken,
    #[error(
        "the server refused the install request without sending it; stock is unchanged, and a new run may ask again once the reason is fixed"
    )]
    RequestRefused,
    #[error(
        "the install request may have reached stock; never resent: wait for the server to resolve it before a new run"
    )]
    RequestUncertain,
    #[error("stock refused or failed the install (error {code:?}); it keeps running stock")]
    StockRefused { code: Option<u32> },
    /// Stock's [`STOCK_CLOCK_UNSET`]: it refused before downloading, most
    /// likely because its clock was never set.
    #[error(
        "stock refused the install before downloading (error {STOCK_CLOCK_UNSET}), most likely because its clock is not set: the stock server sets it only when it is within 10 minutes or the owner asks; it keeps running stock"
    )]
    StockClockUnset,
    #[error("build {reported} answers on bank {bank}, not the served build")]
    WrongReplacement { reported: String, bank: u8 },
    #[error(
        "the installer's update key was not enrolled ({0}); nothing was promoted: the feeder runs the replacement from bank 0 and still holds stock in bank 1"
    )]
    UpdateKey(EnrollError),
    /// The enrollment went unanswered and the signer list read after it
    /// does not show the key (or could not be read). It is not sent again.
    #[error(
        "the update key's enrollment went unanswered and the feeder does not list it as a signer; it is not sent again, and nothing was promoted"
    )]
    UpdateKeyUncertain,
    #[error(
        "the manifest's version {version} is not above the feeder's floor {floor}; it would need a downgrade statement, so nothing was promoted"
    )]
    NotAboveFloor { version: u32, floor: u32 },
    #[error("no version is left above the feeder's floor {floor}")]
    NoVersionAbove { floor: u32 },
    /// A run resumed after the activation, signing for itself, with no
    /// manifest an earlier run kept: the version to wait for is unknown.
    #[error(
        "bank 1 runs the build, but the version promoted is unknown: pass the manifest the earlier run promoted"
    )]
    PromotionUnknown,
    #[error(
        "the promotion stopped: {0}; the feeder keeps running bank 0, but bank 1 may be partly overwritten: the verified backup is now the copy of stock"
    )]
    Promotion(UpdateError),
    #[error("bank 1's trial boot fell back to bank 0")]
    FellBack,
    #[error("build {reported} answers on bank 1 after the promotion, not the promoted build")]
    WrongPromotion { reported: String },
    #[error("gave up waiting at {0:?}")]
    TimedOut(Phase),
}

/// What the caller does next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Ask the owner to have their resolver answer the override's name with
    /// its broker; look the name up and show [`DnsOverride::check`], as
    /// advice only; then start the broker, so its enrollment window opens
    /// just before the owner restarts the feeder and stock looks the name up
    /// again; then [`Install::dns_pointed`].
    PointDns(DnsOverride),
    /// With this host joined to stock's setup access point, send these bytes
    /// once to [`SETUP_ADDRESS`] over UDP, then call
    /// [`Install::frame_sent`]. Never resent: stock applies a frame once.
    SendFrame(FrameBytes),
    /// Serve the broker and report stock with [`Install::stock`].
    AwaitStock,
    /// Show the owner how the broker knows the feeder; then
    /// [`Install::feeder_confirmed`] or [`Install::feeder_rejected`].
    ConfirmFeeder(FeederTrust),
    /// Serve the image and ask stock once to install it; report with
    /// [`Install::requested`]. Comes once per run.
    RequestInstall,
    /// Stock is installing: report progress, a refusal, or its session
    /// ending as it restarts into the replacement.
    AwaitInstall,
    /// Admit this installer as the replacement's owner by this route. A key
    /// the build already knows opens a session at once: then
    /// [`Install::paired`]. Otherwise, by [`PairRoute::Recovery`], the
    /// recovery window's exchange, then [`Install::paired`]; by
    /// [`PairRoute::Setup`], the Bluetooth setup exchange with the owner's
    /// Wi-Fi, then [`Install::set_up`].
    Pair(PairRoute),
    /// Bluetooth setup is done and the feeder reboots onto the owner's
    /// Wi-Fi: find it on the LAN and open a session; then
    /// [`Install::joined`].
    AwaitJoin,
    /// Read the replacement's status; report with [`Install::replacement`].
    ReadStatus,
    /// Tell the stock server the served build answered from bank 0 so it
    /// closes its record; then [`Install::trial_closed`], whatever it said.
    CloseTrial,
    /// Enroll this public update key ([`crate::update_key::Enroll`]), the
    /// owner pressing Dispense when asked; report its end with
    /// [`Install::key_enrolled`]. Comes once per run.
    EnrollKey([u8; 32]),
    /// The enrollment went unanswered: read the feeder's client list and
    /// report it with [`Install::signers`] (or [`Install::signers_unread`]).
    /// The enrollment is never sent again.
    CheckSigners,
    /// The enrollment is out; waiting for [`Install::key_enrolled`].
    AwaitEnroll,
    /// Promotion is refused until [`Install::backup`] is given a verified
    /// stock backup: read bank 1 with [`Bank1Reader`] and check it with
    /// [`StockBackup::from_bank`] (or a copy kept earlier, with
    /// [`StockBackup::open`]), and keep it.
    Backup,
    /// Read the feeder's floor (`UpdateState`'s `min_version`); report it
    /// with [`Install::floor`].
    ReadFloor,
    /// Sign the reviewed build at this version with the installer's update
    /// key, keep the manifest, then [`Install::signed`].
    Sign {
        version: u32,
    },
    /// Push the signed update until bank 1 is activated; report with
    /// [`Install::promoted`].
    Promote,
    /// Read the status and the update state until bank 1 confirms; report
    /// with [`Install::confirmed`]. Only from a session opened after the
    /// activation's restart: bank 0 answering before it would read as a
    /// fallback.
    AwaitConfirm,
    /// The replacement is confirmed on bank 1.
    Done,
    Failed(InstallError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Stage {
    PointDns(DnsOverride),
    Provision(FrameBytes),
    /// The frame is out; waiting for the caller to say so.
    FrameOut,
    AwaitStock,
    ConfirmFeeder(FeederTrust),
    Request,
    Requesting,
    AwaitInstall,
    Pair,
    AwaitJoin,
    ReadStatus,
    CloseTrial,
    EnrollKey([u8; 32]),
    /// The enrollment is out; waiting for its end.
    Enrolling,
    CheckSigners,
    Backup,
    Floor,
    Sign {
        version: u32,
    },
    Promote,
    AwaitConfirm,
    Done,
    Failed(InstallError),
}

/// One install run.
#[derive(Debug)]
pub struct Install {
    stage: Stage,
    /// The served build's identity, lowercase hex.
    build: String,
    options: Options,
    /// The promoted manifest's version, which bank 1 must confirm: known
    /// from the start for a manifest signed before the run, and once the
    /// floor is read when the run signs.
    version: Option<u32>,
    backup: Option<VerifiedBackup>,
    /// Stock's latest download progress, percent.
    progress: Option<u8>,
    /// This run's request reached stock's session, so the stock server holds
    /// a record for it to close.
    sent: bool,
}

impl Install {
    /// A run that installs `build` (the served image's identity, 64 hex),
    /// pairs by `options`' route and promotes as they say. A frame whose
    /// values stock would misread is refused here, before anything is
    /// sent.
    pub fn new(provision: Provision, build: &str, options: Options) -> Result<Self, FrameError> {
        let stage = match provision {
            Provision::Dns(dns) => Stage::PointDns(dns),
            Provision::Frame(frame) => Stage::Provision(FrameBytes(frame.encode()?)),
            Provision::Done => Stage::AwaitStock,
        };
        Ok(Self::at(stage, build, options))
    }

    /// A run that resumes with the replacement already installed by stock
    /// in an earlier run, which stopped before promotion or before its
    /// confirmation: it starts at pairing, and stock is not asked anything.
    pub fn from_bank0(build: &str, options: Options) -> Self {
        Self::at(Stage::Pair, build, options)
    }

    fn at(stage: Stage, build: &str, options: Options) -> Self {
        let version = match options.promotion {
            Promotion::Signed(version) => Some(version),
            Promotion::Sign { .. } => None,
        };
        Self {
            stage,
            build: build.to_ascii_lowercase(),
            options,
            version,
            backup: None,
            progress: None,
            sent: false,
        }
    }

    /// The next thing to do.
    pub fn step(&mut self) -> Step {
        match &self.stage {
            Stage::PointDns(dns) => Step::PointDns(dns.clone()),
            Stage::Provision(frame) => {
                let frame = frame.clone();
                self.stage = Stage::FrameOut;
                Step::SendFrame(frame)
            }
            Stage::FrameOut | Stage::AwaitStock => Step::AwaitStock,
            Stage::ConfirmFeeder(trust) => Step::ConfirmFeeder(trust.clone()),
            Stage::Request => {
                self.stage = Stage::Requesting;
                Step::RequestInstall
            }
            // Asked once: until its outcome comes, there is only waiting.
            Stage::Requesting | Stage::AwaitInstall => Step::AwaitInstall,
            Stage::Pair => Step::Pair(self.options.route),
            Stage::AwaitJoin => Step::AwaitJoin,
            Stage::ReadStatus => Step::ReadStatus,
            Stage::CloseTrial => Step::CloseTrial,
            &Stage::EnrollKey(key) => {
                self.stage = Stage::Enrolling;
                Step::EnrollKey(key)
            }
            // Sent once: until its end comes, there is only waiting.
            Stage::Enrolling => Step::AwaitEnroll,
            Stage::CheckSigners => Step::CheckSigners,
            Stage::Backup if self.backup.is_some() => {
                self.stage = Stage::Floor;
                Step::ReadFloor
            }
            Stage::Backup => Step::Backup,
            Stage::Floor => Step::ReadFloor,
            &Stage::Sign { version } => Step::Sign { version },
            Stage::Promote => Step::Promote,
            Stage::AwaitConfirm => Step::AwaitConfirm,
            Stage::Done => Step::Done,
            Stage::Failed(error) => Step::Failed(error.clone()),
        }
    }

    /// Where the run stands, for a progress display.
    pub fn phase(&self) -> Phase {
        match self.stage {
            Stage::PointDns(_) | Stage::Provision(_) | Stage::FrameOut => Phase::Provision,
            Stage::AwaitStock => Phase::AwaitStock,
            Stage::ConfirmFeeder(_) => Phase::ConfirmFeeder,
            Stage::Request | Stage::Requesting => Phase::Request,
            Stage::AwaitInstall => Phase::AwaitInstall,
            Stage::Pair => Phase::Pair,
            Stage::AwaitJoin => Phase::Join,
            Stage::ReadStatus => Phase::ReadStatus,
            Stage::CloseTrial => Phase::CloseTrial,
            Stage::EnrollKey(_) | Stage::Enrolling | Stage::CheckSigners => Phase::UpdateKey,
            Stage::Backup => Phase::Backup,
            Stage::Floor | Stage::Sign { .. } => Phase::Sign,
            Stage::Promote => Phase::Promote,
            Stage::AwaitConfirm => Phase::AwaitConfirm,
            Stage::Done => Phase::Done,
            // Failed is reported through `step`; its phase was the last.
            Stage::Failed(_) => Phase::Done,
        }
    }

    /// Stock's latest download progress, percent.
    pub fn progress(&self) -> Option<u8> {
        self.progress
    }

    /// The build this run installs.
    pub fn build(&self) -> &str {
        &self.build
    }

    /// The version the promotion offers, once known.
    pub fn version(&self) -> Option<u32> {
        self.version
    }

    /// The backup promotion waits for, once given.
    pub fn verified_backup(&self) -> Option<VerifiedBackup> {
        self.backup
    }

    /// The owner says their resolver answers stock's broker name with the
    /// installer's broker and the feeder restarted: wait for stock there.
    pub fn dns_pointed(&mut self) {
        if matches!(self.stage, Stage::PointDns(_)) {
            self.stage = Stage::AwaitStock;
        }
    }

    /// The frame went out (the caller left stock's access point after).
    pub fn frame_sent(&mut self) {
        if self.stage == Stage::FrameOut {
            self.stage = Stage::AwaitStock;
        }
    }

    /// The frame went out and no stock arrived in the time joining takes.
    pub fn frame_not_taken(&mut self) {
        if self.stage == Stage::AwaitStock {
            self.stage = Stage::Failed(InstallError::FrameNotTaken);
        }
    }

    /// Stock as the broker sees it. Ready moves the run to the owner's check
    /// of the feeder; a version other than [`STOCK_VERSION`], or a contested
    /// enrollment, ends it.
    pub fn stock(&mut self, view: &StockView) -> Result<(), StockNotReady> {
        if !matches!(
            self.stage,
            Stage::AwaitStock | Stage::FrameOut | Stage::ConfirmFeeder(_)
        ) {
            return Ok(());
        }
        if view.trust == FeederTrust::Contested {
            self.stage = Stage::Failed(InstallError::Contested);
            return Ok(());
        }
        if matches!(self.stage, Stage::ConfirmFeeder(_)) {
            return Ok(());
        }
        if !view.online {
            return Err(StockNotReady::Offline);
        }
        let Some(version) = &view.version else {
            return Err(StockNotReady::NoVersion);
        };
        if version != STOCK_VERSION {
            self.stage = Stage::Failed(InstallError::UnsupportedVersion(version.clone()));
            return Ok(());
        }
        if view.power != Some(StockPower::Adapter) {
            return Err(StockNotReady::NotOnAdapter);
        }
        self.stage = Stage::ConfirmFeeder(view.trust.clone());
        Ok(())
    }

    /// The owner recognized the feeder [`Step::ConfirmFeeder`] showed.
    pub fn feeder_confirmed(&mut self) {
        if matches!(self.stage, Stage::ConfirmFeeder(_)) {
            self.stage = Stage::Request;
        }
    }

    /// The owner did not recognize it: nothing is sent.
    pub fn feeder_rejected(&mut self) {
        if matches!(self.stage, Stage::ConfirmFeeder(_)) {
            self.stage = Stage::Failed(InstallError::FeederRejected);
        }
    }

    /// What became of the request [`Step::RequestInstall`] asked for.
    pub fn requested(&mut self, outcome: Requested) {
        if self.stage != Stage::Requesting {
            return;
        }
        self.sent = outcome == Requested::Sent;
        self.stage = match outcome {
            Requested::Sent => Stage::AwaitInstall,
            Requested::Refused => Stage::Failed(InstallError::RequestRefused),
            Requested::Uncertain => Stage::Failed(InstallError::RequestUncertain),
        };
    }

    /// Stock's download progress.
    pub fn stock_progress(&mut self, percent: u8) {
        if self.stage == Stage::AwaitInstall {
            self.progress = Some(percent.min(100));
        }
    }

    /// Stock reported an error for the install (its own refusal, or the
    /// server's record of one), even before the request's outcome came.
    pub fn stock_refused(&mut self, code: Option<u32>) {
        if matches!(self.stage, Stage::AwaitInstall | Stage::Requesting) {
            self.stage = Stage::Failed(match code {
                Some(STOCK_CLOCK_UNSET) => InstallError::StockClockUnset,
                code => InstallError::StockRefused { code },
            });
        }
    }

    /// Stock stopped reporting after the request: it activated bank 0 and
    /// restarted into the replacement (or lost power; the replacement's
    /// answer, read next, tells which).
    pub fn stock_gone(&mut self) {
        if self.stage == Stage::AwaitInstall {
            self.stage = Stage::Pair;
        }
    }

    /// The installer's key is an owner of the replacement: a key the build
    /// already knew, or the recovery window's exchange.
    pub fn paired(&mut self) {
        if self.stage == Stage::Pair {
            self.stage = Stage::ReadStatus;
        }
    }

    /// Bluetooth setup enrolled the installer and gave the feeder the
    /// owner's Wi-Fi; it reboots to join, and is found on the LAN next.
    pub fn set_up(&mut self) {
        if self.stage == Stage::Pair && self.options.route == PairRoute::Setup {
            self.stage = Stage::AwaitJoin;
        }
    }

    /// The feeder joined the owner's Wi-Fi and a session with it opened.
    pub fn joined(&mut self) {
        if self.stage == Stage::AwaitJoin {
            self.stage = Stage::ReadStatus;
        }
    }

    /// The replacement's status after pairing: the served build on bank 0
    /// moves on, and on bank 1 (an earlier run activated it) only its
    /// confirmation is left, for which the promoted version must be known
    /// ([`Self::promoted_before`]); anything else ends the run.
    pub fn replacement(&mut self, status: Replacement<'_>) {
        if self.stage != Stage::ReadStatus {
            return;
        }
        let reported = status.build.to_ascii_lowercase();
        self.stage = match status.bank {
            _ if reported != self.build => Stage::Failed(InstallError::WrongReplacement {
                reported,
                bank: status.bank,
            }),
            // Only a request this run sent left a record to close.
            0 if self.sent => Stage::CloseTrial,
            0 => self.after_bank0(),
            _ if self.version.is_none() => Stage::Failed(InstallError::PromotionUnknown),
            _ => Stage::AwaitConfirm,
        };
    }

    /// The version an earlier run signed and promoted, from the manifest it
    /// kept: what a run resumed after the activation waits to see
    /// confirmed when it signs for itself. A manifest given to this run
    /// already names it.
    pub fn promoted_before(&mut self, version: u32) {
        if self.version.is_none() && matches!(self.options.promotion, Promotion::Sign { .. }) {
            self.version = Some(version);
        }
    }

    /// The stock server was told; whether it closed its record or not, the
    /// feeder is where it should be (a record left open only makes the next
    /// run wait for its resolution).
    pub fn trial_closed(&mut self) {
        if self.stage == Stage::CloseTrial {
            self.stage = self.after_bank0();
        }
    }

    /// From the served build on bank 0: the update key first when it needs
    /// enrolling, then the backup.
    fn after_bank0(&self) -> Stage {
        match self.options.update_key {
            UpdateKey::Enroll(key) => Stage::EnrollKey(key),
            UpdateKey::BuiltIn => Stage::Backup,
        }
    }

    /// How the one enrollment [`Step::EnrollKey`] asked for ended. Enrolled
    /// now or already listed moves on; an unanswered one is judged by the
    /// signer list ([`Step::CheckSigners`]); any other end stops the run
    /// before promotion.
    pub fn key_enrolled(&mut self, outcome: Result<Enrolled, EnrollError>) {
        if self.stage != Stage::Enrolling {
            return;
        }
        self.stage = match outcome {
            Ok(Enrolled::Now | Enrolled::Already) => Stage::Backup,
            Err(EnrollError::Uncertain) => Stage::CheckSigners,
            Err(error) => Stage::Failed(InstallError::UpdateKey(error)),
        };
    }

    /// The feeder's clients, read after an unanswered enrollment: the key
    /// listed as an update signer means the press landed.
    pub fn signers(&mut self, clients: &[Client]) {
        if self.stage != Stage::CheckSigners {
            return;
        }
        let UpdateKey::Enroll(key) = self.options.update_key else {
            return;
        };
        let listed = clients
            .iter()
            .any(|client| client.key.0 == key && client.role == ClientRole::UpdateSigner);
        self.stage = if listed {
            Stage::Backup
        } else {
            Stage::Failed(InstallError::UpdateKeyUncertain)
        };
    }

    /// The client list could not be read after an unanswered enrollment:
    /// whether it landed stays unknown, and it is not sent again.
    pub fn signers_unread(&mut self) {
        if self.stage == Stage::CheckSigners {
            self.stage = Stage::Failed(InstallError::UpdateKeyUncertain);
        }
    }

    /// A verified stock backup; it may come at any point before promotion.
    pub fn backup(&mut self, backup: VerifiedBackup) {
        self.backup = Some(backup);
    }

    /// The feeder's floor, its highest confirmed version: an offer at or
    /// below it needs a downgrade statement, which the installer never
    /// makes. A manifest signed before the run must be above it; a run that
    /// signs for itself picks one above both the floor and any version it
    /// signed before.
    pub fn floor(&mut self, floor: u32) {
        if self.stage != Stage::Floor {
            return;
        }
        self.stage = match self.options.promotion {
            Promotion::Signed(version) if version <= floor => {
                Stage::Failed(InstallError::NotAboveFloor { version, floor })
            }
            Promotion::Signed(_) => Stage::Promote,
            Promotion::Sign { above } => match floor.max(above).checked_add(1) {
                Some(version) => {
                    self.version = Some(version);
                    Stage::Sign { version }
                }
                None => Stage::Failed(InstallError::NoVersionAbove { floor }),
            },
        };
    }

    /// The manifest [`Step::Sign`] asked for is signed and kept.
    pub fn signed(&mut self) {
        if matches!(self.stage, Stage::Sign { .. }) {
            self.stage = Stage::Promote;
        }
    }

    /// The promotion's outcome: bank 1 activated, or the update's own
    /// error.
    pub fn promoted(&mut self, outcome: Result<(), UpdateError>) {
        if self.stage != Stage::Promote {
            return;
        }
        self.stage = match outcome {
            Ok(()) => Stage::AwaitConfirm,
            Err(error) => Stage::Failed(InstallError::Promotion(error)),
        };
    }

    /// The replacement after the promotion's restart, with the highest
    /// version it reports confirmed (`UpdateView::min_version`) and its
    /// readiness. Done once bank 1 runs the build and either has confirmed
    /// the promoted version, or can feed: a trial build takes no feed until it
    /// confirms, so a ready one has confirmed. Readiness is what settles a
    /// reinstall of a build the feeder confirmed before, which skips the trial
    /// and leaves the floor below the version. A trial boot still deciding
    /// waits.
    pub fn confirmed(
        &mut self,
        status: Replacement<'_>,
        confirmed_version: u32,
        readiness: Option<Readiness>,
    ) {
        if self.stage != Stage::AwaitConfirm {
            return;
        }
        let reported = status.build.to_ascii_lowercase();
        match status.bank {
            0 => self.stage = Stage::Failed(InstallError::FellBack),
            _ if reported != self.build => {
                self.stage = Stage::Failed(InstallError::WrongPromotion { reported });
            }
            _ => match self.version {
                Some(version)
                    if confirmed_version >= version || readiness == Some(Readiness::Ready) =>
                {
                    self.stage = Stage::Done;
                }
                Some(_) => {}
                None => self.stage = Stage::Failed(InstallError::PromotionUnknown),
            },
        }
    }

    /// The caller's bound on the current wait ran out.
    pub fn gave_up(&mut self) {
        self.stage = match self.stage {
            Stage::Done | Stage::Failed(_) => return,
            // Unanswered may have enrolled the key; it is never sent again.
            Stage::Enrolling | Stage::CheckSigners => {
                Stage::Failed(InstallError::UpdateKeyUncertain)
            }
            _ => Stage::Failed(InstallError::TimedOut(self.phase())),
        };
    }
}

#[cfg(test)]
mod tests;
