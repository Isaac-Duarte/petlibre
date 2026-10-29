//! What the device API's requests hand the firmware's owners, and how
//! each request ended: settings, the panel lock, the clock, a tune, the
//! Wi-Fi credentials and the client store. The owner tasks fill these from their own state; the
//! device API only formats them.
//!
//! Sans-IO and allocation-free, like the rest of the crate. The settings'
//! and a tune's types are the SDK's: its settings mailbox and speaker carry
//! them for every firmware.

use wellfed_core::clock::wall::{Jump, Sync, SyncError};
pub use wellfed_core::clock::wall::{Origin, State as ClockState};
pub use wellfed_core::clock::zone::Zone;
use wellfed_core::named_enum;

use crate::clients::{Label, Role};

pub use wellfed_core::settings::{
    MAX_VOLUME, MealReminder, SettingsFields, SettingsResult, SettingsView, Window,
};
pub use wellfed_core::tone::PlayResult;

/// The fields of a `lock` request; none present reads the lock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LockFields {
    pub locked: Option<bool>,
}

impl LockFields {
    /// Whether the request changes anything (and so must be signed).
    pub fn changes(&self) -> bool {
        self.locked.is_some()
    }
}

/// The fields of a `time` request, as sent (validated by the clock).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimeFields {
    pub unix_ms: u64,
    pub offset_s: i32,
    pub next_change_s: u64,
    pub next_offset_s: i32,
}

/// How a `time` request ended, for its reply: the jump against the running
/// clock in milliseconds (`None` for a first sync), or the refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeResult {
    Synced { jump_ms: Option<i64> },
    Refused(&'static str),
}

impl TimeFields {
    /// The sync the wall clock checks and adopts.
    pub const fn sync(self) -> Sync {
        Sync {
            unix_ms: self.unix_ms,
            offset_s: self.offset_s,
            next_change_s: self.next_change_s,
            next_offset_s: self.next_offset_s,
        }
    }
}

impl TimeResult {
    /// How the wall clock took a sync, as the reply reports it.
    pub const fn from_sync(result: Result<Jump, SyncError>) -> Self {
        match result {
            Ok(Jump::First) => Self::Synced { jump_ms: None },
            Ok(Jump::Step(delta) | Jump::Jumped(delta)) => Self::Synced {
                jump_ms: Some(delta),
            },
            Err(SyncError::Time) => Self::Refused("time"),
            Err(SyncError::Offset) => Self::Refused("offset"),
            Err(SyncError::NextChange) => Self::Refused("next_change"),
            Err(SyncError::Bounded) => Self::Refused("bounded"),
        }
    }
}

/// Where the feeder asks for the time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NtpServer {
    Off,
    /// The router that gave the feeder its address: on a home network the
    /// one host the feeder already trusts to route it.
    Gateway {
        port: u16,
    },
    Address {
        ip: [u8; 4],
        port: u16,
    },
}

impl Default for NtpServer {
    fn default() -> Self {
        Self::Gateway {
            port: wellfed_core::clock::ntp::PORT,
        }
    }
}

/// The time settings: the owner's zone and the NTP server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimeSettings {
    /// `None`: local time comes from a client sync's offsets, if any.
    pub zone: Option<Zone>,
    pub ntp: NtpServer,
}

impl TimeSettings {
    /// A checksum of the settings, so a pending write can tell whether
    /// the store now holds what it asked for without keeping a copy.
    pub fn digest(&self) -> u32 {
        let mut bytes = [0; wellfed_core::clock::zone::TEXT_MAX + 8];
        let text = self.zone.as_ref().map_or(&[][..], Zone::text);
        bytes[..text.len()].copy_from_slice(text);
        let (kind, ip, port) = match self.ntp {
            NtpServer::Off => (0, [0; 4], 0),
            NtpServer::Gateway { port } => (1, [0; 4], port),
            NtpServer::Address { ip, port } => (2, ip, port),
        };
        let tail = wellfed_core::clock::zone::TEXT_MAX;
        bytes[tail] = u8::try_from(text.len()).unwrap_or(u8::MAX);
        bytes[tail + 1] = kind;
        bytes[tail + 2..tail + 6].copy_from_slice(&ip);
        bytes[tail + 6..tail + 8].copy_from_slice(&port.to_le_bytes());
        wellfed_core::store::slot::crc32(&bytes)
    }
}

/// The time settings as the writer last published them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimeSettingsView {
    pub settings: TimeSettings,
    pub generation: u32,
    pub writable: bool,
}

/// How a time settings request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeSettingsResult {
    /// The current settings; `pending` once a change is queued for the
    /// writer.
    Current {
        view: TimeSettingsView,
        pending: bool,
    },
    /// A change is already queued, or the store is still loading.
    Busy,
    ReadOnly,
    /// This firmware keeps no time settings.
    Unserved,
}

pub use wellfed_proto::api::{PORTION_MG_MAX, PORTION_MG_MIN};

/// The portion calibration as the feeder stores it: what the owner
/// weighed. The bowl mode is the build's, not stored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Calibration {
    /// The mean weight of one portion, `None` until measured.
    pub portion_mg: Option<u32>,
}

