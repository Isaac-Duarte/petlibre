//! The consumer installer's steps, sans-IO: from a feeder on stock 1.1.15
//! to the replacement confirmed on bank 1 ([consumer installation]). The
//! Android app, `wellfed-server` and the `wellfed` CLI run these same
//! steps; each supplies the IO (joining Wi-Fi, the UDP frame, the broker and
//! image server stock talks to, the device API session).
//!
//! The run, in order ([`Step`] says which is next):
//! 1. **Provision:** one type-3 setup frame ([`frame`]) to stock's setup
//!    access point names the owner's Wi-Fi and the installer's broker. Stock
//!    then connects there with its own MQTT credentials, which the broker
//!    trusts on first use; nothing captured, nothing redirected.
//! 2. **Stock ready:** stock reports 1.1.15 on its adapter, the same
//!    conditions its own precheck and the server's trial gate hold.
//! 3. **Request:** one `OTA_UPGRADE`, never resent. A refused precheck can
//!    erase stock's OTA status sector and an uncertain request may still be
//!    downloading, so whatever happens to it ends this run; a new run
//!    starts only after the server has resolved it.
//! 4. **Install:** stock downloads the image, checks its byte-sum, writes
//!    bank 0 and restarts into the replacement, which then answers the
//!    device API from bank 0.
//! 5. **Pair:** the installer becomes the replacement's owner through
//!    pairing (the setup code keyed in on the panel, over BLE or the LAN
//!    recovery window). A key the build already knows counts.
//! 6. **Bank 0:** the device API names the served build on bank 0, and the
//!    stock server is told so it closes its record.
//! 7. **Backup:** promotion overwrites bank 1, the feeder's only copy of
//!    stock, so it waits for a [`VerifiedBackup`]. There is no way past this
//!    step without one.
//! 8. **Promote:** the signed update ([`crate::update::Installer`]) writes
//!    bank 1 from bank 0, and its trial boot confirms on bank 1.
//!
//! A step that fails ends the run ([`Step::Failed`]); nothing is retried
//! here, and the caller bounds every wait ([`Install::gave_up`]).
//!
//! [consumer installation]: ../../../../docs/firmware/consumer-installation.md

pub mod backup;
pub mod frame;

pub use backup::{BackupError, STOCK_OTA_SHA256, STOCK_VERSION, VerifiedBackup};
pub use frame::{FrameError, SETUP_ADDRESS, SetupFrame};

use crate::update::InstallError as UpdateError;

/// How stock is pointed at the installer's broker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Provision {
    /// Send this frame to stock's setup access point first.
    Frame(SetupFrame),
    /// Stock already connects to the installer's broker (provisioned by an
    /// earlier frame, or on the bench).
    Done,
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

/// The installer's view of stock, from its broker's session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StockView {
    /// Stock's telemetry is current.
    pub online: bool,
    /// The version it last reported, if it reported one recently.
    pub version: Option<String>,
    pub power: Option<StockPower>,
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
    /// The server refused it before sending anything.
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

