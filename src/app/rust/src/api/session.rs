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
use crate::api::feeder::{FeederReply, FeederRequest, PlansView, TimeInfo};
use crate::api::keys::ClientKey;
use crate::api::model::{
    Blocked, ClientRole, Desiccant, FeedOutcome, FeedReason, FoodLevel, MissedReason,
};
use crate::frb_generated::StreamSink;
use crate::requests;
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
    /// The last feed this boot, when it ended on a fault, until a later feed
    /// completes.
    pub fault: Option<FaultView>,
    pub desiccant: Desiccant,
    pub uptime_ms: u32,
    /// The flash bank it runs from.
    pub bank: u8,
    /// The plans' next meal, by the feeder's clock; `None` with no enabled
    /// plan, no clock, or before the plans are read.
    pub next_meal: Option<NextMealView>,
    /// The newest feed the history has given so far.
    pub last_fed: Option<FeedResult>,
    /// Today's meals, earliest first.
    pub meals: Vec<MealView>,
    /// The time settings and the clock, once read.
    pub time: Option<TimeInfo>,
    /// The feeding plans, once read.
    pub plans: Option<PlansView>,
    /// The base of a Wi-Fi change.
    pub wifi_generation: u32,
}

/// The feed `Status.feed_fault` names, with its `Fed` event once read.
pub struct FaultView {
    pub feed: u32,
    pub outcome: FeedOutcome,
    pub reason: FeedReason,
    pub result: Option<FeedResult>,
}

pub struct NextMealView {
    pub at_ms: i64,
    /// The minute of the feeder's local day.
    pub minute: u16,
    pub portions: u8,
}

/// One enabled plan that runs today.
pub struct MealView {
    pub slot: u8,
    pub minute: u16,
    pub portions: u8,
    /// Plays of the meal call before it.
    pub call: u8,
    pub standing: MealStanding,
}

pub enum MealStanding {
    Fed {
        result: FeedResult,
    },
    /// Its time passed with no feed recorded for it.
    Passed,
    /// The next meal of all the plans.
    Next {
        at_ms: i64,
    },
    Later,
}

/// Who asked for a feed or a change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FedBy {
    Panel,
    /// A client, by its key's first 4 bytes in hex.
    Client {
        fingerprint: String,
    },
    Feeder,
    Plan {
        slot: u8,
    },
    /// A feed a reset interrupted, whose requester the journal lost.
    Unknown,
}

/// One entry of the feeder's history.
pub struct HistoryEntry {
    pub seq: u64,
    pub at_ms: Option<i64>,
    pub body: HistoryBody,
}

pub enum HistoryBody {
    Fed {
        result: FeedResult,
    },
    Missed {
        slot: u8,
        due_ms: i64,
        reason: MissedReason,
    },
    Restarted,
    Lock {
        locked: bool,
        by: FedBy,
    },
    SettingsChanged {
        by: FedBy,
    },
    WifiChanged {
        by: FedBy,
    },
    PlansChanged {
        by: FedBy,
    },
    Joined {
        fingerprint: String,
    },
    Revoked {
        fingerprint: String,
        by: FedBy,
    },
    ClockSet {
        by: FedBy,
    },
    Lost {
        count: u16,
    },
    FeedingBlocked {
        reason: Option<Blocked>,
    },
    MealSoon {
        minute: u16,
        portions: u8,
    },
    Supply {
        adapter: Option<bool>,
    },
    DispenseRefused {
        reason: Blocked,
    },
    Food {
        level: FoodLevel,
    },
    /// Events that left the ring before they were read.
    Gap {
        from: u64,
        to: u64,
    },
    /// Nothing the history shows.
    Other,
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
    /// When the feeder recorded it; `None` while its clock was invalid.
    pub at_ms: Option<i64>,
    pub by: FedBy,
    /// What the food beam read during the feed.
    pub hopper: FoodLevel,
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

#[allow(
    clippy::large_enum_variant,
    reason = "each message is encoded for Dart at once, and never held"
)]
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
    /// More of the history, oldest first. A session reads it from the oldest
    /// event the feeder keeps, so `Opened` starts it anew; `caught_up` is
    /// false only ever before the first batch, and marks that batch's end.
    History {
        entries: Vec<HistoryEntry>,
        caught_up: bool,
    },
    /// Record lines after a `TailLog`.
    Log {
        seq: u32,
        dropped: u32,
        lines: Vec<String>,
    },
    /// The session ended; `error` says why when it was not asked to.
    Closed {
        error: Option<BridgeError>,
    },
}

/// What [`FeederSession::run`] starts from, taken once.
struct Start {
    keys: Keypair,
    address: SocketAddr,
    commands: mpsc::UnboundedReceiver<Command>,
    sync_clock: bool,
}

#[frb(opaque)]
pub struct FeederSession {
    feeder: [u8; 32],
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
            feeder,
            commands,
            start: Mutex::new(Some(Start {
                keys: key.keypair().clone(),
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
            self.feeder,
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

    /// Run one request. A write goes out once; see [`FeederReply`] for how it
    /// can end short of an answer.
    pub async fn ask(&self, request: FeederRequest) -> FeederReply {
        requests::run(&self.commands, self.feeder, request).await
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
