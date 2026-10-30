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
//!      hold runs stock's factory restore, which erases its plans, so the
//!      owner accepts that first; then one type-3 setup frame ([`frame`]) to
//!      stock's setup access point names the owner's Wi-Fi and the
//!      installer's broker.
//!    - **DNS** ([`dns`]), which keeps stock's plans for the import: the
//!      owner's resolver answers stock's broker name with the installer's
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
//! 10. **Stock's plans:** the bank-0 role serves stock's two plan sectors
//!     (`ReadStockPlans`), which [`plans`] decodes and converts; the caller
//!     keeps the bytes beside the backup and shows the owner the import. A
//!     table that cannot be read stops the run here, before promotion,
//!     unless the owner chose to go on without plans
//!     ([`Install::without_stock_plans`]).
//! 11. **Version:** the feeder's floor (`UpdateState`) decides it. A
//!     manifest signed before the run must be above it; otherwise the
//!     installer signs the build itself, above both the floor and any
//!     version it signed before ([`Promotion`]).
//! 12. **Promote:** the signed update ([`crate::update::Installer`]) writes
//!     bank 1 from bank 0, and its trial boot confirms on bank 1.
//! 13. **Apply the plans:** once bank 1 confirms, the converted plans are
//!     put, and only onto a table with every slot empty: plans the owner
//!     already set on the replacement are never replaced. The put goes
//!     once; one left unanswered is judged by reading the plans back, never
//!     by sending it again.
//!
//! A step that fails ends the run ([`Step::Failed`]); nothing is retried
//! here, and the caller bounds every wait ([`Install::gave_up`]).
//!
//! [consumer installation]: ../../../../docs/firmware/consumer-installation.md

pub mod backup;
pub mod dns;
pub mod frame;
pub mod plans;

pub use backup::{
    BackupError, Bank1Reader, CONTAINER_HEADER, DOWNGRADE_LEN, ReadError, RestoreError,
    RestoreOffer, STOCK_OTA_SHA256, STOCK_PAYLOAD_LEN, STOCK_PAYLOAD_SHA256, STOCK_VERSION,
    StockBackup, StockPins, VerifiedBackup,
};
pub use dns::{DnsCheck, DnsError, DnsOverride, STOCK_BROKER_PORT};
pub use frame::{FrameError, SETUP_ADDRESS, SetupFrame};
pub use plans::{DecodeError, Import, StockPlans};

use wellfed_model::{Client, ClientRole, Generation, PLANS, Plan, Plans, Readiness, Refusal};

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
    /// The owner holds Reset, which runs stock's factory restore (its plans
    /// erased with the rest) and opens its setup access point; then this
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
    StockPlans,
    /// The floor read and the version chosen or checked, and the signing.
    Sign,
    Promote,
    AwaitConfirm,
    ApplyPlans,
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
    #[error(
        "the owner kept stock's plans: the Reset hold erases them, so nothing was done; point stock's broker name at the installer in your DNS instead"
    )]
    ResetDeclined,
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
    /// Stock's plan sectors do not decode. Before promotion this stops the
    /// run with stock intact in bank 1; with the owner's choice to go on, or
    /// from a kept copy, it is the reason the plans were left behind.
    #[error("stock's plans could not be read: {0}")]
    StockPlans(DecodeError),
    #[error("the feeder did not serve stock's plans ({0:?})")]
    StockPlansUnread(Refusal),
    #[error(
        "the replacement is installed and confirmed, but it already has plans in slots {slots:?}; stock's plans were not applied over them"
    )]
    PlansAlreadySet { slots: Vec<usize> },
    #[error(
        "the replacement is installed and confirmed, but its plans cannot be written now; stock's plans were not applied"
    )]
    PlansNotWritable,
    #[error(
        "the replacement is installed and confirmed, but its plans changed (now generation {}) before stock's were put; nothing was changed",
        current.0
    )]
    PlansStale { current: Generation },
    #[error(
        "the replacement is installed and confirmed, but it refused stock's plans ({0:?}); nothing was changed"
    )]
    PlansRefused(Refusal),
    #[error(
        "the replacement is installed and confirmed, but a put of stock's plans went out and whether they are on it is unknown: the plans read back are not the import, and it is not sent again: check the meals on the feeder and set any that are missing"
    )]
    PlansUncertain,
    #[error(
        "the replacement is installed and confirmed, but its plans could not be read; stock's plans were not applied"
    )]
    PlansUnread,
}