impl Calibration {
    /// Whether a portion weight is inside the documented limits.
    pub const fn valid(&self) -> bool {
        match self.portion_mg {
            None => true,
            Some(mg) => mg >= PORTION_MG_MIN && mg <= PORTION_MG_MAX,
        }
    }

    /// A checksum of the calibration, so a pending write can tell whether
    /// the store now holds what it asked for.
    pub fn digest(&self) -> u32 {
        let mut bytes = [0; 5];
        if let Some(mg) = self.portion_mg {
            bytes[0] = 1;
            bytes[1..].copy_from_slice(&mg.to_le_bytes());
        }
        wellfed_core::store::slot::crc32(&bytes)
    }
}

/// The calibration as the writer last published it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CalibrationView {
    pub calibration: Calibration,
    pub generation: u32,
    pub writable: bool,
}

/// How a calibration request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalibrationResult {
    /// The current calibration; `pending` once a change is queued for the
    /// writer.
    Current {
        view: CalibrationView,
        pending: bool,
    },
    /// A change is already queued, or the store is still loading.
    Busy,
    ReadOnly,
    /// This firmware keeps no calibration.
    Unserved,
}

/// What the last NTP exchange came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NtpResult {
    Used,
    /// The answer disagreed with a valid clock beyond both error bounds.
    Bounded,
    Lost,
    /// Unusable: unsynchronized, kiss-o'-death, too loose or malformed.
    Refused,
}

/// The clock and the NTP exchange, for the time view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeState {
    pub clock: ClockState,
    /// The uptime now, to age the last fix.
    pub now_ms: u32,
    pub ntp_last: Option<NtpResult>,
}

/// How a `lock` request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockResult {
    /// The lock as the panel's owner last published it; `pending` once a
    /// change is queued for that owner.
    Current { locked: bool, pending: bool },
    /// A change is already queued.
    Busy,
    /// This firmware runs no panel owner, so there is no lock to set.
    Unserved,
}

/// Which credentials the Wi-Fi join uses this boot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WifiSource {
    /// The build's credentials. An image without build Wi-Fi (a consumer
    /// image) also reports `Build` while nothing is stored, when it joins
    /// nothing at all.
    #[default]
    Build,
    Stored,
    /// Stored credentials failed to join; the build's took over this boot.
    Fallback,
}

impl WifiSource {
    /// The source as the C join shim and the port's atomics carry it.
    pub const fn code(self) -> u8 {
        match self {
            Self::Build => 0,
            Self::Stored => 1,
            Self::Fallback => 2,
        }
    }

    /// The inverse of `code`; anything else reads as the build's.
    pub const fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Stored,
            2 => Self::Fallback,
            _ => Self::Build,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Stored => "stored",
            Self::Fallback => "fallback",
        }
    }
}

/// The Wi-Fi credentials as a `wifi` reply reports them: never the passphrase.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WifiView {
    /// CRC-32 of the stored SSID; `None` when the build's credentials apply.
    pub stored_ssid_crc: Option<u32>,
    pub generation: u32,
    pub writable: bool,
    pub source: WifiSource,
}

/// How a `wifi` request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WifiResult {
    /// The current state; `pending` once a change is queued for the writer.
    Current {
        view: WifiView,
        pending: bool,
    },
    Busy,
    ReadOnly,
    /// The SSID or passphrase failed the store's checks.
    Invalid,
}

/// A change to the client store, queued for the settings writer, which
/// alone writes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientChange {
    Enroll {
        key: [u8; 32],
        label: Label,
        role: Role,
    },
    Revoke {
        key: [u8; 32],
    },
    /// The factory reset: mark it, forget every client, and leave the rest
    /// to the restart that follows.
    Reset,
}

named_enum! {
    /// Whether a one-slot mailbox took a change.
    pub enum Requested {
        Queued = "queued",
        /// A change is already waiting for the writer.
        Busy = "busy",
    }
}

named_enum! {
    /// What became of a queued client store change.
    pub enum KeyResult {
        Done = "done",
        Unarmed = "unarmed",
        /// A reset needs a new identity, and the generator never started.
        NoGenerator = "no-generator",
        NotWritable = "not-writable",
        Full = "full",
        Duplicate = "duplicate",
        NotFound = "not-found",
        Io = "io",
    }
}

named_enum! {
    /// The panel's child lock, which a `lock` request reads or sets.
    pub enum Lock {
        Locked = "locked",
        Unlocked = "unlocked",
    }
}

impl Lock {
    pub const fn from_locked(locked: bool) -> Self {
        if locked { Self::Locked } else { Self::Unlocked }
    }

    pub const fn locked(self) -> bool {
        matches!(self, Self::Locked)
    }
}

pub use wellfed_core::schedule::{PLANS, Plan, Table, Weekdays};

/// The occurrence the loop took before feeding it: the plan in slot
/// `plan`, due at `due_s` (Unix seconds), fed as the next feed after
/// `high_water` (the journal's newest when it was taken). It stays in the
/// store after the feed, so a restart can tell whether that feed was ever
/// reserved, until the next claim replaces it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Claim {
    pub plan: u8,
    pub due_s: u32,
    pub high_water: u32,
    /// The plan ran once, so taking the occurrence disabled it.
    pub disabled: bool,
}

