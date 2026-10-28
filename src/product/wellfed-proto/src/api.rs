//! Device API: one Noise session per client over TCP.
//!
//! A connection starts with one clear [`Open`] frame naming the session:
//! - [`Open::Session`]: an enrolled client, Noise IK under [`PROLOGUE`]. The
//!   client knows the feeder's identity key and sends its own encrypted in
//!   the first message, carrying a [`Hello`]; the feeder answers with a
//!   [`Welcome`] (or refuses an unknown key, an old major version or a full
//!   table) in the second.
//! - [`Open::Invite`]: a new client redeems an invite, Noise XXpsk3 with the
//!   invite's secret as the PSK. The third message carries a [`Join`] and
//!   the feeder's first transport message a [`Joined`]; the client then
//!   reconnects with [`Open::Session`].
//! - [`Open::Recovery`]: the button-hold window. The connection runs BLE setup's
//!   setup exchange ([`crate::setup`]) unchanged, code proof included, so a
//!   LAN neighbor cannot slip in while the window is open.
//!
//! Every frame is a u16 little-endian length and that many bytes
//! (`wellfed_session::frame`), at most [`FRAME_MAX`]. After the handshake each frame
//! is one Noise transport message sealing one [`ClientMessage`] or
//! [`FeederMessage`].
//!
//! Evolution ([device API](../../../../docs/firmware/device-api.md#evolution)):
//! - [`PROLOGUE`] never changes. A new major is negotiated in
//!   [`Hello::major`], and a feeder refuses one it does not serve with
//!   [`OpenRefusal::Version`].
//! - Within a major, a new op, event body, outcome or refusal is a variant
//!   appended at the end, gated by a [`Capabilities`] bit both sides claim
//!   ([`Op::capability`], [`EventBody::capability`]).
//! - An existing struct or variant never gains, loses, reorders or retypes
//!   a field: postcard is positional, so an old peer would misread every
//!   byte after it. New data is a new variant behind a bit.
//! - While the research unit is the only feeder running Wellfed, a message
//!   may still change in place when that is simpler: the firmware and every
//!   client are built from this repository together. The change rewrites
//!   the message's fixture lines by hand (blessing only appends).
//!
//! The byte fixtures in `tests/fixtures/v1.txt` pin every v1 message, and
//! their test refuses to re-bless a line that changed.

use serde::{Deserialize, Serialize};

pub use crate::setup::Wifi;

/// Bound into the IK and invite handshakes, so a session meant for another
/// protocol or version fails at its first encrypted message.
pub const PROLOGUE: &[u8] = b"wellfed-api-v1";

pub const PROTOCOL_MAJOR: u16 = 1;
/// The oldest major this feeder still serves.
pub const PROTOCOL_MAJOR_MIN: u16 = 1;

/// The largest frame either way, Noise tag included.
pub const FRAME_MAX: usize = 1024;
/// The largest message a frame can seal.
pub const MESSAGE_MAX: usize = FRAME_MAX - 16;

/// The mDNS service the feeder advertises.
pub const SERVICE: &str = "_wellfed._tcp";
/// The TCP port the feeder listens on.
pub const PORT: u16 = 4747;

/// A client-chosen request id: random, so a retry after a reconnect is
/// still recognized.
pub type RequestId = [u8; 16];

/// The first frame on a connection, in clear.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Open {
    Session,
    Invite { id: u32 },
    Recovery,
}

/// Feature bits within a major version.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities(pub u32);

impl Capabilities {
    /// Everything major version 1 first defined.
    pub const V1: Self = Self(0);
    /// Feeding: [`Op::Feed`] is served, and [`Op::CancelFeed`],
    /// [`Outcome::Feeding`] and [`EventBody::Fed`] are in use. A feeder
    /// pushes `Fed` events only to a session whose hello carries this bit.
    pub const FEED: Self = Self(1);
    /// Updates over the device API: [`Op::OfferUpdate`], the chunks and
    /// the update's state, cancelling it, and the update keys
    /// ([`Op::EnrollUpdateKey`], [`Op::UpdateKeyStatement`]), with
    /// [`Outcome::Update`] and [`Refusal::Update`]. Owner sessions only.
    pub const UPDATE: Self = Self(2);
    /// Feeding readiness: [`Op::Status`] is answered
    /// [`Outcome::FeederStatus`], which says whether a feed can start now
    /// and why not, and [`EventBody::FeedingBlocked`] records each change
    /// of a lasting reason.
    pub const READINESS: Self = Self(4);
    /// The time settings and the clock's state: [`Op::GetTime`],
    /// [`Op::PutTime`] and [`Outcome::Time`].
    pub const TIME: Self = Self(8);
    /// The debug log: [`Op::TailLog`] and the [`FeederMessage::Log`]
    /// batches it starts. Owner sessions only. The lines are diagnostics,
    /// not a contract: their text may change with any build.
    pub const LOG: Self = Self(32);
    /// Statistics: [`Op::Stats`] and [`Outcome::Stats`], and the live
    /// [`Metric`]s beyond power and signal, which a feeder sends only to a
    /// subscriber whose hello carries this bit.
    pub const STATS: Self = Self(16);
    /// Per-feed traces: [`Op::FeedTrace`] and [`Outcome::FeedTrace`], and
    /// the statistics' [`StatsSection::Feeds`]. Served only beside
    /// [`Self::STATS`] (both bits are needed): a feeder that serves the
    /// statistics but predates traces could not decode the op.
    pub const TRACES: Self = Self(64);
    /// Portion calibration: [`Op::GetCalibration`] and
    /// [`Op::PutCalibration`] with [`Outcome::Calibration`] and
    /// [`Refusal::Unverified`], and the [`StatsSection::Portions`]
    /// statistics.
    pub const CALIBRATION: Self = Self(256);
    /// The supply, judged: [`Metric::Supply`] to subscribers, the
    /// [`EventBody::Supply`] history of its changes, and readiness's
    /// [`Blocked::Battery`]. A session without the bit sees that reason as
    /// [`Blocked::Starting`] ([`Blocked::seen_by`]).
    pub const POWER: Self = Self(128);
    /// The stock backup: [`Op::ReadBank1`] and [`Outcome::Bank1`], and
    /// stock's stored plans, [`Op::ReadStockPlans`] and
    /// [`Outcome::StockPlans`]. Owner sessions only, and served only by the
    /// bank-0 recovery role, which can read bank 1 while it holds the
    /// feeder's only copy of stock.
    pub const BACKUP: Self = Self(512);
    /// Feeding plans: [`Op::GetPlans`] and [`Op::PutPlans`] with
    /// [`Outcome::Plans`], and the [`EventBody::Plans`],
    /// [`EventBody::MealSoon`] and [`EventBody::Missed`] history.
    pub const PLANS: Self = Self(1024);
    /// The desiccant reminder's reset: [`Op::ResetDesiccant`] and
    /// [`Refusal::NoClock`]. The period ([`Settings::desiccant_days`]) and
    /// the count ([`Status::desiccant`]) are part of those messages.
    pub const DESICCANT: Self = Self(2048);
    /// Every bit this crate defines: what a current client claims. A
    /// session then runs on the bits both sides claim, so a client adapts
    /// to whatever feeder it meets, however old.
    pub const KNOWN: Self = Self(
        Self::FEED.0
            | Self::UPDATE.0
            | Self::READINESS.0
            | Self::TIME.0
            | Self::STATS.0
            | Self::LOG.0
            | Self::TRACES.0
            | Self::CALIBRATION.0
            | Self::POWER.0
            | Self::BACKUP.0
            | Self::PLANS.0
            | Self::DESICCANT.0,
    );

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// The client's IK first-message payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub major: u16,
    pub capabilities: Capabilities,
}

/// The feeder's IK second-message payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Welcome<'a> {
    Open {
        major: u16,
        capabilities: Capabilities,
        /// The running image's build identity.
        build: &'a str,
        /// The role this key holds.
        role: Role,
        /// The event history's epoch: drawn at random when the ring is
        /// created and kept with it. Event numbers count within one epoch,
        /// so a cursor from another epoch means nothing here: the client
        /// starts again from the oldest event kept.
        epoch: u32,
    },
    Refused(OpenRefusal),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpenRefusal {
    /// The key is not enrolled (or was revoked).
    Unknown,
    /// The client's major version is one this feeder no longer serves, or
    /// does not know yet; `supported` is the newest it serves.
    Version { supported: u16 },
    /// Every session slot is taken.
    Full,
}

/// A client's role, as the client store keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Owner,
    Member,
    Server,
    UpdateSigner,
}

/// An invite redemption's third-message payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Join<'a> {
    /// A name for the client, shown when the owner lists clients (1-16
    /// bytes).
    pub label: &'a str,
}

/// The feeder's answer to a [`Join`], sealed; the connection then closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Joined {
    Enrolled { role: Role },
    Refused(Refusal),
}