/// What became of the one `PutPlans` of the import.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlansPut {
    Done,
    Refused(Refusal),
    /// No answer came: it may or may not have been stored.
    Unanswered,
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
    /// Warn the owner, before the feeder is touched, that the Reset hold
    /// runs stock's factory restore and erases its plans, so they note them
    /// first (the vendor app shows them); then [`Install::reset_accepted`]
    /// or [`Install::reset_declined`].
    ConfirmReset,
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
    /// Read stock's plan sectors from the bank-0 role (`ReadStockPlans`)
    /// and report them with [`Install::stock_plans`] (or a refusal with
    /// [`Install::stock_plans_unread`]); keep the bytes beside the backup
    /// and show the owner [`Install::import`].
    ReadStockPlans,
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
    /// Read the replacement's plans (`GetPlans`) and report them with
    /// [`Install::plans`]: before the import is put, to see the table is
    /// empty, and after a put that went unanswered, to judge it.
    ReadPlans,
    /// Send `PutPlans` with these values once, and report the answer with
    /// [`Install::plans_put`]. Goes once in a run. Keep `base` durably before it
    /// leaves: a later run reports it with [`Install::earlier_put`], and goes
    /// again only if the plans are still at it.
    PutPlans {
        base: Generation,
        plans: [Option<Plan>; PLANS],
    },
    /// The put is out; waiting for [`Install::plans_put`].
    AwaitPut,
    /// The replacement is confirmed on bank 1, with stock's plans applied
    /// when there were any.
    Done,
    Failed(InstallError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Stage {
    PointDns(DnsOverride),
    /// The frame waits for the owner to accept losing stock's plans.
    ConfirmReset(FrameBytes),
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
    StockPlans,
    Floor,
    Sign {
        version: u32,
    },
    Promote,
    AwaitConfirm,
    /// Waiting for the plans to check the table is empty.
    CheckPlans,
    PutPlans {
        base: Generation,
    },
    /// The put was sent; waiting for its answer.
    Putting,
    /// The put went unanswered: waiting for the plans read back.
    VerifyPlans,
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
    /// Stock's plans, converted, once read.
    import: Option<Import>,
    /// The owner chose to promote even when stock's plans cannot be read.
    without_plans: bool,
    /// Why stock's plans were left behind, when the owner chose to go on.
    plans_skipped: Option<InstallError>,
    /// What an earlier run's put of them left on record.
    earlier_put: EarlierPut,
}

/// What an earlier run's put of stock's plans left on record, which is on disk
/// before any put leaves. So `None` means no put ever left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EarlierPut {
    None,
    /// One may have left, against this generation.
    From(Generation),
    /// There is a record that cannot be read: one may have left.
    Unread,
}

impl Install {
    /// A run that installs `build` (the served image's identity, 64 hex),
    /// pairs by `options`' route and promotes as they say. A frame whose
    /// values stock would misread is refused here, before anything is
    /// sent.
    pub fn new(provision: Provision, build: &str, options: Options) -> Result<Self, FrameError> {
        let stage = match provision {
            Provision::Dns(dns) => Stage::PointDns(dns),
            Provision::Frame(frame) => Stage::ConfirmReset(FrameBytes(frame.encode()?)),
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
            import: None,
            without_plans: false,
            plans_skipped: None,
            earlier_put: EarlierPut::None,
        }
    }

