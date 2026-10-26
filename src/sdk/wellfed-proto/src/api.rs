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
    /// Every bit this crate defines: what a current client claims. A
    /// session then runs on the bits both sides claim, so a client adapts
    /// to whatever feeder it meets, however old.
    pub const KNOWN: Self = Self(Self::FEED.0 | Self::UPDATE.0);

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
    /// result is a [`EventBody::Fed`] in the history. Refused `Unavailable`
    /// by a feeder without [`Capabilities::FEED`].
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
        )
    }

    /// The bit both sides must claim before the op is served. The match
    /// has no wildcard, so a new op cannot land without its decision.
    pub const fn capability(&self) -> Capabilities {
        match self {
            Self::Feed { .. } | Self::CancelFeed { .. } => Capabilities::FEED,
            Self::OfferUpdate { .. }
            | Self::UpdateChunk { .. }
            | Self::UpdateState
            | Self::CancelUpdate
            | Self::EnrollUpdateKey { .. }
            | Self::UpdateKeyStatement { .. } => Capabilities::UPDATE,
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
}

#[expect(
    clippy::large_enum_variant,
    reason = "a wire message, built in place: the feeder cannot box"
)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub generation: u32,
    pub lights: bool,
    pub sound: bool,
    pub volume: u8,
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
    /// confirmed.
    Fed {
        feed: u32,
        requested: u8,
        completed: u8,
        outcome: FeedOutcome,
        reason: FeedReason,
        by: Source,
    },
}

impl EventBody {
    /// The bit a session's hello must carry for the feeder to push this
    /// body; a session without it never sees the event (its cursor moves
    /// past it). No wildcard, as for [`Op::capability`].
    pub const fn capability(&self) -> Capabilities {
        match self {
            Self::Fed { .. } => Capabilities::FEED,
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
    Uncertain,
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
    /// No portion was confirmed for too long: empty, jammed or a sensor fault.
    NoFeedback,
    Power,
    Sensor,
    /// The outlet beam read blocked.
    Outlet,
    Overcurrent,
    /// The feedback sensor's edges could not be trusted.
    FeedbackStream,
    /// An output failed to apply or verify.
    Output,
}

/// The first 4 bytes of a client's key: enough to tell clients apart in
/// the history, which has 16 bytes for a body.
pub type Fingerprint = [u8; 4];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    Panel,
    Client(Fingerprint),
    Feeder,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Metric {
    Power(Power),
    Rssi(i32),
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
                by: Source::Client(client),
            },
        ] {
            let mut out = [0; 16];
            let len = encode(&body, &mut out).unwrap();
            assert_eq!(decode::<EventBody>(&out[..len]), Ok(body));
        }
    }
}