/// What a client sends once the session is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMessage<'a> {
    Request {
        id: RequestId,
        #[serde(borrow)]
        op: Op<'a>,
    },
    /// Push every durable event after `after` (0 for the oldest kept), then
    /// each new one as it happens, and the live samples if `samples`.
    Subscribe { after: Option<u64>, samples: bool },
}

/// One operation. Reads may be repeated freely; a write's id makes a retry
/// return the first answer instead of running again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op<'a> {
    Status,
    GetSettings,
    /// Applies only if the settings are still at `base` generation.
    PutSettings {
        base: u32,
        changes: SettingsChanges,
    },
    /// Stores credentials for the next join, or clears them with `None`.
    /// Applies only if the Wi-Fi store is still at `base` generation.
    PutWifi {
        base: u32,
        #[serde(borrow)]
        wifi: Option<Wifi<'a>>,
    },
    SetLock {
        locked: bool,
    },
    Play {
        tune: Tune,
    },
    TimeSync(TimeSync),
    /// A single-use invite for a new client, held in RAM for 10 minutes.
    CreateInvite {
        role: Role,
    },
    ListClients,
    RevokeClient {
        key: [u8; 32],
    },
    /// Answered [`Outcome::Feeding`] once the feed is durably reserved; its
    /// result is a [`EventBody::Fed`] in the history, with the portions
    /// asked for and those delivered. `portions` outside 1 to
    /// [`PORTIONS_MAX`] is refused [`Refusal::Portions`] before anything
    /// else is checked. Refused `Unavailable` by a feeder without
    /// [`Capabilities::FEED`].
    Feed {
        portions: u8,
    },
    Reboot,
    /// Stop the feed [`Outcome::Feeding`] named, if it is still running.
    /// `NotFound` once it has ended (its `Fed` event says how).
    CancelFeed {
        feed: u32,
    },
    /// Offer a signed version 2 manifest, with the downgrade statement it
    /// needs when its version is not above the floor (always, for stock).
    /// Bank 1 checks it, records it and gives way to bank 0
    /// ([`UpdatePhase::GivingWay`]): the client reconnects and offers it
    /// again, and bank 0, which installs only that manifest, starts
    /// receiving. Refused `Busy` while a feed runs.
    OfferUpdate {
        #[serde(borrow)]
        manifest: &'a [u8],
        #[serde(borrow)]
        downgrade: Option<&'a [u8]>,
    },
    /// Image bytes at `offset`, one of the offsets [`UpdateView::wanted`]
    /// names, at most [`UPDATE_CHUNK_MAX`] bytes. Idempotent by offset: a
    /// chunk already written is answered with the state and changes
    /// nothing, so a retry needs no remembered id.
    UpdateChunk {
        offset: u32,
        #[serde(borrow)]
        data: &'a [u8],
    },
    /// The update's state, for a client that reconnected.
    UpdateState,
    /// Stop the install with bank 1 left invalid, and forget the
    /// candidate.
    CancelUpdate,
    /// Enroll an owner's Ed25519 update-signing key. Answered once a
    /// Dispense press at the feeder confirms it, or refused
    /// `Update(Unconfirmed)` when nobody presses within the window.
    EnrollUpdateKey {
        key: [u8; 32],
        #[serde(borrow)]
        label: &'a str,
    },
    /// A key statement a built-in or the release key signed: fill the
    /// release slot, or retire a key id for good.
    UpdateKeyStatement {
        #[serde(borrow)]
        statement: &'a [u8],
    },
    /// The time settings and the clock's state.
    GetTime,
    /// Applies only if the time settings are still at `base` generation.
    /// Owner sessions only: the NTP server decides which fixes the clock
    /// hears.
    PutTime {
        base: u32,
        #[serde(borrow)]
        changes: TimeChanges<'a>,
    },
    /// Push the feeder's record lines after line `after` (`None`: from
    /// the oldest its RAM tail keeps), then each new one, as
    /// [`FeederMessage::Log`] batches. Answered `Done`; asking again moves
    /// the cursor. Lines count from 1 each boot, so a cursor past the
    /// newest line (an earlier boot's) starts from the oldest.
    TailLog {
        after: Option<u32>,
    },
    /// One section of the feeder's statistics, answered
    /// [`Outcome::Stats`]. Each section fits one frame on its own.
    Stats {
        section: StatsSection,
    },
    /// The trace of feed `feed` (the number [`Outcome::Feeding`] gave it),
    /// from byte `offset` of its body: answered [`Outcome::FeedTrace`] with
    /// at most [`PAGE_MAX`](crate::trace::PAGE_MAX) bytes, so a client asks
    /// again from where each page ends until it has the whole body.
    /// `NotFound` when the feeder keeps no trace of it, `Busy` while it
    /// still runs, `Invalid` for an offset past the body's end.
    FeedTrace {
        feed: u32,
        offset: u16,
    },
    /// The portion calibration and where the rotor rests, answered
    /// [`Outcome::Calibration`].
    GetCalibration,
    /// Store `calibration` if the calibration is still at `base`
    /// generation, answered `Done` once the store holds it. Owner sessions
    /// only. A bowl mode this feeder has not verified is refused
    /// [`Refusal::Unverified`], and a portion weight outside
    /// [`PORTION_MG_MIN`]..=[`PORTION_MG_MAX`] `Invalid`.
    PutCalibration {
        base: u32,
        calibration: Calibration,
    },
    /// Bank 1's bytes from `offset`, answered [`Outcome::Bank1`] with at
    /// most [`BANK1_PAGE_MAX`] bytes, so a client asks again from where
    /// each page ends until it has the whole bank. Read-only and owner
    /// sessions only. Served only from bank 0 (`Unavailable` from bank 1,
    /// whose own image is the running build), `Busy` while an install
    /// holds the flash, `Invalid` for an offset at or past the bank's end.
    /// The bytes are as the flash holds them: after stock installed a
    /// build into bank 0, stock with its marker's first byte zeroed.
    ReadBank1 {
        offset: u32,
    },
    /// The feeding plans, answered [`Outcome::Plans`].
    GetPlans,
    /// Replace every plan slot with `plans` if the plans are still at
    /// `base` generation, answered `Done` once the store holds them. A
    /// plan asking for portions outside 1 to [`PORTIONS_MAX`] is refused
    /// [`Refusal::Portions`] (the first such slot's), one with another
    /// field outside [`Plan`]'s limits `Invalid`. A
    /// plan whose minute or days changed (or a new one) runs only from
    /// its next occurrence after the save.
    PutPlans {
        base: u32,
        plans: [Option<Plan>; PLANS_MAX],
    },
    /// Record that the desiccant was replaced today, by the feeder's local
    /// calendar, answered `Done` with the settings' generation once the
    /// store holds it. Refused [`Refusal::NoClock`] while the clock is
    /// invalid: the feeder never records a date it does not know.
    ResetDesiccant,
    /// Stock's two plan sectors as the flash holds them, answered
    /// [`Outcome::StockPlans`]: what the installer imports before a
    /// promotion. Read-only, owner sessions only, and served only where
    /// [`Op::ReadBank1`] is (the bank-0 role, `Busy` while an install holds
    /// the flash).
    ReadStockPlans,
}

/// The entries [`Outcome::StockPlans`] carries: stock's ten 44-byte
/// entries, everything its boot can read.
pub const STOCK_PLAN_ENTRIES_LEN: usize = 440;

/// The most bank-1 bytes one [`Outcome::Bank1`] carries: well within a
/// frame beside the response's id and numbers, and a whole number of pages
/// in the bank (1,792 of them).
pub const BANK1_PAGE_MAX: usize = 768;

/// The lightest portion a calibration may store: well under any dry-food
/// compartment's load.
pub const PORTION_MG_MIN: u32 = 1_000;
/// The heaviest: well over what one compartment of the PLAF103's rotor holds.
pub const PORTION_MG_MAX: u32 = 50_000;

/// What the owner measured, as the feeder keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Calibration {
    /// The mean weight of one portion, `None` until measured.
    pub portion_mg: Option<u32>,
    /// The bowl mode the feeder runs.
    pub bowl: Bowl,
}

/// A single bowl turns one way and drops every compartment into one bowl;
/// a double bowl alternates directions between two.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Bowl {
    Single,
    Double,
}

/// Where the rotor rests between feeds, as the feeding owner knows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    /// Just past a mark, where a completed feed stops.
    AtMark,
    /// Anywhere between two marks: the next feed homes it first.
    Unknown,
}

/// [`Op::GetCalibration`]'s answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalibrationView {
    pub generation: u32,
    pub writable: bool,
    pub calibration: Calibration,
    /// `None` from a feeder with no feeding owner, or before it opens.
    pub phase: Option<Phase>,
}

/// Plan slots a feeder keeps, as stock keeps at most ten plans.
pub const PLANS_MAX: usize = 10;
/// The most portions one feed asks for, whoever asks (a client, a plan or
/// Dispense): the vendor app's cap on this model. Every feed asks for 1 to
/// this many; anything else is refused [`Refusal::Portions`].
pub const PORTIONS_MAX: u8 = 48;

