//! `wellfed-model`'s plain enums, mirrored so flutter_rust_bridge generates
//! their Dart types from the model itself. A mirror lists the same variants
//! as its model type; the generated code converts by name, so a variant
//! added to the model fails the bridge's build until it is mirrored here.

use flutter_rust_bridge::frb;
pub use wellfed_model::setup::{Enrolled, Refusal as SetupRefusal, WifiSaved};
pub use wellfed_model::{
    Blocked, ClientRole, FeedOutcome, FeedReason, FoodLevel, MissedReason, UpdatePhase, UpdateStop,
};

#[frb(mirror(Enrolled))]
pub enum _Enrolled {
    New,
    Already,
}

#[frb(mirror(WifiSaved))]
pub enum _WifiSaved {
    Saved,
    Unchanged,
}

#[frb(mirror(SetupRefusal))]
pub enum _SetupRefusal {
    Busy,
    Unconfirmed,
    WrongCode,
    Full,
    NotWritable,
    Invalid,
}

#[frb(mirror(ClientRole))]
pub enum _ClientRole {
    Owner,
    Member,
    Server,
    UpdateSigner,
}

#[frb(mirror(FoodLevel))]
pub enum _FoodLevel {
    Unread,
    Present,
    Empty,
}

#[frb(mirror(Blocked))]
pub enum _Blocked {
    Starting,
    Journal,
    Fault,
    Resolving,
    Feeding,
    Joining,
    Updating,
    Setup,
    Unserved,
    Battery,
    Restarting,
}

#[frb(mirror(FeedOutcome))]
pub enum _FeedOutcome {
    Done,
    Cancelled,
    Expired,
    Failed,
    Uncertain,
}

#[frb(mirror(FeedReason))]
pub enum _FeedReason {
    Done,
    Cancelled,
    Clock,
    ObservationGap,
    Timeout,
    NoFeedback,
    Power,
    Sensor,
    OutletBlocked,
    Jammed,
    MarkStuck,
    Overcurrent,
    FeedbackStream,
    Output,
    Interrupted,
}

pub use wellfed_model::{
    Bowl, CallClip, Counters, Counts, CurrentUnit, Desiccant, Erases, FeedStats, Flash, Food,
    MealReminder, Memory, NtpResult, Phase, Portions, PowerStats, ResetCause, Resets, Stats,
    StatsSection, TaskStack, TimeSource, TraceEntry, TraceMotor, TraceSummary, Tune, UpdateRefusal,
    Weekday, Window,
};

#[frb(mirror(Bowl))]
pub enum _Bowl {
    Single,
    Double,
}

#[frb(mirror(Phase))]
pub enum _Phase {
    AtMark,
    Unknown,
}

#[frb(mirror(Tune))]
pub enum _Tune {
    Chime,
    Meal,
    Coin,
    Alert,
    Test,
    Twinkle,
}

