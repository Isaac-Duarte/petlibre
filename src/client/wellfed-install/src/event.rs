//! What an install tells the person as it goes, typed so each platform
//! words it its own way: the CLI prints the lines it always printed, the
//! app draws its screens. An event never asks anything; questions go
//! through the [`Prompter`](crate::Prompter).
//!
//! A failure an event reports in passing (a read the run tries again, a
//! record it could not remove) carries its text, rendered with its causes:
//! the run has already decided what it means.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use wellfed_client::install::{DnsCheck, Import, InstallError, StockNotReady};
use wellfed_client::update_key::Enrolled;

/// One step of progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The resolver this computer asks answered stock's broker `name`
    /// with `check`, where `broker` is this installer.
    DnsChecked {
        name: String,
        broker: IpAddr,
        check: DnsCheck,
    },
    /// The feeder restarts; the run waits for stock at the broker.
    AwaitingStock,
    /// Stock's server runs, its broker trusting the first feeder.
    StockServerStarted,
    /// The setup frame went out once; the feeder's answer, if any.
    FrameSent {
        answer: Option<Vec<u8>>,
    },
    /// Why the run still waits for stock.
    Waiting(Wait),
    /// Stock connected but its startup reports were lost; the owner
    /// restarts it once to replay them.
    VersionLost,
    /// Stock's server stopped on its own, so stock is not waited for.
    StockServerEnded {
        error: String,
    },
    /// The broker enrolled the device the owner pinned ahead of time.
    DevicePinned {
        device: String,
        address: String,
    },
    /// The one install request is going out.
    Requesting {
        build: String,
        request: uuid::Uuid,
    },
    /// Stock's download, in percent.
    StockProgress(u8),
    /// Stock went quiet: it restarts into the replacement.
    StockRestarting,
    /// The replacement knows this client's key already.
    KeyKnown,
    /// The replacement knows this client's key, at `address`.
    KeyKnownAt {
        identity: [u8; 32],
        address: SocketAddr,
    },
    /// No session opened with the feeder the key file knows.
    KnownKeyRefused {
        error: String,
    },
    /// Paired through the recovery window.
    Recovered {
        identity: [u8; 32],
        outcome: crate::pairing::Outcome,
    },
    /// The replacement has no network yet, so it starts Bluetooth setup.
    SetupStarting,
    /// No feeder was found in Bluetooth setup mode within the wait.
    NoSetupFeeder {
        error: String,
    },
    /// Paired over Bluetooth; the feeder reboots to join the network.
    SetUp {
        identity: [u8; 32],
        outcome: crate::pairing::Outcome,
    },
    /// The replacement joined the owner's network.
    Joined {
        address: SocketAddr,
    },
    /// The replacement answers with `build` from `bank`.
    ReplacementAnswers {
        build: String,
        bank: u8,
    },
    /// Stock's server closed its record of the request.
    TrialClosed,
    /// Stock's server kept its record open.
    TrialOpen {
        error: String,
    },
    /// The update key with id `key`, after its one enrollment.
    KeyEnrollment {
        key: [u8; 8],
        outcome: Enrollment,
    },
    /// A request of the enrollment went unanswered.
    EnrollUnanswered {
        error: String,
    },
    /// The Dispense press could not be announced (the request still runs).
    PressNotAnnounced {
        error: String,
    },
    /// The feeder's client list could not be read after an unanswered
    /// enrollment.
    SignersUnread {
        error: String,
    },
    /// The feeder's floor, its highest confirmed version.
    Floor(u32),
    FloorUnread {
        error: String,
    },
    /// The build was signed for the promotion and kept at `path`.
    Signed {
        build: String,
        version: u32,
        key: [u8; 8],
        path: PathBuf,
    },
    /// The owner's own stock copy was kept at `path` before the promotion.
    GivenBackupKept {
        path: PathBuf,
    },
    /// The stock backup an earlier run kept, at `path`.
    BackupFound {
        path: PathBuf,
    },
    /// Bank 1 is being read from the feeder.
    ReadingBank1 {
        identity: [u8; 32],
    },
    /// Bytes of bank 1 read so far.
    Bank1Read(usize),
    /// The verified backup was kept at `path`.
    BackupKept {
        path: PathBuf,
    },
    /// Stock's plan sectors were kept at `path`.
    PlansKept {
        path: PathBuf,
    },
    /// Stock's plans, as the replacement will run them, read now or (with
    /// `kept`) from the copy an earlier run kept there.
    StockPlans {
        import: Import,
        kept: Option<PathBuf>,
    },
    /// The owner chose to go on without stock's plans, for this reason.
    WithoutStockPlans {
        skipped: InstallError,
    },
    /// Bank 0 writes the replacement into bank 1.
    Promoting,
    /// The push's progress.
    Written {
        written: u32,
        length: u32,
    },
    /// The push's session dropped; it resumes on a new one.
    SessionDropped {
        error: String,
    },
    /// Bank 1 was found running the build on a new session.
    AlreadyActivated,
    /// Bank 1 is activated; the feeder restarts into it.
    Activated,
    /// The replacement's plans could not be read.
    PlansUnread {
        error: String,
    },
    /// Stock's converted plans go to the replacement, once.
    PuttingPlans,
    /// The install is confirmed on bank 1.
    Installed {
        build: String,
    },
    /// What became of stock's plans.
    Plans(PlansResult),
    /// The applied copy of stock's plans was set aside at `path`.
    PlansSetAside {
        path: PathBuf,
    },
    /// The resume record was not removed (the install is done regardless).
    ResumeRecordKept {
        error: String,
    },
}

/// Why the run still waits for stock to be ready.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wait {
    /// No feeder has enrolled at the broker yet, so the server's session
    /// with stock has not started.
    NoFeeder,
    /// Stock's server cannot read its own status.
    Unreadable {
        error: String,
    },
    Stock(StockNotReady),
}

/// How the update key's one enrollment ended, as far as it is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enrollment {
    Enrolled(Enrolled),
    /// No answer came; the feeder's signer list decides instead.
    Unanswered,
}

/// What became of stock's plans at the end of the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlansResult {
    /// Skipped, for this reason.
    Skipped(InstallError),
    /// Neither read by this run nor kept unapplied by an earlier one.
    NotRead,
    /// Set on the replacement.
    Set,
    /// Stock held none.
    None,
    /// None converted: nothing was set.
    NoneConverted,
}
