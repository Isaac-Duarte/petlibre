//! Why an install, or one of its shared steps, stopped. Each variant names
//! what happened; where the feeder is left and what to run next is the
//! platform's to say (the CLI adds its flags, the app its screens), so the
//! variants that call for such advice wrap their cause whole and add none.

use std::net::SocketAddr;
use std::path::PathBuf;

use wellfed_client::install::{BackupError, FrameError, InstallError, ReadError, STOCK_VERSION};
use wellfed_model::Answer;
use wellfed_model::setup::Refusal;

use crate::pairing::refusal_text;
use crate::platform::PlatformError;
use crate::track::Stopped;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The platform's own failure: a prompt, the radio, the key store.
    #[error(transparent)]
    Platform(PlatformError),
    /// Joining stock's setup network, or coming back from it, failed. Nothing
    /// is asked of stock until it reaches the broker, so a new run may go again.
    #[error("the handover over the feeder's setup network: {0}")]
    Handover(#[source] PlatformError),
    #[error("show the setup code")]
    ShowCode(#[source] PlatformError),
    #[error(transparent)]
    Connection(#[from] wellfed_connect::Error),
    #[error("connect to {address}")]
    Connect {
        address: SocketAddr,
        #[source]
        source: wellfed_connect::Error,
    },
    #[error("read the system random source")]
    Random(#[source] wellfed_formats::Error),
    #[error("the system clock is before 1970 or past 2554")]
    Clock,

    #[error("read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("read {}", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("create {}", path.display())]
    Create {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("write {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: wellfed_formats::Error,
    },
    #[error("remove {}", path.display())]
    Remove {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("read the review's build-manifest.json")]
    ReviewManifest {
        #[source]
        source: wellfed_formats::Error,
    },
    #[error("the review's build-manifest.json has no 64-hex startup identity")]
    NoStartupIdentity,
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error(transparent)]
    Signing(#[from] wellfed_signing::Error),

    #[error("start the stock server")]
    StockServer(#[source] PlatformError),
    #[error("start the image server")]
    Images(#[source] PlatformError),
    #[error("the stock server has stopped")]
    StockServerGone,
    /// It ended on its own, for a reason other than its enrollment window
    /// closing: its port was taken, or its records could not be read.
    #[error("the stock server stopped: {0}")]
    StockServerFailed(String),
    /// The server would not send the install request, and its ledger shows
    /// no record of it, so nothing reached stock.
    #[error(
        "the server refused the install request without sending it; stock is unchanged, and a new run may ask again once the reason is fixed"
    )]
    RequestRefused(#[source] wellfed_stock::server::Refusal),
    #[error(transparent)]
    Trial(#[from] wellfed_stock::server::Refusal),
    /// The image server admits one IPv4 peer: the address the enrollment
    /// recorded (or, on the bench host, the captured device's on the
    /// hotspot).
    #[error("the stock server knows no IPv4 address for the feeder to serve the image to")]
    NoPeer,
    #[error("read the enrollment {}", path.display())]
    EnrollmentUnreadable {
        path: PathBuf,
        #[source]
        source: wellfed_formats::Error,
    },
    #[error("the frame could not be sent: open a UDP socket")]
    UdpSocket(#[source] std::io::Error),
    #[error("send the setup frame to {to}")]
    FrameSend {
        to: std::net::SocketAddrV4,
        #[source]
        source: std::io::Error,
    },

    /// The broker enrolled another device than the owner pinned: whatever
    /// reached it first is not the feeder the owner means to install.
    #[error(
        "the broker enrolled device {enrolled}, connecting from {address}, but the expected device is {expected}; nothing was sent to stock. Check which device reached the broker before running again"
    )]
    OtherDevice {
        expected: String,
        enrolled: String,
        address: String,
    },
    /// The feeder this install paired with no longer answers where it was
    /// last seen, where stock connected from, or by discovery.
    #[error(
        "feeder {}, which this install paired with, did not answer where it last did, where stock connected from, or by discovery",
        wellfed_formats::hex::encode(&feeder[..8])
    )]
    NotAnswering { feeder: [u8; 32] },
    #[error("a consumer image joins no network until Bluetooth setup gives it one")]
    NoHomeNetwork,
    #[error(
        "no address for the replacement: none was given, and no image lease names where the feeder connected from"
    )]
    NoAddress,

    /// Bluetooth setup went through on another feeder than this install's.
    #[error(
        "the feeder that took setup is {}, not this install's {}: it has this client's key and the Wi-Fi, and nothing else of this install was done to it",
        wellfed_formats::hex::encode(found),
        wellfed_formats::hex::encode(expected)
    )]
    OtherFeeder { expected: [u8; 32], found: [u8; 32] },
    #[error("the feeder refused: {}", refusal_text(*.0))]
    SetupRefused(Refusal),
    #[error(transparent)]
    SetupSession(#[from] wellfed_client::setup::Error),
    #[error("the feeder never proved the code")]
    NoProof,
    #[error("the feeder closed the connection: is its recovery window open?")]
    RecoveryClosed,
    #[error("the feeder did not answer in time")]
    RecoveryTimeout,

    #[error("the feeder answered {answer:?} to a {request} request")]
    Answered {
        request: &'static str,
        answer: Box<Answer>,
    },
    #[error("the feeder refused to list its clients: {0:?}")]
    ClientsRefused(wellfed_model::Refusal),
    #[error("the feeder's identity is not known yet")]
    NoIdentity,
    #[error("no session with the replacement")]
    NoSession(#[source] Box<Error>),

    #[error("this build serves no stock backup; update bank 0 first, or pass a copy you hold")]
    NoBackupServed,
    #[error("read bank 1")]
    Bank1(#[source] ReadError),
    #[error(
        "bank 1 is not stock 1.1.15's pinned image; nothing was kept, and the feeder is unchanged"
    )]
    NotStock(#[source] BackupError),
    #[error(
        "{0} is not stock {STOCK_VERSION}'s image (damaged, partial or foreign); delete {0} to read the backup from the feeder again",
        path.display()
    )]
    BackupDamaged {
        path: PathBuf,
        #[source]
        source: BackupError,
    },
    #[error("{} already holds another copy of stock; it is never replaced", path.display())]
    BackupExists { path: PathBuf },
    #[error("the stock backup is not stock's pinned image")]
    Unverified,
    /// No verified backup could be had: the promotion, which overwrites
    /// the feeder's only copy of stock, is refused.
    #[error(transparent)]
    Backup(Box<Error>),
    /// The owner's own stock copy could not be kept before the promotion.
    #[error(transparent)]
    KeepGiven(Box<Error>),
    /// Bluetooth setup failed; the replacement runs from bank 0.
    #[error(transparent)]
    Setup(Box<Error>),

    #[error("this build takes no updates over the device API")]
    RestoreNoUpdates,
    #[error("the feeder was not found on the network")]
    RestoreNotFound,
    /// The update key a restore signs with did not reach the feeder.
    #[error("the restore's update key")]
    RestoreEnroll(#[source] wellfed_client::update_key::EnrollError),
    /// The update key could not be made or read back here.
    #[error("the restore's update key file")]
    RestoreKey(#[source] wellfed_signing::Error),
    /// The key file was gone when the run came to spend it: something else
    /// removed it (an owner leaving an earlier run's screen forgets it). The
    /// run stops, since what it signed with is no longer what this phone
    /// holds, and nothing was offered.
    #[error("the restore's update key file went away before the offer, so nothing was offered")]
    RestoreKeyGone,
    /// An install is running on the feeder now, so nothing was cancelled and
    /// nothing offered.
    #[error("an install is running on the feeder ({phase:?}), so nothing was offered")]
    RestoreInstallRunning { phase: wellfed_model::UpdatePhase },
    /// An earlier install was left pending, and the one cancel of it did not
    /// show it gone. Nothing was offered.
    #[error("an earlier install is pending and could not be cancelled, so nothing was offered")]
    RestoreCancelUnknown,
    /// Stopped with nothing offered, so the feeder keeps the replacement.
    #[error("the restore stopped on {0}, with nothing offered")]
    RestoreStopped(crate::track::Stop),

    #[error("no update key to enroll")]
    NoUpdateKey,
    #[error("no review and key to sign the promotion with")]
    NoSigner,
    #[error("no signed manifest to promote with")]
    NoManifest,
    #[error("image.bin is shorter than its header")]
    ShortImage,
    #[error("the enrollment ended before this step")]
    EnrollmentEnded,

    /// The steps ended the install.
    #[error(transparent)]
    Steps(InstallError),
    #[error(transparent)]
    Stopped(Stopped),
}

impl Error {
    /// The session with the feeder ended, with nothing said about its contents:
    /// a read may simply go again.
    pub fn is_connection_lost(&self) -> bool {
        matches!(
            self,
            Self::Platform(_) | Self::Connect { .. } | Self::Connection(_) | Self::NoSession(_)
        )
    }
}
