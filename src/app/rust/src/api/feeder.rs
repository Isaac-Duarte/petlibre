//! What Dart asks a feeder and what comes back, beside the card and the feed
//! the session's stream carries. [`crate::requests`] turns each request into
//! the device API's ops, so Dart only ever names what the owner chose.
//!
//! Every write goes out once, under a fresh id: an answer that never came is
//! [`FeederReply::Unanswered`], which says the write may have run, and nothing
//! sends it again. [`FeederReply::NotSent`] means it never left this phone.

pub use crate::api::model::{
    Blocked, Bowl, CallClip, ClientRole, Desiccant, FeedOutcome, FeedReason, FoodLevel,
    MealReminder, NtpResult, Phase, Stats, StatsSection, TimeSource, TraceEntry, TraceSummary,
    Tune, UpdatePhase, UpdateRefusal, Weekday, Window,
};

/// What the owner asks of a feeder.
pub enum FeederRequest {
    GetSettings,
    PutSettings {
        base: u32,
        changes: SettingsChanges,
    },
    /// Stored for the feeder's next join. The rules are checked here first.
    PutWifi {
        base: u32,
        ssid: String,
        passphrase: String,
    },
    ForgetWifi {
        base: u32,
    },
    SetLock {
        locked: bool,
    },
    Play {
        tune: Tune,
    },
    /// This phone's clock and zone.
    SyncClock,
    GetTime,
    PutTime {
        base: u32,
        zone: Option<ZoneChange>,
        /// As `wellfed time --ntp` takes it: `off`, `gateway`, or an address.
        ntp: Option<String>,
    },
    CreateInvite {
        role: ClientRole,
    },
    ListClients,
    RevokeClient {
        key: Vec<u8>,
    },
    Reboot,
    ResetDesiccant,
    Stats {
        section: StatsSection,
    },
    /// Every page of one feed's trace.
    ReadTrace {
        feed: u32,
    },
    GetCalibration,
    PutCalibration {
        base: u32,
        portion: PortionChange,
        bowl: Bowl,
    },
    GetPlans,
    /// The change the owner made to the table they saw.
    EditPlans {
        plans: PlansView,
        edit: PlanEdit,
    },
    GetCall,
    /// Convert a WAV recording and upload it in place of the stored clip.
    UploadCall {
        wav: Vec<u8>,
    },
    DeleteCall,
    /// Where an install stands.
    UpdateState,
    /// Stop the install, with bank 1 left invalid.
    CancelUpdate,
    /// Push the feeder's record lines after `after`, as [`SessionUpdate::Log`].
    ///
    /// [`SessionUpdate::Log`]: crate::api::session::SessionUpdate::Log
    TailLog {
        after: Option<u32>,
    },
}

/// A settings change; what is `None` stays.
#[derive(Clone, Copy, Debug, Default)]
pub struct SettingsChanges {
    pub lights: Option<bool>,
    pub sound: Option<bool>,
    pub volume: Option<u8>,
    pub lights_window: Option<Window>,
    pub sound_window: Option<Window>,
    pub desiccant_days: Option<u8>,
    pub meal_reminder: Option<MealReminder>,
}

pub enum ZoneChange {
    /// This phone's zone.
    Phone,
    /// An IANA name.
    Named {
        name: String,
    },
    Clear,
}

pub enum PortionChange {
    Clear,
    /// One portion's weight in milligrams.
    Milligrams {
        mg: u32,
    },
    /// What `portions` counted portions weighed, in grams as typed.
    Weighed {
        grams: String,
        portions: u32,
    },
}