/// One feeding plan: `portions` at minute `minute` of the local day
/// (0-1439), on the weekdays `days` lists (bit 0 Monday to bit 6 Sunday),
/// or once at its next occurrence when it lists none. A plan that ran once
/// stays in its slot, disabled. With no valid clock, plans hold; an
/// occurrence more than 15 minutes late never runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub minute: u16,
    /// 1 to [`PORTIONS_MAX`].
    pub portions: u8,
    pub days: u8,
    pub enabled: bool,
}

/// [`Op::GetPlans`]'s answer: the plan in each slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plans {
    pub generation: u32,
    pub writable: bool,
    pub plans: [Option<Plan>; PLANS_MAX],
}

/// The largest [`Op::UpdateChunk`]: an eighth of a flash sector, which
/// a sealed request carries with room to spare.
pub const UPDATE_CHUNK_MAX: usize = 512;
/// The chunk offsets one [`UpdateView`] names: a sector's worth.
pub const WANTED_MAX: usize = 8;

impl Op<'_> {
    /// Whether the op changes something, so its id is remembered. A chunk
    /// is idempotent by its offset instead, so a whole image does not push
    /// every other write out of the feeder's memory.
    pub const fn writes(&self) -> bool {
        !matches!(
            self,
            Self::Status
                | Self::GetSettings
                | Self::ListClients
                | Self::UpdateState
                | Self::UpdateChunk { .. }
                | Self::GetTime
                | Self::TailLog { .. }
                | Self::Stats { .. }
                | Self::FeedTrace { .. }
                | Self::GetCalibration
                | Self::ReadBank1 { .. }
                | Self::GetPlans
                | Self::ReadStockPlans
        )
    }

    /// The bits both sides must claim before the op is served. The match
    /// has no wildcard, so a new op cannot land without its decision.
    pub const fn capability(&self) -> Capabilities {
        match self {
            Self::Stats { section } => section.capability(),
            Self::FeedTrace { .. } => Capabilities::STATS.union(Capabilities::TRACES),
            Self::Feed { .. } | Self::CancelFeed { .. } => Capabilities::FEED,
            Self::OfferUpdate { .. }
            | Self::UpdateChunk { .. }
            | Self::UpdateState
            | Self::CancelUpdate
            | Self::EnrollUpdateKey { .. }
            | Self::UpdateKeyStatement { .. } => Capabilities::UPDATE,
            Self::GetTime | Self::PutTime { .. } => Capabilities::TIME,
            Self::TailLog { .. } => Capabilities::LOG,
            Self::GetCalibration | Self::PutCalibration { .. } => Capabilities::CALIBRATION,
            Self::ReadBank1 { .. } | Self::ReadStockPlans => Capabilities::BACKUP,
            Self::GetPlans | Self::PutPlans { .. } => Capabilities::PLANS,
            Self::ResetDesiccant => Capabilities::DESICCANT,
            Self::Status
            | Self::GetSettings
            | Self::PutSettings { .. }
            | Self::PutWifi { .. }
            | Self::SetLock { .. }
            | Self::Play { .. }
            | Self::TimeSync(_)
            | Self::CreateInvite { .. }
            | Self::ListClients
            | Self::RevokeClient { .. }
            | Self::Reboot => Capabilities::V1,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsChanges {
    pub lights: Option<bool>,
    pub sound: Option<bool>,
    /// 0-100.
    pub volume: Option<u8>,
    pub lights_window: Option<Window>,
    pub sound_window: Option<Window>,
    /// [`DESICCANT_DAYS_MIN`] to [`DESICCANT_DAYS_MAX`].
    pub desiccant_days: Option<u8>,
    pub meal_reminder: Option<MealReminder>,
}

/// Whether the feeder sends a [`EventBody::MealSoon`] ahead of each
/// scheduled meal, and how many minutes ([`MEAL_REMINDER_MINUTES_MIN`] to
/// [`MEAL_REMINDER_MINUTES_MAX`]) ahead.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MealReminder {
    #[default]
    Off,
    Before {
        minutes: u8,
    },
}

/// The shortest meal reminder lead, in minutes.
pub const MEAL_REMINDER_MINUTES_MIN: u8 = 1;
/// The longest meal reminder lead, in minutes.
pub const MEAL_REMINDER_MINUTES_MAX: u8 = 60;

/// The shortest desiccant reminder period, in days.
pub const DESICCANT_DAYS_MIN: u8 = 1;
/// The longest desiccant reminder period, in days.
pub const DESICCANT_DAYS_MAX: u8 = 90;
/// The period a feeder starts with: stock's cloud used 14 days.
pub const DESICCANT_DAYS_DEFAULT: u8 = 14;

/// When lights or sound that are on may show or play: always, or only from
/// minute `start` to minute `end` of the local day (0-1439, both included;
/// an end before the start wraps past midnight). With no valid clock every
/// window is open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Window {
    #[default]
    Always,
    Between {
        start: u16,
        end: u16,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tune {
    Chime,
    Meal,
    Coin,
    Alert,
    Test,
    Twinkle,
}

/// A change to the time settings; `None` leaves a field as it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeChanges<'a> {
    /// The owner's zone as a POSIX TZ rule (at most 48 bytes, the footer
    /// of the zone's TZif file), or `""` to clear it.
    #[serde(borrow)]
    pub zone: Option<&'a str>,
    pub ntp: Option<NtpServer>,
}

/// Where the feeder asks for the time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NtpServer {
    Off,
    /// The router that gave the feeder its address.
    Gateway {
        port: u16,
    },
    Address {
        ip: [u8; 4],
        port: u16,
    },
}

/// Who last set the clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeSource {
    /// An owner's session, which may step the clock freely.
    Owner,
    /// A member's or the server's session, which may only narrow it.
    Client,
    Ntp,
}

/// What the last NTP exchange came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NtpResult {
    /// The clock took the answer.
    Used,
    /// The answer disagreed with a valid clock beyond both error bounds.
    Bounded,
    /// No answer in time.
    Lost,
    /// An answer that could not be used (unsynchronized, kiss-o'-death,
    /// too loose or malformed).
    Refused,
}

/// The time settings and the clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeView<'a> {
    pub generation: u32,
    pub writable: bool,
    /// The zone's POSIX rule, `""` when none is set.
    pub zone: &'a str,
    pub ntp: NtpServer,
    /// Unix milliseconds; `None` while the clock is invalid: the feeder
    /// needs the time, and scheduled meals hold.
    pub clock_ms: Option<u64>,
    /// How far off the clock could be.
    pub error_ms: Option<u32>,
    /// East of UTC, when a zone or a client sync gave one.
    pub offset_s: Option<i32>,
    /// The uptime clock's learned correction, parts per billion.
    pub rate_ppb: i32,
    /// How far the true rate could be from it.
    pub rate_bound_ppb: u32,
    pub learned: bool,
    pub syncs: u32,
    pub last: Option<TimeSource>,
    /// Seconds since the last fix.
    pub last_age_s: Option<u32>,
    pub ntp_last: Option<NtpResult>,
}

/// The wall clock and the zone's next change, as the wall clock takes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeSync {
    pub unix_ms: u64,
    pub offset_s: i32,
    pub next_change_s: u64,
    pub next_offset_s: i32,
}

/// What the feeder sends once the session is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[expect(
    clippy::large_enum_variant,
    reason = "a wire message, built in place: the feeder cannot box"
)]
pub enum FeederMessage<'a> {
    Response {
        id: RequestId,
        #[serde(borrow)]
        result: Outcome<'a>,
    },
    Event(Event),
    /// Events `from..=to` left the ring before this client read them.
    Gap {
        from: u64,
        to: u64,
    },
    Sample(Sample),
    /// Record lines after [`Op::TailLog`]'s cursor, each ending in `\n`,
    /// as many whole lines as fit one frame (at most [`LOG_TEXT_MAX`]
    /// bytes). `seq` is the last line's number, the cursor to resume
    /// after; `dropped` counts the lines after the previous cursor that
    /// the batch does not hold, because the tail overwrote them before
    /// they were sent. Only to a session that asked, and only when it has
    /// sent everything before.
    Log {
        seq: u32,
        dropped: u32,
        text: &'a str,
    },
}