/// The plans as the writer last published them, with what the loop keeps
/// beside them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlansView {
    pub table: Table,
    /// Each slot's mark: the latest occurrence (Unix seconds) already run,
    /// missed or before the plan's save, which never runs; 0 for none. The
    /// claim's occurrence counts too ([`Self::mark`]).
    pub marks: [u32; PLANS],
    pub claim: Option<Claim>,
    /// Bumped by every change to the table (a client's, or a plan that
    /// ran once disabling itself), never by a mark: a client's `base`.
    pub revision: u32,
    /// The revision the last client table saved this boot produced (0 for
    /// none): a client's write landed if this is its `base` plus one, even
    /// once the loop has written since.
    pub replaced: u32,
    pub writable: bool,
}

impl PlansView {
    /// Slot `plan`'s mark in Unix seconds, the claim's occurrence
    /// included; `None` for none.
    pub fn mark(&self, plan: usize) -> Option<u32> {
        let mark = self.marks.get(plan).copied().unwrap_or(0);
        let claimed = self
            .claim
            .filter(|claim| usize::from(claim.plan) == plan)
            .map_or(0, |claim| claim.due_s);
        Some(mark.max(claimed)).filter(|&mark| mark != 0)
    }
}

/// A change to the plans, queued for the writer, which alone writes them.
/// Each but [`Self::Replace`] is the loop's; one taken against a table
/// that has changed since (`revision`) is dropped, and the loop decides
/// again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlansChange {
    /// A client's whole new table, saved at `at_s` (Unix seconds, `None`
    /// with no valid clock): a plan that is new, or whose minute or days
    /// changed, runs only from its first occurrence after it.
    Replace { table: Table, at_s: Option<u32> },
    /// Take an occurrence before its feed is queued, so it can never run
    /// twice; a plan that runs once is disabled with it.
    Claim { claim: Claim, revision: u32 },
    /// The claimed feed was refused before it was reserved: give the
    /// occurrence back (re-enabling a plan that runs once).
    Release { claim: Claim },
    /// A restart found the claim's feed never reserved: the occurrence
    /// is spent, and the claim cleared.
    Settle { claim: Claim },
    /// Slot `plan`'s occurrence `due_s` will never run: its mark moves
    /// there, and a plan that runs once is disabled.
    Pass { plan: u8, due_s: u32, revision: u32 },
}

/// How a plans request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlansResult {
    /// The current plans; `pending` once a change is queued for the
    /// writer.
    Current {
        view: PlansView,
        pending: bool,
    },
    /// A change is already queued, or the store is still loading.
    Busy,
    ReadOnly,
    /// This firmware keeps no plans.
    Unserved,
}

pub use wellfed_proto::api::{CallClip, CallView};

const _: () = assert!(
    wellfed_proto::api::CALL_BYTES_MAX == wellfed_core::clip::BYTES_MAX
        && wellfed_proto::api::CALL_CHUNK_MAX == wellfed_core::clip::CHUNK_MAX as usize
        && wellfed_proto::api::CALL_PLAYS_MAX == wellfed_core::clip::PLAYS_MAX
        && wellfed_proto::api::CALL_RATE_HZ == wellfed_core::clip::RATE_HZ
);

/// A change to the meal call, queued for the writer, which alone writes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallChange {
    /// Erase the clip and wait for an upload of `bytes` whose CRC is `crc`.
    Start {
        bytes: u32,
        crc: u32,
    },
    Delete,
}

/// How a meal call request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallResult {
    /// The call as the writer last published it; `pending` while a change
    /// or a chunk is queued for it.
    Current {
        view: CallView,
        pending: bool,
    },
    /// A change is already queued, or the store is still loading.
    Busy,
    ReadOnly,
    /// This firmware keeps no meal call.
    Unserved,
}

/// Whether a chunk of `len` bytes at `offset` is the next the upload in
/// `clip` wants (`true`), one it already has (`false`), or neither:
/// `Invalid`, as is any chunk with no upload under way. Every chunk but
/// the last is a whole [`wellfed_core::clip::CHUNK_MAX`], so each lands in
/// one sector.
pub fn chunk_wanted(
    clip: CallClip,
    offset: u32,
    len: usize,
) -> Result<bool, wellfed_proto::api::Refusal> {
    let CallClip::Receiving {
        bytes, received, ..
    } = clip
    else {
        return Err(wellfed_proto::api::Refusal::Invalid);
    };
    if offset < received && offset.is_multiple_of(wellfed_core::clip::CHUNK_MAX) {
        return Ok(false);
    }
    let left = bytes.saturating_sub(received);
    let whole = left.min(wellfed_core::clip::CHUNK_MAX);
    if offset == received && left > 0 && u32::try_from(len).ok() == Some(whole) {
        Ok(true)
    } else {
        Err(wellfed_proto::api::Refusal::Invalid)
    }
}