#[frb(mirror(Weekday))]
pub enum _Weekday {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

#[frb(mirror(Window))]
pub enum _Window {
    Always,
    Between { start: u16, end: u16 },
}

#[frb(mirror(MealReminder))]
pub enum _MealReminder {
    Off,
    Before { minutes: u8 },
}

#[frb(mirror(Desiccant))]
pub enum _Desiccant {
    Unset,
    Unknown,
    Remaining { days: i32, due: bool },
}

#[frb(mirror(TimeSource))]
pub enum _TimeSource {
    Owner,
    Client,
    Ntp,
}

#[frb(mirror(NtpResult))]
pub enum _NtpResult {
    Used,
    Bounded,
    Lost,
    Refused,
}

#[frb(mirror(CallClip))]
pub enum _CallClip {
    None,
    Receiving { bytes: u32, crc: u32, received: u32 },
    Stored { bytes: u32, crc: u32 },
}

#[frb(mirror(UpdateRefusal))]
pub enum _UpdateRefusal {
    Manifest,
    Key,
    Revoked,
    Signature,
    Target,
    Same,
    Downgrade,
    Statement,
    Pending,
    Failed,
    Fallback,
    Attempt,
    Installing,
    NotInstalling,
    Store,
    Unconfirmed,
    Battery,
}

#[frb(mirror(UpdatePhase))]
pub enum _UpdatePhase {
    Idle,
    Checking,
    GivingWay,
    Receiving,
    Activated,
}

#[frb(mirror(UpdateStop))]
pub enum _UpdateStop {
    Timeout,
    Cancelled,
    Order,
    Marker,
    Write,
    Hash,
    Activate,
    Store,
}

#[frb(mirror(MissedReason))]
pub enum _MissedReason {
    Late,
    Blocked { reason: Blocked },
    Interrupted,
}

#[frb(mirror(StatsSection))]
pub enum _StatsSection {
    Counters,
    Memory,
    Flash,
    Power,
    Food,
    Feeds,
    Portions,
}

#[frb(mirror(Stats))]
pub enum _Stats {
    Counters(Counters),
    Memory(Memory),
    Flash(Flash),
    Power(PowerStats),
    Food(Food),
    Feeds(FeedStats),
    Portions(Portions),
}

#[frb(mirror(Counters))]
pub struct _Counters {
    pub boot: Counts,
    pub reset: ResetCause,
    pub lifetime: Option<Counts>,
    pub resets: Option<Resets>,
    pub handshake_max_ms: u32,
    pub handshake_mean_ms: u32,
}

#[frb(mirror(Counts))]
pub struct _Counts {
    pub uptime_s: u32,
    pub feeds: u32,
    pub feeds_failed: u32,
    pub portions: u32,
    pub presses: u32,
    pub sessions: u32,
    pub reconnects: u32,
    pub dhcp_renewals: u32,
    pub outages: u32,
    pub battery_s: u32,
}

#[frb(mirror(ResetCause))]
pub enum _ResetCause {
    PowerOn,
    Watchdog,
    Software,
    Brownout,
    Other,
}

#[frb(mirror(Resets))]
pub struct _Resets {
    pub power_on: u32,
    pub watchdog: u32,
    pub software: u32,
    pub brownout: u32,
    pub other: u32,
}

#[frb(mirror(Memory))]
pub struct _Memory {
    pub heap_free: u32,
    pub heap_min: u32,
    pub tasks: Vec<TaskStack>,
    pub tasks_total: u32,
}

#[frb(mirror(TaskStack))]
pub struct _TaskStack {
    pub name: String,
    pub free_min: u32,
}

#[frb(mirror(Flash))]
pub struct _Flash {
    pub boot: Erases,
    pub lifetime: Option<Erases>,
    pub events_kept: u32,
    pub events_capacity: u32,
}

#[frb(mirror(Erases))]
pub struct _Erases {
    pub storage: Vec<u32>,
    pub update: u32,
}

#[frb(mirror(PowerStats))]
pub struct _PowerStats {
    pub adapter: Option<bool>,
    pub adapter_changes: u32,
    pub outages: u32,
    pub battery_s: u32,
    pub battery_min_mv: Option<u32>,
    pub battery_max_mv: Option<u32>,
    pub battery_mv: Vec<Option<u16>>,
    pub period_s: u32,
}

#[frb(mirror(Portions))]
pub struct _Portions {
    pub portions: u32,
    pub homed: u32,
    pub outlet_breaks: u32,
    pub portion_mg: Option<u32>,
    pub delivered_mg: Option<u64>,
}

#[frb(mirror(Food))]
pub struct _Food {
    pub present: Option<bool>,
    pub emptied: u32,
    pub refilled: u32,
    pub feeds_since_refill: Option<u32>,
}

#[frb(mirror(FeedStats))]
pub struct _FeedStats {
    pub traced: u32,
    pub drive_ms: u32,
    pub samples: u32,
    pub unit: CurrentUnit,
    pub current_peak: u32,
    pub current_mean: Option<u32>,
    pub rising: u32,
    pub falling: u32,
    pub beam_breaks: u32,
    pub reversals: u32,
    pub kept: u32,
    pub capacity: u32,
    pub unsaved: u32,
    pub last: Option<TraceSummary>,
}

#[frb(mirror(CurrentUnit))]
pub enum _CurrentUnit {
    Scaled,
    MilliAmps,
}

#[frb(mirror(TraceSummary))]
pub struct _TraceSummary {
    pub feed: u32,
    pub requested: u8,
    pub completed: u8,
    pub outcome: FeedOutcome,
    pub reason: FeedReason,
    pub drive_ms: u32,
    pub samples: u32,
    pub unit: CurrentUnit,
    pub current_peak: u32,
    pub current_mean: Option<u32>,
    pub rising: u16,
    pub falling: u16,
    pub first_edge_ms: Option<u32>,
    pub beam_breaks: u16,
    pub reversals: u16,
    pub battery_rest_mv: Option<u16>,
    pub battery_min_mv: Option<u16>,
    pub stride: u16,
    pub truncated: bool,
}

#[frb(mirror(TraceMotor))]
pub enum _TraceMotor {
    Stop,
    Drive0,
    Drive1,
}

#[frb(mirror(TraceEntry))]
pub enum _TraceEntry {
    Current {
        at_ms: u32,
        value: u32,
    },
    Mark {
        at_ms: u32,
        edges: u32,
        high: bool,
    },
    Beams {
        at_ms: u32,
        outlet_clear: bool,
        food: bool,
    },
    Battery {
        at_ms: u32,
        mv: u32,
    },
    Motor {
        at_ms: u32,
        motor: TraceMotor,
    },
}