    /// The next thing to do.
    pub fn step(&mut self) -> Step {
        match &self.stage {
            Stage::PointDns(dns) => Step::PointDns(dns.clone()),
            Stage::ConfirmReset(_) => Step::ConfirmReset,
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
                self.stage = Stage::StockPlans;
                Step::ReadStockPlans
            }
            Stage::Backup => Step::Backup,
            Stage::StockPlans => Step::ReadStockPlans,
            Stage::Floor => Step::ReadFloor,
            &Stage::Sign { version } => Step::Sign { version },
            Stage::Promote => Step::Promote,
            Stage::AwaitConfirm => Step::AwaitConfirm,
            Stage::CheckPlans | Stage::VerifyPlans => Step::ReadPlans,
            &Stage::PutPlans { base } => {
                let plans = self
                    .import
                    .as_ref()
                    .map_or([None; PLANS], |import| import.plans);
                self.stage = Stage::Putting;
                Step::PutPlans { base, plans }
            }
            // Sent once: until its answer comes, there is only waiting.
            Stage::Putting => Step::AwaitPut,
            Stage::Done => Step::Done,
            Stage::Failed(error) => Step::Failed(error.clone()),
        }
    }

    /// Where the run stands, for a progress display.
    pub fn phase(&self) -> Phase {
        match self.stage {
            Stage::PointDns(_) | Stage::ConfirmReset(_) | Stage::Provision(_) | Stage::FrameOut => {
                Phase::Provision
            }
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
            Stage::StockPlans => Phase::StockPlans,
            Stage::Floor | Stage::Sign { .. } => Phase::Sign,
            Stage::Promote => Phase::Promote,
            Stage::AwaitConfirm => Phase::AwaitConfirm,
            Stage::CheckPlans | Stage::PutPlans { .. } | Stage::Putting | Stage::VerifyPlans => {
                Phase::ApplyPlans
            }
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

    /// Stock's plans as converted, once read.
    pub fn import(&self) -> Option<&Import> {
        self.import.as_ref()
    }

    /// Why stock's plans were left behind, when the owner chose to promote
    /// without them.
    pub fn plans_skipped(&self) -> Option<&InstallError> {
        self.plans_skipped.as_ref()
    }

    /// The owner chose to promote even if stock's plans cannot be read:
    /// the run then goes on without them, and says why.
    pub fn without_stock_plans(&mut self) {
        self.without_plans = true;
    }

    /// The owner says their resolver answers stock's broker name with the
    /// installer's broker and the feeder restarted: wait for stock there.
    pub fn dns_pointed(&mut self) {
        if matches!(self.stage, Stage::PointDns(_)) {
            self.stage = Stage::AwaitStock;
        }
    }

    /// The owner noted stock's plans and accepts the Reset hold erasing
    /// them: the frame comes next.
    pub fn reset_accepted(&mut self) {
        if let Stage::ConfirmReset(frame) = &self.stage {
            self.stage = Stage::Provision(frame.clone());
        }
    }

    /// The owner keeps stock's plans: nothing is done.
    pub fn reset_declined(&mut self) {
        if matches!(self.stage, Stage::ConfirmReset(_)) {
            self.stage = Stage::Failed(InstallError::ResetDeclined);
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

    /// Stock's plan sectors as the bank-0 role served them, read at
    /// `now_ms` (Unix milliseconds, which decides whether a skip is still
    /// in force). A readable table is converted, and promotion follows; one
    /// that cannot be read stops the run unless the owner chose to go on
    /// without it.
    pub fn stock_plans(&mut self, count: u8, entries: &[u8], now_ms: u64) {
        if self.stage != Stage::StockPlans {
            return;
        }
        match StockPlans::decode_parts(count, entries) {
            Ok(stock) => {
                self.import = Some(plans::convert(&stock, now_ms));
                self.stage = Stage::Floor;
            }
            Err(error) => self.skip_plans(InstallError::StockPlans(error)),
        }
    }

    /// The feeder refused to serve stock's plans.
    pub fn stock_plans_unread(&mut self, refusal: Refusal) {
        if self.stage == Stage::StockPlans {
            self.skip_plans(InstallError::StockPlansUnread(refusal));
        }
    }

    /// Stock's plans as an earlier run kept them (`plans::kept`), for a
    /// run that did not read the sectors itself: one resumed after the
    /// promotion, when bank 0 no longer serves them. Taken only before the
    /// plans are checked and only when this run has no import, so the same
    /// empty-and-writable check applies. A copy that does not decode is
    /// recorded as the reason the plans were left behind: the run that
    /// kept it promoted without them.
    pub fn kept_stock_plans(&mut self, kept: &[u8], now_ms: u64) {
        let before_check = matches!(
            self.stage,
            Stage::Pair
                | Stage::ReadStatus
                | Stage::AwaitConfirm
                | Stage::Floor
                | Stage::Sign { .. }
                | Stage::Promote
        );
        if !before_check || self.import.is_some() || self.plans_skipped.is_some() {
            return;
        }
        match StockPlans::decode_kept(kept) {
            Ok(stock) => self.import = Some(plans::convert(&stock, now_ms)),
            Err(error) => self.plans_skipped = Some(InstallError::StockPlans(error)),
        }
    }

    fn skip_plans(&mut self, error: InstallError) {
        if self.without_plans {
            self.plans_skipped = Some(error);
            self.stage = Stage::Floor;
        } else {
            self.stage = Stage::Failed(error);
        }
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
                    self.stage = self.after_confirm();
                }
                Some(_) => {}
                None => self.stage = Stage::Failed(InstallError::PromotionUnknown),
            },
        }
    }

    /// Done, unless there are converted plans to apply.
    fn after_confirm(&self) -> Stage {
        let converted = self
            .import
            .as_ref()
            .is_some_and(|import| import.plans.iter().any(Option::is_some));
        if converted {
            Stage::CheckPlans
        } else {
            Stage::Done
        }
    }

    /// An earlier run may have sent its put of the import, against `base`.
    /// Reported before the confirmation, as [`Install::kept_stock_plans`] is.
    pub fn earlier_put(&mut self, base: Generation) {
        if self.stage == Stage::AwaitConfirm {
            self.earlier_put = EarlierPut::From(base);
        }
    }

    /// An earlier run left a record of its put that cannot be read: treated as
    /// a put against a table that has moved, so nothing is sent.
    pub fn earlier_put_unread(&mut self) {
        if self.stage == Stage::AwaitConfirm {
            self.earlier_put = EarlierPut::Unread;
        }
    }

    /// The replacement's plans, read for [`Step::ReadPlans`]. Before the
    /// put, the import goes only onto a writable table with every slot
    /// empty. After an unanswered put in this run, the table read back is the
    /// judge: the import there is done, anything else is uncertain, and this
    /// run does not send it again.
    ///
    /// After an earlier run's put ([`Install::earlier_put`]), the import there
    /// is done, and otherwise it goes again only from the generation that put
    /// was sent against. Every change to the table moves the generation, so
    /// that one is proof neither the put nor the owner changed it; an empty
    /// table alone is not, since the owner may have emptied it. Anything else
    /// is uncertain, and nothing is sent.
    pub fn plans(&mut self, view: &Plans) {
        match self.stage {
            // The import already there: a put an earlier run sent landed.
            Stage::CheckPlans
                if self
                    .import
                    .as_ref()
                    .is_some_and(|import| import.plans == view.plans) =>
            {
                self.stage = Stage::Done;
            }
            Stage::CheckPlans => {
                let slots: Vec<usize> = view
                    .plans
                    .iter()
                    .enumerate()
                    .filter_map(|(slot, plan)| plan.map(|_| slot))
                    .collect();
                let moved = match self.earlier_put {
                    EarlierPut::None => false,
                    EarlierPut::From(base) => {
                        base != view.generation || !slots.is_empty() || !view.writable
                    }
                    EarlierPut::Unread => true,
                };
                self.stage = if moved {
                    Stage::Failed(InstallError::PlansUncertain)
                } else if !slots.is_empty() {
                    Stage::Failed(InstallError::PlansAlreadySet { slots })
                } else if !view.writable {
                    Stage::Failed(InstallError::PlansNotWritable)
                } else {
                    Stage::PutPlans {
                        base: view.generation,
                    }
                };
            }
            Stage::VerifyPlans => {
                let applied = self
                    .import
                    .as_ref()
                    .is_some_and(|import| import.plans == view.plans);
                self.stage = if applied {
                    Stage::Done
                } else {
                    Stage::Failed(InstallError::PlansUncertain)
                };
            }
            _ => {}
        }
    }

    /// What became of the one put [`Step::PutPlans`] asked for. A refusal
    /// changed nothing; no answer is judged by reading the plans back. Refused
    /// after an earlier run's put, it says nothing of that one: it may be what
    /// moved the table.
    pub fn plans_put(&mut self, outcome: PlansPut) {
        if self.stage != Stage::Putting {
            return;
        }
        self.stage = match outcome {
            PlansPut::Done => Stage::Done,
            PlansPut::Refused(_) if self.earlier_put != EarlierPut::None => {
                Stage::Failed(InstallError::PlansUncertain)
            }
            PlansPut::Refused(Refusal::Stale { current }) => {
                Stage::Failed(InstallError::PlansStale { current })
            }
            PlansPut::Refused(refusal) => Stage::Failed(InstallError::PlansRefused(refusal)),
            PlansPut::Unanswered => Stage::VerifyPlans,
        };
    }

    /// The caller's bound on the current wait ran out.
    pub fn gave_up(&mut self) {
        self.stage = match self.stage {
            Stage::Done | Stage::Failed(_) => return,
            // Installed and confirmed: what is left unknown is the plans.
            Stage::CheckPlans | Stage::PutPlans { .. } if self.earlier_put != EarlierPut::None => {
                Stage::Failed(InstallError::PlansUncertain)
            }
            Stage::CheckPlans | Stage::PutPlans { .. } => Stage::Failed(InstallError::PlansUnread),
            Stage::Putting | Stage::VerifyPlans => Stage::Failed(InstallError::PlansUncertain),
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
