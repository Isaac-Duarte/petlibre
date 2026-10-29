//! `wellfed-model`'s plain enums, mirrored so flutter_rust_bridge generates
//! their Dart types from the model itself. A mirror lists the same variants
//! as its model type; the generated code converts by name, so a variant
//! added to the model fails the bridge's build until it is mirrored here.

use flutter_rust_bridge::frb;
pub use wellfed_model::setup::{Enrolled, Refusal as SetupRefusal, WifiSaved};
pub use wellfed_model::{Blocked, ClientRole, FeedOutcome, FeedReason, FoodLevel};

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