/// Which step a wait was given up on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Provision,
    AwaitStock,
    Request,
    AwaitInstall,
    Pair,
    ReadStatus,
    CloseTrial,
    Backup,
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
        "the server refused the install request without sending it; stock is unchanged, and a new run may ask again once the reason is fixed"
    )]
    RequestRefused,
    #[error(
        "the install request may have reached stock; never resent: wait for the server to resolve it before a new run"
    )]
    RequestUncertain,
    #[error("stock refused or failed the install (error {code:?}); it keeps running stock")]
    StockRefused { code: Option<u32> },
    #[error("build {reported} answers on bank {bank}, not the served build on bank 0")]
    WrongReplacement { reported: String, bank: u8 },
    #[error("the promotion stopped: {0}; the feeder keeps running bank 0, stock still in bank 1")]
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
    /// With this host joined to stock's setup access point, send these bytes
    /// once to [`SETUP_ADDRESS`] over UDP, then call
    /// [`Install::frame_sent`]. Never resent: stock applies a frame once.
    SendFrame(Vec<u8>),
    /// Serve the broker and report stock with [`Install::stock`].
    AwaitStock,
    /// Serve the image and ask stock once to install it; report with
    /// [`Install::requested`]. Comes once per run.
    RequestInstall,
    /// Stock is installing: report progress, a refusal, or its session
    /// ending as it restarts into the replacement.
    AwaitInstall,
    /// Admit this installer as the replacement's owner (or find its key
    /// already enrolled); then [`Install::paired`].
    Pair,
    /// Read the replacement's status; report with [`Install::replacement`].
    ReadStatus,
    /// Tell the stock server the served build answered from bank 0 so it
    /// closes its record; then [`Install::trial_closed`], whatever it said.
    CloseTrial,
    /// Promotion is refused until [`Install::backup`] is given a verified
    /// stock backup.
    Backup,
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
    Provision(Vec<u8>),
    /// The frame is out; waiting for the caller to say so.
    FrameOut,
    AwaitStock,
    Request,
    Requesting,
    AwaitInstall,
    Pair,
    ReadStatus,
    CloseTrial,
    Backup,
    Promote,
    AwaitConfirm {
        version: u32,
    },
    Done,
    Failed(InstallError),
}

/// One install run.
#[derive(Debug)]
pub struct Install {
    stage: Stage,
    /// The served build's identity, lowercase hex.
    build: String,
    backup: Option<VerifiedBackup>,
    /// Stock's latest download progress, percent.
    progress: Option<u8>,
    /// This run's request reached stock's session, so the stock server holds
    /// a record for it to close.
    sent: bool,
}

impl Install {
    /// A run that installs `build` (the served image's identity, 64 hex).
    /// A frame whose values stock would misread is refused here, before
    /// anything is sent.
    pub fn new(provision: Provision, build: &str) -> Result<Self, FrameError> {
        let stage = match provision {
            Provision::Frame(frame) => Stage::Provision(frame.encode()?),
            Provision::Done => Stage::AwaitStock,
        };
        Ok(Self {
            stage,
            build: build.to_ascii_lowercase(),
            backup: None,
            progress: None,
            sent: false,
        })
    }

    /// A run that resumes with the replacement already on bank 0 (stock
    /// installed it in an earlier run, which stopped before promotion):
    /// it starts at pairing, and stock is not asked anything.
    pub fn from_bank0(build: &str) -> Self {
        Self {
            stage: Stage::Pair,
            build: build.to_ascii_lowercase(),
            backup: None,
            progress: None,
            sent: false,
        }
    }

    /// The next thing to do.
    pub fn step(&mut self) -> Step {
        match &self.stage {
            Stage::Provision(frame) => {
                let frame = frame.clone();
                self.stage = Stage::FrameOut;
                Step::SendFrame(frame)
            }
            Stage::FrameOut | Stage::AwaitStock => Step::AwaitStock,
            Stage::Request => {
                self.stage = Stage::Requesting;
                Step::RequestInstall
            }
            // Asked once: until its outcome comes, there is only waiting.
            Stage::Requesting | Stage::AwaitInstall => Step::AwaitInstall,
            Stage::Pair => Step::Pair,
            Stage::ReadStatus => Step::ReadStatus,
            Stage::CloseTrial => Step::CloseTrial,
            Stage::Backup if self.backup.is_some() => {
                self.stage = Stage::Promote;
                Step::Promote
            }
            Stage::Backup => Step::Backup,
            Stage::Promote => Step::Promote,
            Stage::AwaitConfirm { .. } => Step::AwaitConfirm,
            Stage::Done => Step::Done,
            Stage::Failed(error) => Step::Failed(error.clone()),
        }
    }

