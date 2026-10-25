//! A session's requests and the feeder's answers to them.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{
    ClientRole, Generation, Hex, Settings, SettingsPatch, Status, UpdateRefusal, UpdateView,
};

/// One operation. Reads may be repeated freely; a write's request id makes
/// a retry return the first answer instead of running again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Status,
    GetSettings,
    /// Applies only if the settings are still at `base`.
    PutSettings {
        base: Generation,
        changes: SettingsPatch,
    },
    /// Stores credentials for the next join, or clears them with `None`.
    /// Applies only if the Wi-Fi store is still at `base`.
    PutWifi {
        base: Generation,
        wifi: Option<Wifi>,
    },
    SetLock {
        locked: bool,
    },
    Play {
        tune: Tune,
    },
    TimeSync(TimeSync),
    /// A single-use invite for a new client, held in the feeder's RAM for
    /// 10 minutes.
    CreateInvite {
        role: ClientRole,
    },
    ListClients,
    RevokeClient {
        key: [u8; 32],
    },
    /// Answered [`Answer::Feeding`] once the feed is durably reserved; its
    /// result is a [`crate::EventBody::Fed`] in the history.
    Feed {
        portions: u8,
    },
    Reboot,
    /// Stop the feed [`Answer::Feeding`] named, if it is still running.
    CancelFeed {
        feed: u32,
    },
    /// Offer a signed version 2 manifest, with the downgrade statement it
    /// needs when its version is not above the floor (always, for stock).
    /// Bank 1 gives way to bank 0, where the same offer starts the install.
    OfferUpdate {
        manifest: Vec<u8>,
        downgrade: Option<Vec<u8>>,
    },
    /// Image bytes at one of the offsets [`UpdateView::wanted`] names.
    /// Idempotent by offset.
    UpdateChunk {
        offset: u32,
        data: Vec<u8>,
    },
    UpdateState,
    /// Stop the install with bank 1 left invalid.
    CancelUpdate,
    /// Enroll an owner's Ed25519 update-signing key, once a Dispense press
    /// at the feeder confirms it.
    EnrollUpdateKey {
        key: [u8; 32],
        label: String,
    },
    /// A key statement a built-in or the release key signed.
    UpdateKeyStatement {
        statement: Vec<u8>,
    },
}

/// Credentials for the feeder's next join.
#[derive(Clone, PartialEq, Eq)]
pub struct Wifi {
    /// 1-32 bytes; an SSID need not be text.
    pub ssid: Vec<u8>,
    /// 8-63 printable ASCII characters (WPA2-PSK).
    pub psk: String,
}

impl fmt::Debug for Wifi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Wifi")
            .field("ssid", &String::from_utf8_lossy(&self.ssid))
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tune {
    Chime,
    Meal,
    Coin,
    Alert,
    Test,
    Twinkle,
}

/// The wall clock and the zone's next change, so the feeder keeps local
/// time across a DST change without another sync.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeSync {
    pub unix_ms: u64,
    pub offset_s: i32,
    /// Unix seconds of the zone's next change, 0 when it has none ahead.
    pub next_change_s: u64,
    pub next_offset_s: i32,
}

/// The feeder's answer to one request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    /// A write took effect (or a read of nothing); for a settings or Wi-Fi
    /// write, `generation` is the store's after it.
    Done {
        generation: Option<Generation>,
    },
    Status(Status),
    Settings(Settings),
    /// The enrolled clients, in store order.
    Clients(Vec<Client>),
    Invite(Invite),
    Refused(Refusal),
    /// The feed is reserved in the feeder's journal under `feed` and runs
    /// now; a retry of the request returns this again rather than feeding
    /// twice.
    Feeding {
        feed: u32,
    },
    Update(UpdateView),
}

/// Why the feeder refused a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    /// This client's role may not do that.
    Forbidden,
    /// The store moved past the request's `base`: re-read and decide again.
    Stale {
        current: Generation,
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
    Update(UpdateRefusal),
}

/// One enrolled client.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Client {
    pub key: Hex<32>,
    pub label: String,
    pub role: ClientRole,
    /// Whether this is the session asking.
    pub you: bool,
}

/// A single-use invite. Anyone holding `secret` within `seconds` joins with
/// the role it was created for, so it is shown once and never logged.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invite {
    pub id: u32,
    /// The PSK the new client's handshake must hold.
    pub secret: Hex<32>,
    pub seconds: u16,
}

impl fmt::Debug for Invite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Invite")
            .field("id", &self.id)
            .field("seconds", &self.seconds)
            .finish_non_exhaustive()
    }
}