/// The most text one [`FeederMessage::Log`] carries: a frame, less the
/// message's tag, numbers and the text's length prefix.
pub const LOG_TEXT_MAX: usize = MESSAGE_MAX - 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome<'a> {
    /// A write took effect (or a read of nothing); for a settings or Wi-Fi
    /// write, `generation` is the store's after it.
    Done {
        generation: Option<u32>,
    },
    Status(Status<'a>),
    Settings(Settings),
    #[serde(borrow)]
    Clients(Clients<'a>),
    Invite(Invite),
    Refused(Refusal),
    /// The feed is reserved in the journal under `feed` and runs now; a
    /// retry of the request returns this again rather than feeding twice.
    Feeding {
        feed: u32,
    },
    /// Where the update stands, after an update op.
    Update(UpdateView),
    #[serde(borrow)]
    Time(TimeView<'a>),
    /// The section [`Op::Stats`] asked for.
    #[serde(borrow)]
    Stats(Stats<'a>),
    /// [`Op::Status`]'s answer to a session with [`Capabilities::READINESS`]:
    /// the status, and whether a feed can start now.
    FeederStatus {
        #[serde(borrow)]
        status: Status<'a>,
        readiness: Readiness,
        /// The supply as the feeder judges it, to a session that shares
        /// [`Capabilities::POWER`]; `None` otherwise.
        supply: Option<Supply>,
    },
    /// One page of the trace [`Op::FeedTrace`] asked for.
    #[serde(borrow)]
    FeedTrace(TracePage<'a>),
    /// The calibration, after [`Op::GetCalibration`].
    Calibration(CalibrationView),
    /// One page of bank 1, after [`Op::ReadBank1`].
    #[serde(borrow)]
    Bank1(Bank1Page<'a>),
    /// The plans, after [`Op::GetPlans`].
    Plans(Plans),
    /// Stock's plan sectors, after [`Op::ReadStockPlans`].
    #[serde(borrow)]
    StockPlans(StockPlanSectors<'a>),
}

/// One page of bank 1: `bytes` from `offset`, of the `total` the feeder
/// serves (the bank's whole extent).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bank1Page<'a> {
    pub offset: u32,
    pub total: u32,
    #[serde(borrow)]
    pub bytes: &'a [u8],
}

/// Stock's plan sectors as the flash holds them: the count byte and the
/// first [`STOCK_PLAN_ENTRIES_LEN`] bytes of the entries. The feeder judges
/// none of it; the installer decodes it as stock's boot does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StockPlanSectors<'a> {
    pub count: u8,
    #[serde(borrow)]
    pub entries: &'a [u8],
}

/// Whether the feeder can start a feed now. One decision, which both
/// [`Op::Status`] reports and [`Op::Feed`] obeys: a feed asked for while
/// [`Self::Blocked`] is refused with [`Blocked::refusal`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Readiness {
    Ready,
    Blocked(Blocked),
}

/// Why a feed cannot start now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Blocked {
    /// The feeding owner has not opened yet (it opens once the boot's
    /// storage is ready), or a newly installed build has not confirmed
    /// itself yet.
    Starting,
    /// The feed journal could not be opened: no feed this boot.
    Journal,
    /// The feeding owner stopped on a fault: no feed until the feeder
    /// restarts.
    Fault,
    /// A feed a reset interrupted is being resolved as uncertain.
    Resolving,
    /// A feed is queued or runs.
    Feeding,
    /// A Wi-Fi join runs or waits for the outputs.
    Joining,
    /// An install holds the flash.
    Updating,
    /// BLE setup mode runs, which ends in a restart.
    Setup,
    /// This firmware has no feeding owner.
    Unserved,
    /// The feeder runs on its batteries and they are below the floor a
    /// feed needs, or unread; or this boot followed a brownout and the
    /// adapter has not been seen since. It clears when the adapter returns
    /// or the pack recovers past the floor's hysteresis. Only to a session
    /// that claims [`Capabilities::POWER`].
    Battery,
}

impl Blocked {
    /// What a feed asked for now is refused with.
    pub const fn refusal(self) -> Refusal {
        match self {
            Self::Journal | Self::Fault => Refusal::Fault,
            Self::Unserved => Refusal::Unavailable,
            Self::Starting
            | Self::Resolving
            | Self::Feeding
            | Self::Joining
            | Self::Updating
            | Self::Setup
            | Self::Battery => Refusal::Busy,
        }
    }

    /// The reason as a session with `capabilities` can decode it: one
    /// without [`Capabilities::POWER`] gets [`Self::Battery`] as
    /// [`Self::Starting`], the reason that also refuses `Busy`, is not
    /// lasting, and says only that the feeder will feed again by itself.
    pub const fn seen_by(self, capabilities: Capabilities) -> Self {
        match self {
            Self::Battery if !capabilities.contains(Capabilities::POWER) => Self::Starting,
            other => other,
        }
    }

    /// Whether the reason lasts: it holds until a restart or an operator
    /// ends it, rather than clearing by itself within seconds or one feed.
    /// Only a lasting reason is recorded in the history
    /// ([`EventBody::FeedingBlocked`]), so joins and feeds do not fill it.
    /// [`Self::Unserved`] is the build's, never a change, so the status
    /// alone reports it.
    pub const fn lasting(self) -> bool {
        match self {
            Self::Journal | Self::Fault | Self::Updating | Self::Setup => true,
            // The supply's changes are their own events (`Supply`), rate
            // limited, so an adapter that comes and goes does not fill the
            // history twice over.
            Self::Starting
            | Self::Resolving
            | Self::Feeding
            | Self::Joining
            | Self::Unserved
            | Self::Battery => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Refusal {
    /// This client's role may not do that.
    Forbidden,
    /// The store moved past the request's `base`: re-read and decide again.
    Stale {
        current: u32,
    },
    /// Another change is still being written, or the stores are loading.
    Busy,
    /// This firmware does not serve the op yet.
    Unavailable,
    /// A field failed the feeder's checks.
    Invalid,
    /// The store cannot be written now.
    NotWritable,
    /// Every client slot (or invite) is taken.
    Full,
    NotFound,
    /// The `sound` setting is off.
    Muted,
    /// The clock refused the sync.
    Time,
    /// The feeding owner stopped on a fault and serves no feed until the
    /// feeder restarts.
    Fault,
    /// An update op was refused: why.
    Update(UpdateRefusal),
    /// A bowl mode this feeder has not verified on hardware.
    Unverified,
    /// A feed, or a plan's, asked for `asked` portions, outside 1 to
    /// [`PORTIONS_MAX`].
    Portions {
        asked: u8,
    },
    /// The clock is invalid, so the feeder cannot date the change: sync
    /// the time first.
    NoClock,
}

/// Where an update stands. The install itself does not depend on the
/// session: a client that lost its connection reconnects, reads
/// [`Op::UpdateState`] and sends what [`Self::wanted`] names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateView {
    pub phase: UpdatePhase,
    /// The bank this answer comes from.
    pub bank: u8,
    /// The highest confirmed version: an offer at or below it needs a
    /// downgrade statement.
    pub min_version: u32,
    /// The manifest being checked, installed or given way for, if any.
    pub digest: Option<[u8; 32]>,
    pub version: u32,
    pub image_len: u32,
    /// Bytes written and read back so far.
    pub written: u32,
    /// The chunk offsets wanted now, each [`UPDATE_CHUNK_MAX`] bytes (the
    /// last may be shorter); all `None` between sectors or outside an
    /// install.
    pub wanted: [Option<u32>; WANTED_MAX],
    /// Why the last install stopped before activation, if it did.
    pub stopped: Option<UpdateStop>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpdatePhase {
    /// No update in progress.
    Idle,
    /// The offer is being checked; ask [`Op::UpdateState`] again.
    Checking,
    /// Bank 1 recorded the offer and restarts into bank 0 once this
    /// answer is out: reconnect and offer the same manifest again.
    GivingWay,
    /// Bank 0 is writing bank 1: send the chunks [`UpdateView::wanted`]
    /// names.
    Receiving,
    /// Every byte verified and the marker written: the feeder restarts
    /// into the new build, which confirms itself once it is healthy.
    Activated,
}

/// Why an install stopped. Bank 1's marker was never written, so the
/// feeder keeps running bank 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpdateStop {
    /// No chunk came for a minute.
    Timeout,
    Cancelled,
    /// A chunk at an offset that was not wanted, or of the wrong length.
    Order,
    /// The image's first bytes are not the loader's marker.
    Marker,
    /// A sector did not read back as written.
    Write,
    /// The whole image's SHA-256 differs from the manifest's.
    Hash,
    /// The marker did not read back.
    Activate,
    /// The update store could not be written.
    Store,
}

/// Why an update op was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpdateRefusal {
    /// Not a version 2 manifest (a version 1 one, wrong length or magic),
    /// or an unknown image kind.
    Manifest,
    /// No trusted key has the manifest's (or a statement's) key id.
    Key,
    /// That key was retired.
    Revoked,
    Signature,
    /// Not for bank 1, or outside the reviewed size.
    Target,
    /// That build is already running.
    Same,
    /// The version is not above the floor (or the image is stock): a
    /// downgrade statement must come with it.
    Downgrade,
    /// The downgrade or key statement does not check out (another feeder,
    /// another manifest, an old floor, a key that may not sign it).
    Statement,
    /// Bank 0 installs only the manifest bank 1 gave way for; cancel
    /// that update first.
    Pending,
    /// This manifest was activated and failed its trial: sign another.
    Failed,
    /// Bank 0 does not hold a build that can install it, so bank 1 does
    /// not give way.
    Fallback,
    /// An install reset the feeder this power cycle: no new install until
    /// the next power cycle.
    Attempt,
    /// Another update is in progress.
    Installing,
    /// No install is receiving chunks.
    NotInstalling,
    /// The update store cannot be read or written.
    Store,
    /// Nobody pressed Dispense in time to confirm the key.
    Unconfirmed,
    /// The feeder runs on its batteries: no install starts until the
    /// adapter returns.
    Battery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status<'a> {
    pub build: &'a str,
    pub bank: u8,
    pub uptime_ms: u32,
    pub boot_count: u32,
    pub reset_flags: u32,
    pub faults: u32,
    pub rssi: i32,
    pub wifi: WifiSource,
    pub link_losses: u32,
    /// Losses the link check noticed (the DHCP lease ran out or the gateway
    /// stopped answering) rather than the association. The name is from the
    /// internet DNS check that check replaced, kept for the wire.
    pub dns_losses: u32,
    /// Unix milliseconds, `None` while the clock is invalid.
    pub clock_ms: Option<u64>,
    pub power: Option<Power>,
    pub locked: Option<bool>,
    pub settings_generation: u32,
    pub wifi_generation: u32,
    /// The newest event's sequence number, `None` before the first.
    pub last_event: Option<u64>,
    pub sessions: u8,
    /// The hopper as the food beam last read it.
    pub food: FoodLevel,
    /// The last feed this boot that ended on a fault (`Failed` or
    /// `Uncertain`), until a later feed completes: a jam, a blocked outlet
    /// or a sensor that still wants seeing to. Its `Fed` event has the rest.
    pub feed_fault: Option<FeedFault>,
    pub desiccant: Desiccant,
}

/// The desiccant reminder, counted in the feeder's local calendar days.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Desiccant {
    /// No replacement is recorded yet.
    Unset,
    /// The clock is invalid, so the days cannot be counted.
    Unknown,
    /// Days left of the period since the last replacement: due at 0, and
    /// counting on below zero after that.
    Remaining { days: i32 },
}

impl Desiccant {
    /// Whether the replacement is due; `None` while it cannot be counted.
    pub const fn due(self) -> Option<bool> {
        match self {
            Self::Remaining { days } => Some(days <= 0),
            Self::Unset | Self::Unknown => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WifiSource {
    Build,
    Stored,
    Fallback,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Power {
    pub adapter: bool,
    pub battery_mv: Option<u32>,
}

/// The supply as the feeder judges it ([`Capabilities::POWER`]): never an
/// unknown passed off as a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Supply {
    /// The adapter's detect pin; `None` with no fresh sample.
    pub adapter: Option<bool>,
    pub battery: Battery,
    /// Whether the feeding owner judges the pack below the floor a feed
    /// needs (with its hold and hysteresis); `None` before it has judged,
    /// or in a firmware without one. Judged on the adapter too, where it
    /// does not refuse feeds.
    pub below_floor: Option<bool>,
}

/// The battery pack in a [`Supply`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Battery {
    /// No pack fitted: the channel reads near zero.
    Absent,
    /// The pack's millivolts, and its charge (0–100 in steps of 10) on
    /// stock's curve of the battery channel.
    Reading { mv: u16, percent: u8 },
    /// A value no pack of this board can read (above fresh cells, or on
    /// the pack alone too low to be running), as read.
    Implausible { mv: u16 },
    /// Nothing sampled: the monitor has not run, its ADC failed, or the
    /// firmware does not sample the supply.
    Unread,
    /// The last sample is this old, past the monitor's limit.
    Stale { age_ms: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub generation: u32,
    pub lights: bool,
    pub sound: bool,
    pub volume: u8,
    pub lights_window: Window,
    pub sound_window: Window,
    /// The desiccant reminder's period in days.
    pub desiccant_days: u8,
    /// The local calendar day (days since 1970-01-01) the desiccant was
    /// last replaced; `None` before the first [`Op::ResetDesiccant`].
    pub desiccant_reset: Option<i32>,
    pub meal_reminder: MealReminder,
    pub writable: bool,
}

/// The enrolled clients, in store order.
pub const CLIENTS_MAX: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clients<'a> {
    #[serde(borrow)]
    pub clients: [Option<Client<'a>>; CLIENTS_MAX],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Client<'a> {
    pub key: [u8; 32],
    pub label: &'a str,
    pub role: Role,
    /// Whether this is the session asking.
    pub you: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invite {
    pub id: u32,
    /// The PSK the new client's handshake must hold. Show it once (QR or
    /// link); anyone holding it within `seconds` joins with the role.
    pub secret: [u8; 32],
    pub seconds: u16,
}

/// One durable record of the feeder's history.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Monotonic across reboots; a client's cursor.
    pub seq: u64,
    /// Unix seconds, `None` when the clock was invalid.
    pub at: Option<u32>,
    pub body: EventBody,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventBody {
    Boot {
        bank: u8,
        reset_flags: u32,
    },
    Lock {
        locked: bool,
        by: Source,
    },
    Settings {
        generation: u32,
        by: Source,
    },
    Wifi {
        generation: u32,
        by: Source,
    },
    Enrolled {
        client: Fingerprint,
        role: Role,
        path: Path,
    },
    Revoked {
        client: Fingerprint,
        by: Source,
    },
    ClockSync {
        by: Source,
        /// How far the clock moved, saturated to i32 (about 24 days).
        jump_ms: Option<i32>,
    },
    /// Events the feeder could not record (its queue overflowed).
    Lost {
        count: u16,
    },
    /// A feed ended. `completed` counts the portions the feedback sensor
    /// confirmed. `hopper` is what the food beam read during it: a feed
    /// runs whatever the hopper reads, as stock's does, so a misreading
    /// beam never skips a meal, and an empty one is recorded here.
    Fed {
        feed: u32,
        requested: u8,
        completed: u8,
        outcome: FeedOutcome,
        reason: FeedReason,
        hopper: FoodLevel,
        by: Source,
    },
    /// The lasting reason the feeder cannot feed changed ([`Blocked::lasting`]):
    /// `Some` names the new one, `None` says it can feed again.
    FeedingBlocked {
        reason: Option<Blocked>,
    },
    /// The adapter came or went, or the pack crossed its floor. At most
    /// one a minute: changes inside that minute are recorded together,
    /// as the supply stands at its end.
    Supply(Supply),
    /// A client replaced the plans; `generation` is the store's after it.
    Plans {
        generation: u32,
        by: Source,
    },
    /// The occurrence of plan `plan` due at `due` (Unix seconds) did not
    /// run, and never will.
    Missed {
        plan: u8,
        due: u32,
        reason: MissedReason,
    },
    /// Dispense was pressed on an unlocked panel while a feed could not
    /// start (`reason`), during a feed among them: it started nothing and
    /// is not queued.
    DispenseRefused {
        reason: Blocked,
    },
    /// The occurrence of plan `plan` due at `due` (Unix seconds), at
    /// minute `minute` of the local day, is expected to feed `portions`:
    /// sent the reminder's lead ahead of it. A restart inside the lead may
    /// send it again, with the same `plan` and `due`. It is never
    /// withdrawn: the meal may still be missed, or the plan disabled,
    /// deleted or moved, so the `Fed` or `Missed` for that `plan` and `due`
    /// (or the plans) say what happened.
    MealSoon {
        plan: u8,
        due: u32,
        minute: u16,
        portions: u8,
    },
    /// The food beam's level changed: at most once a feed, since the beam
    /// is read only while a feed's emitters are on. The first reading
    /// after a boot counts as a change.
    Food {
        level: FoodLevel,
    },
}

impl EventBody {
    /// The bit a session's hello must carry for the feeder to push this
    /// body; a session without it never sees the event (its cursor moves
    /// past it). No wildcard, as for [`Op::capability`].
    pub const fn capability(&self) -> Capabilities {
        match self {
            Self::Fed { .. } | Self::DispenseRefused { .. } | Self::Food { .. } => {
                Capabilities::FEED
            }
            Self::FeedingBlocked { .. } => Capabilities::READINESS,
            Self::Supply(_) => Capabilities::POWER,
            Self::Plans { .. } | Self::Missed { .. } | Self::MealSoon { .. } => Capabilities::PLANS,
            Self::Boot { .. }
            | Self::Lock { .. }
            | Self::Settings { .. }
            | Self::Wifi { .. }
            | Self::Enrolled { .. }
            | Self::Revoked { .. }
            | Self::ClockSync { .. }
            | Self::Lost { .. } => Capabilities::V1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeedOutcome {
    Done,
    Cancelled,
    /// It never started: the feeder could not get ready in time.
    Expired,
    Failed,
    /// The feeder stopped without recording a result: it may have
    /// dispensed anything up to `requested`. Never retried automatically.
    /// When a power cut lost the reservation itself, `requested` is the
    /// feeder's cap on one feed, not what was asked.
    Uncertain,
}

/// A feed that ended on a fault, as [`Status::feed_fault`] names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedFault {
    pub feed: u32,
    pub outcome: FeedOutcome,
    pub reason: FeedReason,
}

/// What the food beam across the hopper reads, filtered as stock filters
/// its `surplusGrain`: one reading with food shows food at once, and the
/// hopper reads empty only after a second of readings without (stock's 20
/// readings at its 50 ms poll). The beam is lit only during feeds, so the
/// level is the last feed's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FoodLevel {
    /// No qualified reading: no feed has lit the beam this boot, or this
    /// feed ended before the beam settled.
    Unread,
    Present,
    /// The beam read no food (at any time during a feed, for a `Fed`).
    Empty,
}

/// Why a feed ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeedReason {
    Done,
    Cancelled,
    /// The feeder's clock jumped.
    Clock,
    /// The feeder could not watch the motor often enough.
    ObservationGap,
    /// The whole feed ran past its limit.
    Timeout,
    /// No rotor mark for too long: the mark sensor lost, or a rotor stalled
    /// with no current to show it.
    NoFeedback,
    Power,
    /// A sensor could not be read: the ADC, the beams, or current at rest
    /// over the jam limit.
    Sensor,
    /// The outlet beam read blocked. Mid-feed, the feed ends with the
    /// portions delivered so far and is never resumed (stock pauses up to
    /// about 10 minutes instead); at the start it ends with no motion.
    OutletBlocked,
    /// One current reading over the ceiling: stopped at once, no reversal.
    Overcurrent,
    /// The feedback sensor's edges could not be trusted.
    FeedbackStream,
    /// An output failed to apply or verify.
    Output,
    /// A jam no reversal freed: the motor backed off three times, as
    /// stock's does (its code 2031), or the jam held while backing off.
    /// The feed ends, never retried; stock counts it done.
    Jammed,
    /// The mark sensor read a mark for longer than any mark lasts.
    MarkStuck,
}

/// The first 4 bytes of a client's key: enough to tell clients apart in
/// the history, which has 16 bytes for a body.
pub type Fingerprint = [u8; 4];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    Panel,
    Client(Fingerprint),
    Feeder,
    /// The plan in this slot.
    Schedule(u8),
}

/// Why a plan's occurrence did not run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MissedReason {
    /// It fell while the feeder was off or had no valid time, or the clock
    /// moved past it, more than the grace (15 minutes) ago.
    Late,
    /// It waited past the grace for a feed it could not start: why.
    Blocked(Blocked),
    /// The feeder stopped after it took the occurrence and before the
    /// feed was reserved.
    Interrupted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Path {
    Setup,
    Invite,
    Recovery,
}

/// A live measurement, sent only to subscribers and never stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sample {
    /// Feeder uptime.
    pub at_ms: u32,
    pub metric: Metric,
}

/// One live value. A feeder sends each kind on its own cadence, and one
/// not sent before the next of its kind is replaced by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Metric {
    Power(Power),
    Rssi(i32),
    /// The heap's free bytes now, and the fewest since boot.
    Heap {
        free: u32,
        min: u32,
    },
    /// The loop task's passes since the last loop sample: how many, the
    /// longest and the 99th percentile (an upper bound, within an eighth).
    Loop {
        passes: u32,
        max_us: u32,
        p99_us: u32,
    },
    /// The gateway probe's round trip since the last gateway sample, the
    /// latest that was answered; `None` when none was.
    Gateway {
        round_trip_ms: Option<u32>,
    },
    /// The joined access point's channel (`None` while not joined), and how
    /// often this boot a join landed on another access point than the join
    /// before it.
    Wifi {
        channel: Option<u8>,
        ap_changes: u32,
    },
    /// The supply, judged, each time it is sampled.
    Supply(Supply),
}

impl Metric {
    /// The bit a session's hello must carry for the feeder to send this
    /// metric; no wildcard, as for [`Op::capability`].
    pub const fn capability(&self) -> Capabilities {
        match self {
            Self::Power(_) | Self::Rssi(_) => Capabilities::V1,
            Self::Heap { .. } | Self::Loop { .. } | Self::Gateway { .. } | Self::Wifi { .. } => {
                Capabilities::STATS
            }
            Self::Supply(_) => Capabilities::POWER,
        }
    }
}

/// What [`Op::Stats`] asks for. A section added later needs a bit of its
/// own, since a feeder that predates it cannot decode the request: its
/// [`Self::capability`] names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatsSection {
    Counters,
    Memory,
    Flash,
    Power,
    Food,
    /// This boot's feeds as their traces measured them.
    Feeds,
    Portions,
}

impl StatsSection {
    pub const ALL: [Self; 7] = [
        Self::Counters,
        Self::Memory,
        Self::Flash,
        Self::Power,
        Self::Food,
        Self::Feeds,
        Self::Portions,
    ];

    /// The bits both sides must claim before this section is served.
    pub const fn capability(self) -> Capabilities {
        match self {
            Self::Counters | Self::Memory | Self::Flash | Self::Power | Self::Food => {
                Capabilities::STATS
            }
            Self::Feeds => Capabilities::STATS.union(Capabilities::TRACES),
            Self::Portions => Capabilities::CALIBRATION,
        }
    }
}

/// One section of the statistics, each within a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stats<'a> {
    Counters(Counters),
    #[serde(borrow)]
    Memory(Memory<'a>),
    Flash(Flash),
    Power(PowerStats),
    Food(Food),
    Feeds(FeedStats),
    Portions(Portions),
}

/// What the feeder counts, since this boot and over its life.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counters {
    pub boot: Counts,
    /// Why this boot started.
    pub reset: ResetCause,
    /// Over the feeder's life, this boot's included, from a feeder that
    /// keeps its totals in storage; `None` from one that keeps none (no
    /// storage sectors hold them yet).
    pub lifetime: Option<Counts>,
    /// Boots by their reset's cause, this one included; `None` as
    /// `lifetime`.
    pub resets: Option<Resets>,
    /// This boot's sessions: the longest and the mean time from accepting
    /// the connection to the open session.
    pub handshake_max_ms: u32,
    pub handshake_mean_ms: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub uptime_s: u32,
    /// Feeds that ended, however they ended.
    pub feeds: u32,
    /// Feeds that ended other than done.
    pub feeds_failed: u32,
    /// Portions the feedback sensor confirmed.
    pub portions: u32,
    /// Button gestures the panel took.
    pub presses: u32,
    /// Sessions opened.
    pub sessions: u32,
    /// Wi-Fi links lost, each followed by a rejoin.
    pub reconnects: u32,
    /// DHCP leases renewed (or rebound) without a rejoin.
    pub dhcp_renewals: u32,
    /// Times the adapter went away.
    pub outages: u32,
    /// Seconds run on the batteries.
    pub battery_s: u32,
}

/// Why a boot started, from the ROM's reset flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResetCause {
    /// No reset flag: the supply came up.
    #[default]
    PowerOn,
    Watchdog,
    /// A system reset the firmware asked for (a restart, an update).
    Software,
    Brownout,
    /// A flag none of the above names.
    Other,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resets {
    pub power_on: u32,
    pub watchdog: u32,
    /// A system reset the firmware asked for (a restart, an update).
    pub software: u32,
    pub brownout: u32,
    pub other: u32,
}

/// The task stacks one [`Memory`] lists.
pub const TASKS_MAX: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Memory<'a> {
    pub heap_free: u32,
    pub heap_min: u32,
    /// Each task's fewest free stack bytes since it started, as the RTOS
    /// reports them; empty where it reports none.
    #[serde(borrow)]
    pub tasks: [Option<TaskStack<'a>>; TASKS_MAX],
    /// The tasks the RTOS runs: more than `tasks` lists when they did not
    /// all fit.
    pub tasks_total: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskStack<'a> {
    /// At most 16 bytes.
    pub name: &'a str,
    pub free_min: u32,
}

/// The storage sectors [`Erases`] counts one by one, from the region's
/// first; the board's map says which store owns each. The region has grown
/// past them (to 48 sectors), but a message never changes a field's type,
/// so the later sectors' erases reach the wire only in a new variant.
pub const FLASH_SECTORS: usize = 32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flash {
    pub boot: Erases,
    /// Over the feeder's life; `None` as [`Counters::lifetime`].
    pub lifetime: Option<Erases>,
    /// Records the event ring holds, and the most it holds.
    pub events_kept: u32,
    pub events_capacity: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Erases {
    /// Each storage sector's erases, by its index in the region.
    pub storage: [u32; FLASH_SECTORS],
    /// Bank 1's sectors, erased by updates.
    pub update: u32,
}

/// Points in [`PowerStats::battery_mv`].
pub const POWER_HISTORY: usize = 24;

/// The supply since this boot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowerStats {
    /// `None` before the supply was first sampled.
    pub adapter: Option<bool>,
    pub adapter_changes: u32,
    pub outages: u32,
    pub battery_s: u32,
    pub battery_min_mv: Option<u32>,
    pub battery_max_mv: Option<u32>,
    /// The battery's voltage at the end of each period, oldest first and
    /// the newest last; `None` where no reading was taken.
    pub battery_mv: [Option<u16>; POWER_HISTORY],
    pub period_s: u32,
}

/// The food beam since this boot. It is read only while the emitters are
/// on, during feeds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Food {
    /// The beam's last reading: `Some(true)` with food, `None` before one.
    pub present: Option<bool>,
    /// The beam went from food to none.
    pub emptied: u32,
    /// The beam went from none to food.
    pub refilled: u32,
    /// Feeds since the last refill this boot saw; `None` before one.
    pub feeds_since_refill: Option<u32>,
}

/// What a trace's current readings count in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CurrentUnit {
    /// The current channel scaled as stock scales it (the ADC's millivolts
    /// at the sense input): proportional to the motor current, but not yet
    /// calibrated to milliamps.
    #[default]
    Scaled,
    MilliAmps,
}