    /// Where the run stands, for a progress display.
    pub fn phase(&self) -> Phase {
        match self.stage {
            Stage::Provision(_) | Stage::FrameOut => Phase::Provision,
            Stage::AwaitStock => Phase::AwaitStock,
            Stage::Request | Stage::Requesting => Phase::Request,
            Stage::AwaitInstall => Phase::AwaitInstall,
            Stage::Pair => Phase::Pair,
            Stage::ReadStatus => Phase::ReadStatus,
            Stage::CloseTrial => Phase::CloseTrial,
            Stage::Backup => Phase::Backup,
            Stage::Promote => Phase::Promote,
            Stage::AwaitConfirm { .. } => Phase::AwaitConfirm,
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

    /// The backup promotion waits for, once given.
    pub fn verified_backup(&self) -> Option<VerifiedBackup> {
        self.backup
    }

    /// The frame went out (the caller left stock's access point after).
    pub fn frame_sent(&mut self) {
        if self.stage == Stage::FrameOut {
            self.stage = Stage::AwaitStock;
        }
    }

    /// Stock as the broker sees it. Ready moves the run to its one
    /// request; a version other than [`STOCK_VERSION`] ends it.
    pub fn stock(&mut self, view: &StockView) -> Result<(), StockNotReady> {
        if !matches!(self.stage, Stage::AwaitStock | Stage::FrameOut) {
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
        self.stage = Stage::Request;
        Ok(())
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
    /// server's record of one).
    pub fn stock_refused(&mut self, code: Option<u32>) {
        if self.stage == Stage::AwaitInstall {
            self.stage = Stage::Failed(InstallError::StockRefused { code });
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

    /// The installer's key is an owner of the replacement.
    pub fn paired(&mut self) {
        if self.stage == Stage::Pair {
            self.stage = Stage::ReadStatus;
        }
    }

    /// The replacement's status after pairing: the served build on bank 0
    /// moves on; anything else ends the run.
    pub fn replacement(&mut self, status: Replacement<'_>) {
        if self.stage != Stage::ReadStatus {
            return;
        }
        let reported = status.build.to_ascii_lowercase();
        self.stage = if reported == self.build && status.bank == 0 {
            // Only a request this run sent left a record to close.
            if self.sent {
                Stage::CloseTrial
            } else {
                Stage::Backup
            }
        } else {
            Stage::Failed(InstallError::WrongReplacement {
                reported,
                bank: status.bank,
            })
        };
    }

    /// The stock server was told; whether it closed its record or not, the
    /// feeder is where it should be (a record left open only makes the next
    /// run wait for its resolution).
    pub fn trial_closed(&mut self) {
        if self.stage == Stage::CloseTrial {
            self.stage = Stage::Backup;
        }
    }

    /// A verified stock backup; it may come at any point before promotion.
    pub fn backup(&mut self, backup: VerifiedBackup) {
        self.backup = Some(backup);
    }

    /// The promotion's outcome: bank 1 activated at `version` (the signed
    /// manifest's), or the update's own error.
    pub fn promoted(&mut self, outcome: Result<u32, UpdateError>) {
        if self.stage != Stage::Promote {
            return;
        }
        self.stage = match outcome {
            Ok(version) => Stage::AwaitConfirm { version },
            Err(error) => Stage::Failed(InstallError::Promotion(error)),
        };
    }

    /// The replacement after the promotion's restart, with the highest
    /// version it reports confirmed (`UpdateView::min_version`). Done once
    /// bank 1 runs the build and has confirmed the promoted version; a
    /// trial boot still deciding waits.
    pub fn confirmed(&mut self, status: Replacement<'_>, confirmed_version: u32) {
        let Stage::AwaitConfirm { version } = self.stage else {
            return;
        };
        let reported = status.build.to_ascii_lowercase();
        match status.bank {
            0 => self.stage = Stage::Failed(InstallError::FellBack),
            _ if reported != self.build => {
                self.stage = Stage::Failed(InstallError::WrongPromotion { reported });
            }
            _ if confirmed_version >= version => self.stage = Stage::Done,
            _ => {}
        }
    }

    /// The caller's bound on the current wait ran out.
    pub fn gave_up(&mut self) {
        if !matches!(self.stage, Stage::Done | Stage::Failed(_)) {
            self.stage = Stage::Failed(InstallError::TimedOut(self.phase()));
        }
    }
}

#[cfg(test)]
mod tests;
