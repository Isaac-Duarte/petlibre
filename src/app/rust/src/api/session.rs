//! A feeder's device API session, as Dart holds it: commands go in through
//! [`FeederSession`], and everything the screens show comes out of the
//! stream [`FeederSession::run`] feeds.

use std::net::SocketAddr;
use std::sync::Mutex;

use flutter_rust_bridge::frb;
use tokio::sync::mpsc;
use wellfed_model::Refusal;
use wellfed_session::noise::Keypair;

use crate::api::error::{BridgeError, ErrorKind};
use crate::api::keys::ClientKey;
use crate::api::model::{Blocked, ClientRole, FeedOutcome, FeedReason, FoodLevel};
use crate::frb_generated::StreamSink;
use crate::session::{self, Command};

/// What the Home card shows, from the feeder's status.
pub struct Card {
    pub build: String,
    pub food: FoodLevel,
    /// The panel's child lock; `None` before the feeder knows.
    pub locked: Option<bool>,
    /// A feed can start now.
    pub ready: bool,
    /// Why a feed cannot start now, when the feeder says.
    pub blocked: Option<Blocked>,
    /// On the adapter; `None` with no fresh sample.
    pub adapter: Option<bool>,
    pub battery_percent: Option<u8>,
    /// `false` while the feeder's clock is invalid: meals wait for it.
    pub clock_set: bool,
    /// Wi-Fi signal, dBm.
    pub rssi: i32,
    /// How the last feed this boot ended when it ended on a fault.
    pub fault: Option<FeedOutcome>,
}

/// A feed's end, from its `Fed` event. An uncertain feed delivered between
/// `confirmed` and `at_most` portions and is never shown as one number.
pub struct FeedResult {
    pub feed: u32,
    pub requested: u8,
    pub confirmed: u8,
    pub at_most: u8,
    pub outcome: FeedOutcome,
    pub reason: FeedReason,
    pub uncertain: bool,
    /// The history's words for it, the same as the CLI's.
    pub words: String,
}

/// Why the feeder refused a feed or its cancel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedRefusal {
    /// This phone's role may not feed.
    Forbidden,
    /// Another change is being written, or the stores are loading.
    Busy,
    /// The feeding owner stopped on a fault until a restart.
    Fault,
    /// Outside 1 to 48 portions.
    Portions,
    /// No such feed is running (a cancel).
    NotFound,
    /// The firmware serves no feeding.
    Unavailable,
    /// Anything else the device API can refuse.
    Other,
}

impl From<Refusal> for FeedRefusal {
    fn from(refusal: Refusal) -> Self {
        match refusal {
            Refusal::Forbidden => Self::Forbidden,
            Refusal::Busy => Self::Busy,
            Refusal::Fault => Self::Fault,
            Refusal::Portions { .. } => Self::Portions,
            Refusal::NotFound => Self::NotFound,
            Refusal::Unavailable => Self::Unavailable,
            _ => Self::Other,
        }
    }
}

pub enum SessionUpdate {
    Opened {
        build: String,
        role: ClientRole,
    },
    Status {
        card: Card,
    },
    /// The feeder reserved the feed under `feed` and runs it now.
    FeedStarted {
        feed: u32,
        portions: u8,
    },
    FeedRefused {
        refusal: FeedRefusal,
    },
    /// No answer came: the feed may have run. It is not sent again; its
    /// `Fed` event or the history settles it.
    FeedUnanswered,
    FeedEnded {
        result: FeedResult,
    },
    /// A stop was refused (`refusal`) or never answered (`None`): the feed
    /// may still run to its end.
    CancelUnconfirmed {
        refusal: Option<FeedRefusal>,
    },
    /// The feeder took this phone's clock.
    ClockSynced,
    /// The feeder refused this phone's clock, or never answered. The next
    /// session tries again.
    ClockNotSynced,
    /// The session ended; `error` says why when it was not asked to.
    Closed {
        error: Option<BridgeError>,
    },
}

/// What [`FeederSession::run`] starts from, taken once.
struct Start {
    keys: Keypair,
    feeder: [u8; 32],
    address: SocketAddr,
    commands: mpsc::UnboundedReceiver<Command>,
    sync_clock: bool,
}

#[frb(opaque)]
pub struct FeederSession {
    commands: mpsc::UnboundedSender<Command>,
    start: Mutex<Option<Start>>,
}

impl FeederSession {
    /// A session with `feeder` (its identity key) at `address` (`ip:port`,
    /// as NSD resolved it). With `sync_clock`, as after pairing, it gives
    /// the feeder this phone's clock and zone once it opens; it does that
    /// anyway when the feeder's clock is not set. Nothing connects until
    /// [`Self::run`].
    #[frb(sync)]
    pub fn new(
        key: &ClientKey,
        feeder: Vec<u8>,
        address: String,
        sync_clock: bool,
    ) -> Result<Self, BridgeError> {
        let feeder: [u8; 32] = feeder
            .try_into()
            .map_err(|_| BridgeError::new(ErrorKind::Key, "a feeder identity is 32 bytes"))?;
        let address = wellfed_connect::parse_address(&address)?;
        let (commands, receiver) = mpsc::unbounded_channel();
        Ok(Self {
            commands,
            start: Mutex::new(Some(Start {
                keys: key.keypair().clone(),
                feeder,
                address,
                commands: receiver,
                sync_clock,
            })),
        })
    }

    /// Connect and serve until the session ends: `updates` hears `Opened`,
    /// then the card, and `Closed` last. Runs once.
    pub async fn run(&self, updates: StreamSink<SessionUpdate>) {
        let start = self.start.lock().ok().and_then(|mut start| start.take());
        let Some(start) = start else {
            let error = BridgeError::new(ErrorKind::Ended, "this session already ran");
            // Nobody listening means nobody to tell.
            let _ = updates.add(SessionUpdate::Closed { error: Some(error) });
            return;
        };
        session::run(
            start.keys,
            start.feeder,
            start.address,
            start.sync_clock,
            start.commands,
            |update| updates.add(update).is_ok(),
        )
        .await;
    }

    /// Feed `portions` (1-48) once. The answer and the feed's end arrive
    /// as updates.
    #[frb(sync)]
    pub fn feed(&self, portions: u8) -> Result<(), BridgeError> {
        self.send(Command::Feed { portions })
    }

    #[frb(sync)]
    pub fn cancel_feed(&self, feed: u32) -> Result<(), BridgeError> {
        self.send(Command::CancelFeed { feed })
    }

    /// Read the status again.
    #[frb(sync)]
    pub fn refresh(&self) -> Result<(), BridgeError> {
        self.send(Command::Refresh)
    }

    #[frb(sync)]
    pub fn close(&self) {
        // A session that already ended has nothing left to close.
        let _ = self.commands.send(Command::Close);
    }

    fn send(&self, command: Command) -> Result<(), BridgeError> {
        self.commands
            .send(command)
            .map_err(|_| BridgeError::new(ErrorKind::Ended, "the session has ended"))
    }
}