/// One feed's trace, summed up: what the drive measured, from every poll,
/// whether or not the body kept each reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceSummary {
    pub feed: u32,
    pub requested: u8,
    pub completed: u8,
    pub outcome: FeedOutcome,
    pub reason: FeedReason,
    /// How long the motor was commanded to move; 0 when it never started.
    pub drive_ms: u32,
    /// Polls that read the current while the motor was driven.
    pub samples: u32,
    pub unit: CurrentUnit,
    pub current_peak: u32,
    pub current_mean: Option<u32>,
    /// Feedback edges, rising and falling, and the first one's time from
    /// the drive's start.
    pub rising: u16,
    pub falling: u16,
    pub first_edge_ms: Option<u32>,
    /// Times a poll found the outlet beam blocked after finding it clear.
    pub beam_breaks: u16,
    /// Turns of the motor command away from the feed's own direction:
    /// one reverse-and-resume counts once, as the owner counts reverse
    /// attempts.
    pub reversals: u16,
    /// The battery pack just before the drive, and its lowest during it;
    /// `None` with no pack fitted, or no reading.
    pub battery_rest_mv: Option<u16>,
    pub battery_min_mv: Option<u16>,
    /// The body keeps every `stride`th current reading: 1 for each poll,
    /// doubling each time a full body halved them.
    pub stride: u16,
    /// The body ran out of room for marks and other entries, and ends
    /// before the drive did.
    pub truncated: bool,
}