pub enum PlanEdit {
    Add {
        plan: PlanView,
    },
    Change {
        slot: u8,
        minute: Option<u16>,
        portions: Option<u8>,
        days: Option<Vec<Weekday>>,
        call: Option<u8>,
        enabled: Option<bool>,
    },
    Enabled {
        slot: u8,
        enabled: bool,
    },
    Delete {
        slot: u8,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanView {
    pub minute: u16,
    pub portions: u8,
    /// None runs it once, at its next occurrence.
    pub days: Vec<Weekday>,
    pub enabled: bool,
    pub call: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlansView {
    pub generation: u32,
    pub writable: bool,
    /// All ten slots.
    pub plans: Vec<Option<PlanView>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsView {
    pub generation: u32,
    pub lights: bool,
    pub sound: bool,
    pub volume: u8,
    pub lights_window: Window,
    pub sound_window: Window,
    pub desiccant_days: u8,
    /// The feeder's local date of the last replacement, `YYYY-MM-DD`.
    pub desiccant_replaced: Option<String>,
    pub meal_reminder: MealReminder,
    pub writable: bool,
}

/// Where the feeder asks for the time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NtpView {
    Off,
    Gateway { port: u16 },
    Address { ip: String, port: u16 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeInfo {
    pub generation: u32,
    pub writable: bool,
    /// The zone's POSIX rule.
    pub zone: Option<String>,
    pub ntp: NtpView,
    /// `None` while the clock is invalid: scheduled meals hold.
    pub clock_ms: Option<i64>,
    pub error_ms: Option<u32>,
    pub offset_s: Option<i32>,
    pub syncs: u32,
    pub last: Option<TimeSource>,
    /// When it was last set, by this phone's clock.
    pub last_at_ms: Option<i64>,
    pub ntp_last: Option<NtpResult>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientView {
    pub key: Vec<u8>,
    /// The key's first 4 bytes in hex, as events name a client.
    pub fingerprint: String,
    pub label: String,
    pub role: ClientRole,
    /// Whether this is this phone.
    pub you: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InviteView {
    /// The text a new client redeems: shown once and never logged.
    pub text: String,
    pub seconds: u16,
    pub role: ClientRole,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalibrationInfo {
    pub generation: u32,
    pub writable: bool,
    pub portion_mg: Option<u32>,
    pub bowl: Bowl,
    /// `None` from a feeder with no feeding owner, or before it opens.
    pub phase: Option<Phase>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallInfo {
    pub writable: bool,
    pub clip: CallClip,
    /// How long one play of a stored clip is.
    pub ms: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceView {
    pub summary: TraceSummary,
    pub stored: bool,
    pub entries: Vec<TraceEntry>,
}

/// Where an install stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateInfo {
    pub phase: UpdatePhase,
    /// The bank it answers from.
    pub bank: u8,
    /// The lowest version it will install.
    pub min_version: u32,
    /// The version being installed.
    pub version: u32,
    pub image_len: u32,
    pub written: u32,
}

/// Why the feeder refused a request: the device API's refusals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeederRefusal {
    Forbidden,
    /// The store moved past the base: read it again and decide again.
    Stale {
        current: u32,
    },
    Busy,
    Unavailable,
    Invalid,
    NotWritable,
    Full,
    NotFound,
    Muted,
    Time,
    Fault,
    Update {
        reason: UpdateRefusal,
    },
    Unverified,
    Portions {
        asked: u8,
    },
    NoClock,
}

/// What this phone's Rust half refused before anything was sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotAsked {
    /// The SSID is not 1-32 bytes, or the passphrase not 8-63 printable ASCII.
    Credentials,
    /// That slot holds no plan.
    EmptySlot,
    /// Every slot holds a plan.
    PlansFull,
    /// The change names nothing to change.
    NothingToChange,
    /// A plan's minute, portions or call is outside its limits.
    BadPlan,
    /// The weight is not grams, or counts no portions.
    BadWeight,
    /// The NTP server is not off, gateway or an IPv4 address.
    BadNtp,
    /// The zone is unknown, or changes irregularly.
    BadZone,
    /// The recording is not a WAV file this reads, or is too long once trimmed.
    BadRecording,
    /// A key is not 32 bytes.
    BadKey,
}

/// How a request ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeederReply {
    /// A write took effect; `generation` is the store's after it, when it has one.
    Done {
        generation: Option<u32>,
    },
    Settings {
        settings: SettingsView,
    },
    Time {
        time: TimeInfo,
    },
    Plans {
        plans: PlansView,
    },
    Clients {
        clients: Vec<ClientView>,
    },
    Invite {
        invite: InviteView,
    },
    Calibration {
        calibration: CalibrationInfo,
    },
    Call {
        call: CallInfo,
    },
    Stats {
        stats: Stats,
    },
    Trace {
        trace: TraceView,
    },
    Update {
        update: UpdateInfo,
    },
    Refused {
        refusal: FeederRefusal,
    },
    /// Refused here, before anything was sent.
    NotAsked {
        reason: NotAsked,
    },
    /// No answer came: a write may have run. It is never sent again.
    Unanswered,
    /// The session ended before the request went out: nothing happened.
    NotSent,
    /// The feeder answered something else.
    Unexpected,
}