/// One page of a trace's body ([`crate::trace`]): `bytes` from `offset`,
/// of `total` in all, and the trace's summary with each page, so a client
/// can tell when the trace it was reading has gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TracePage<'a> {
    pub summary: TraceSummary,
    /// The trace is in the feeder's storage, so it outlives a restart.
    pub stored: bool,
    pub offset: u16,
    pub total: u16,
    #[serde(borrow)]
    pub bytes: &'a [u8],
}

/// This boot's feeds, from their traces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedStats {
    /// Feeds traced.
    pub traced: u32,
    /// Their drive time, polls and current, as each summary counts them.
    pub drive_ms: u32,
    pub samples: u32,
    pub unit: CurrentUnit,
    pub current_peak: u32,
    pub current_mean: Option<u32>,
    pub rising: u32,
    pub falling: u32,
    pub beam_breaks: u32,
    pub reversals: u32,
    /// Traces the storage ring holds, and the most it is sure to hold.
    pub kept: u32,
    pub capacity: u32,
    /// Traces that never reached storage: the next feed's took the RAM
    /// trace first, or the write failed.
    pub unsaved: u32,
    /// The latest feed's summary.
    pub last: Option<TraceSummary>,
}

/// The portions since this boot: what the feedback sensor credited, what
/// the outlet beam saw, and what they weigh by the calibration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Portions {
    /// Portions credited, one a mark (as [`Counts::portions`]).
    pub portions: u32,
    /// Feeds that began with a homing mark, uncredited: each may have
    /// dropped part of a compartment more than it credited.
    pub homed: u32,
    /// Times the outlet beam was seen broken during feeds, sampled every
    /// 10 ms: a cross-check on the credited portions, and a lower bound
    /// (shorter passages fall between samples).
    pub outlet_breaks: u32,
    /// The calibration's weight of one portion, `None` before it is set.
    pub portion_mg: Option<u32>,
    /// `portions` times `portion_mg`: what was delivered by the
    /// calibration, never what the pet ate.
    pub delivered_mg: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode, encode};

    #[test]
    fn the_largest_messages_fit_a_frame() {
        let label = "0123456789abcdef";
        let client = Client {
            key: [0xff; 32],
            label,
            role: Role::UpdateSigner,
            you: true,
        };
        let clients = FeederMessage::Response {
            id: [0xff; 16],
            result: Outcome::Clients(Clients {
                clients: [Some(client); CLIENTS_MAX],
            }),
        };
        let mut out = [0; MESSAGE_MAX];
        let len = encode(&clients, &mut out).unwrap();
        assert_eq!(decode::<FeederMessage<'_>>(&out[..len]), Ok(clients));

        let wifi = ClientMessage::Request {
            id: [0xff; 16],
            op: Op::PutWifi {
                base: u32::MAX,
                wifi: Some(Wifi {
                    ssid: &[0xff; 32],
                    psk: "0123456789012345678901234567890123456789012345678901234567890ab",
                }),
            },
        };
        let len = encode(&wifi, &mut out).unwrap();
        assert_eq!(decode::<ClientMessage<'_>>(&out[..len]), Ok(wifi));

        // The largest offer: a version 2 manifest and a downgrade statement,
        // each well past the size the feeder accepts.
        let offer = ClientMessage::Request {
            id: [0xff; 16],
            op: Op::OfferUpdate {
                manifest: &[0xff; 200],
                downgrade: Some(&[0xff; 200]),
            },
        };
        let len = encode(&offer, &mut out).unwrap();
        assert_eq!(decode::<ClientMessage<'_>>(&out[..len]), Ok(offer));

        let chunk = ClientMessage::Request {
            id: [0xff; 16],
            op: Op::UpdateChunk {
                offset: u32::MAX,
                data: &[0xff; UPDATE_CHUNK_MAX],
            },
        };
        let len = encode(&chunk, &mut out).unwrap();
        assert_eq!(decode::<ClientMessage<'_>>(&out[..len]), Ok(chunk));

        let view = FeederMessage::Response {
            id: [0xff; 16],
            result: Outcome::Update(UpdateView {
                phase: UpdatePhase::Receiving,
                bank: 0,
                min_version: u32::MAX,
                digest: Some([0xff; 32]),
                version: u32::MAX,
                image_len: u32::MAX,
                written: u32::MAX,
                wanted: [Some(u32::MAX); WANTED_MAX],
                stopped: Some(UpdateStop::Store),
            }),
        };
        let len = encode(&view, &mut out).unwrap();
        assert_eq!(decode::<FeederMessage<'_>>(&out[..len]), Ok(view));

        let text = [b'x'; LOG_TEXT_MAX];
        let log = FeederMessage::Log {
            seq: u32::MAX,
            dropped: u32::MAX,
            text: core::str::from_utf8(&text).unwrap(),
        };
        let len = encode(&log, &mut out).unwrap();
        assert_eq!(decode::<FeederMessage<'_>>(&out[..len]), Ok(log));
        for stats in largest_stats() {
            let message = FeederMessage::Response {
                id: [0xff; 16],
                result: Outcome::Stats(stats),
            };
            let len = encode(&message, &mut out).unwrap();
            assert_eq!(decode::<FeederMessage<'_>>(&out[..len]), Ok(message));
        }

        let page = FeederMessage::Response {
            id: [0xff; 16],
            result: Outcome::FeedTrace(TracePage {
                summary: largest_summary(),
                stored: true,
                offset: u16::MAX,
                total: u16::MAX,
                bytes: &[0xff; crate::trace::PAGE_MAX],
            }),
        };
        let len = encode(&page, &mut out).unwrap();
        assert_eq!(decode::<FeederMessage<'_>>(&out[..len]), Ok(page));

        let bank1 = FeederMessage::Response {
            id: [0xff; 16],
            result: Outcome::Bank1(Bank1Page {
                offset: u32::MAX,
                total: u32::MAX,
                bytes: &[0xff; BANK1_PAGE_MAX],
            }),
        };
        let len = encode(&bank1, &mut out).unwrap();
        assert_eq!(decode::<FeederMessage<'_>>(&out[..len]), Ok(bank1));
        // The bank is a whole number of pages.
        assert_eq!(0x15_0000 % BANK1_PAGE_MAX, 0);

        let stock = FeederMessage::Response {
            id: [0xff; 16],
            result: Outcome::StockPlans(StockPlanSectors {
                count: u8::MAX,
                entries: &[0xff; STOCK_PLAN_ENTRIES_LEN],
            }),
        };
        let len = encode(&stock, &mut out).unwrap();
        assert_eq!(decode::<FeederMessage<'_>>(&out[..len]), Ok(stock));

        let plan = Some(Plan {
            minute: u16::MAX,
            portions: u8::MAX,
            days: u8::MAX,
            enabled: true,
        });
        let put = ClientMessage::Request {
            id: [0xff; 16],
            op: Op::PutPlans {
                base: u32::MAX,
                plans: [plan; PLANS_MAX],
            },
        };
        let len = encode(&put, &mut out).unwrap();
        assert_eq!(decode::<ClientMessage<'_>>(&out[..len]), Ok(put));
        let plans = FeederMessage::Response {
            id: [0xff; 16],
            result: Outcome::Plans(Plans {
                generation: u32::MAX,
                writable: true,
                plans: [plan; PLANS_MAX],
            }),
        };
        let len = encode(&plans, &mut out).unwrap();
        assert_eq!(decode::<FeederMessage<'_>>(&out[..len]), Ok(plans));
    }

    /// A summary with every field at its largest encoding.
    fn largest_summary() -> TraceSummary {
        TraceSummary {
            feed: u32::MAX,
            requested: u8::MAX,
            completed: u8::MAX,
            outcome: FeedOutcome::Uncertain,
            reason: FeedReason::Output,
            drive_ms: u32::MAX,
            samples: u32::MAX,
            unit: CurrentUnit::MilliAmps,
            current_peak: u32::MAX,
            current_mean: Some(u32::MAX),
            rising: u16::MAX,
            falling: u16::MAX,
            first_edge_ms: Some(u32::MAX),
            beam_breaks: u16::MAX,
            reversals: u16::MAX,
            battery_rest_mv: Some(u16::MAX),
            battery_min_mv: Some(u16::MAX),
            stride: u16::MAX,
            truncated: true,
        }
    }

    /// A summary fits the storage slot's header with room to spare.
    #[test]
    fn a_summary_fits_its_storage_header() {
        let mut out = [0; 96];
        let len = encode(&largest_summary(), &mut out).unwrap();
        assert!(len <= 80, "{len}");
    }

    /// Every section at its widest: each field at its largest encoding.
    fn largest_stats() -> [Stats<'static>; 7] {
        let counts = Counts {
            uptime_s: u32::MAX,
            feeds: u32::MAX,
            feeds_failed: u32::MAX,
            portions: u32::MAX,
            presses: u32::MAX,
            sessions: u32::MAX,
            reconnects: u32::MAX,
            dhcp_renewals: u32::MAX,
            outages: u32::MAX,
            battery_s: u32::MAX,
        };
        let resets = Resets {
            power_on: u32::MAX,
            watchdog: u32::MAX,
            software: u32::MAX,
            brownout: u32::MAX,
            other: u32::MAX,
        };
        let erases = Erases {
            storage: [u32::MAX; FLASH_SECTORS],
            update: u32::MAX,
        };
        let task = TaskStack {
            name: "0123456789abcdef",
            free_min: u32::MAX,
        };

        [
            Stats::Counters(Counters {
                boot: counts,
                reset: ResetCause::Brownout,
                lifetime: Some(counts),
                resets: Some(resets),
                handshake_max_ms: u32::MAX,
                handshake_mean_ms: u32::MAX,
            }),
            Stats::Memory(Memory {
                heap_free: u32::MAX,
                heap_min: u32::MAX,
                tasks: [Some(task); TASKS_MAX],
                tasks_total: u32::MAX,
            }),
            Stats::Flash(Flash {
                boot: erases,
                lifetime: Some(erases),
                events_kept: u32::MAX,
                events_capacity: u32::MAX,
            }),
            Stats::Power(PowerStats {
                adapter: Some(true),
                adapter_changes: u32::MAX,
                outages: u32::MAX,
                battery_s: u32::MAX,
                battery_min_mv: Some(u32::MAX),
                battery_max_mv: Some(u32::MAX),
                battery_mv: [Some(u16::MAX); POWER_HISTORY],
                period_s: u32::MAX,
            }),
            Stats::Food(Food {
                present: Some(true),
                emptied: u32::MAX,
                refilled: u32::MAX,
                feeds_since_refill: Some(u32::MAX),
            }),
            Stats::Feeds(FeedStats {
                traced: u32::MAX,
                drive_ms: u32::MAX,
                samples: u32::MAX,
                unit: CurrentUnit::MilliAmps,
                current_peak: u32::MAX,
                current_mean: Some(u32::MAX),
                rising: u32::MAX,
                falling: u32::MAX,
                beam_breaks: u32::MAX,
                reversals: u32::MAX,
                kept: u32::MAX,
                capacity: u32::MAX,
                unsaved: u32::MAX,
                last: Some(largest_summary()),
            }),
            Stats::Portions(Portions {
                portions: u32::MAX,
                homed: u32::MAX,
                outlet_breaks: u32::MAX,
                portion_mg: Some(u32::MAX),
                delivered_mg: Some(u64::MAX),
            }),
        ]
    }

    /// A metric not sent before the next of its kind is replaced, so the
    /// feeder holds one of each; each fits a frame many times over.
    #[test]
    fn every_metric_names_its_bit() {
        let stats = [
            Metric::Heap { free: 1, min: 1 },
            Metric::Loop {
                passes: 1,
                max_us: 1,
                p99_us: 1,
            },
            Metric::Gateway {
                round_trip_ms: None,
            },
            Metric::Wifi {
                channel: Some(6),
                ap_changes: 0,
            },
        ];
        for metric in stats {
            assert_eq!(metric.capability(), Capabilities::STATS);
        }
        assert_eq!(Metric::Rssi(-50).capability(), Capabilities::V1);
        assert_eq!(
            Metric::Supply(Supply {
                adapter: None,
                battery: Battery::Unread,
                below_floor: None,
            })
            .capability(),
            Capabilities::POWER
        );
        for section in StatsSection::ALL {
            assert!(Capabilities::KNOWN.contains(section.capability()));
        }
        // Traces need both bits: a feeder with the statistics alone
        // predates them.
        let trace = Op::FeedTrace { feed: 1, offset: 0 }.capability();
        assert!(!Capabilities::STATS.contains(trace));
        assert!(!Capabilities::TRACES.contains(trace));
        assert!(
            Capabilities::STATS
                .union(Capabilities::TRACES)
                .contains(trace)
        );
        assert!(!Capabilities::STATS.contains(StatsSection::Feeds.capability()));
    }

    /// The event ring stores a body in 16 bytes.
    #[test]
    fn every_event_body_fits_sixteen_bytes() {
        let client = [0xff; 4];
        for body in [
            EventBody::Boot {
                bank: 1,
                reset_flags: u32::MAX,
            },
            EventBody::Lock {
                locked: true,
                by: Source::Client(client),
            },
            EventBody::Settings {
                generation: u32::MAX,
                by: Source::Client(client),
            },
            EventBody::Wifi {
                generation: u32::MAX,
                by: Source::Client(client),
            },
            EventBody::Enrolled {
                client,
                role: Role::UpdateSigner,
                path: Path::Recovery,
            },
            EventBody::Revoked {
                client,
                by: Source::Client(client),
            },
            EventBody::ClockSync {
                by: Source::Client(client),
                jump_ms: Some(i32::MIN),
            },
            EventBody::Lost { count: u16::MAX },
            EventBody::Fed {
                feed: u32::MAX,
                requested: u8::MAX,
                completed: u8::MAX,
                outcome: FeedOutcome::Uncertain,
                reason: FeedReason::Output,
                hopper: FoodLevel::Unread,
                by: Source::Client(client),
            },
            EventBody::FeedingBlocked {
                reason: Some(Blocked::Unserved),
            },
            EventBody::Supply(Supply {
                adapter: Some(false),
                battery: Battery::Stale { age_ms: u32::MAX },
                below_floor: Some(true),
            }),
            EventBody::Supply(Supply {
                adapter: Some(true),
                battery: Battery::Implausible { mv: u16::MAX },
                below_floor: Some(false),
            }),
            EventBody::Supply(Supply {
                adapter: Some(false),
                battery: Battery::Reading {
                    mv: u16::MAX,
                    percent: u8::MAX,
                },
                below_floor: Some(true),
            }),
            EventBody::Fed {
                feed: u32::MAX,
                requested: u8::MAX,
                completed: u8::MAX,
                outcome: FeedOutcome::Uncertain,
                reason: FeedReason::Output,
                hopper: FoodLevel::Empty,
                by: Source::Schedule(u8::MAX),
            },
            EventBody::Plans {
                generation: u32::MAX,
                by: Source::Client(client),
            },
            EventBody::Missed {
                plan: u8::MAX,
                due: u32::MAX,
                reason: MissedReason::Blocked(Blocked::Unserved),
            },
            EventBody::DispenseRefused {
                reason: Blocked::Battery,
            },
            EventBody::MealSoon {
                plan: u8::MAX,
                due: u32::MAX,
                minute: u16::MAX,
                portions: u8::MAX,
            },
            EventBody::Food {
                level: FoodLevel::Empty,
            },
        ] {
            let mut out = [0; 16];
            let len = encode(&body, &mut out).unwrap();
            assert_eq!(decode::<EventBody>(&out[..len]), Ok(body));
        }
    }
}
