//! App parity: the app goes through the server first, so every device
//! operation it makes needs a server route. [`parity`] matches every
//! [`Op`] with no wildcard, so a new operation does not build until it is
//! given a route or a stated reason it has none.

use wellfed_model::Op;

/// An HTTP method, as the API document spells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

/// Where a device operation is served through the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parity {
    /// This route, under `/api/v1`, in the API document's spelling.
    Route(Method, &'static str),
    /// No route, and why.
    Without(NoRoute),
}

/// Why a device operation has no server route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoRoute {
    /// The server keeps every feeder's clock and zone itself, from the
    /// household's zone; the zone is changed at `PATCH /household`.
    ClockKeptByServer,
    /// A step of an install, which the server drives as one operation of
    /// its own, owner-approved, from a verified release.
    Install,
    /// It needs a press at the feeder, so the owner runs it at the feeder
    /// with the CLI.
    AtTheFeeder,
    /// A diagnostic read (the record log, statistics, feed traces) the
    /// owner's CLI makes directly; the server's views of them come later.
    Diagnostics,
    /// The portion calibration is measured at the feeder with a scale and
    /// the owner's CLI.
    Calibration,
    /// The feeding plans' route is still to come; until then the owner's
    /// CLI edits them on the feeder.
    PlansToCome,
}

/// The server route for a device operation, or why it has none.
pub const fn parity(op: &Op) -> Parity {
    use Method::{Delete, Get, Patch, Post, Put};
    use Parity::{Route, Without};

    match op {
        Op::Status => Route(Get, "/feeders/{id}"),
        Op::GetSettings => Route(Get, "/feeders/{id}/settings"),
        Op::PutSettings { .. } => Route(Patch, "/feeders/{id}/settings"),
        Op::PutWifi { .. } => Route(Put, "/feeders/{id}/wifi"),
        Op::SetLock { .. } => Route(Post, "/feeders/{id}/lock"),
        Op::Play { .. } => Route(Post, "/feeders/{id}/play"),
        Op::CreateInvite { .. } => Route(Post, "/feeders/{id}/invites"),
        Op::ListClients => Route(Get, "/feeders/{id}/clients"),
        Op::RevokeClient { .. } => Route(Delete, "/feeders/{id}/clients/{key}"),
        Op::Feed { .. } => Route(Post, "/feeders/{id}/feed"),
        Op::Reboot => Route(Post, "/feeders/{id}/reboot"),
        Op::CancelFeed { .. } => Route(Post, "/feeders/{id}/feeds/{feed}/cancel"),
        Op::PutTime { .. } => Route(Patch, "/household"),
        Op::TimeSync(_) | Op::GetTime => Without(NoRoute::ClockKeptByServer),
        Op::OfferUpdate { .. }
        | Op::UpdateChunk { .. }
        | Op::UpdateState
        | Op::CancelUpdate
        | Op::ReadBank1 { .. } => Without(NoRoute::Install),
        Op::EnrollUpdateKey { .. } | Op::UpdateKeyStatement { .. } => Without(NoRoute::AtTheFeeder),
        Op::TailLog { .. } | Op::Stats { .. } | Op::FeedTrace { .. } => {
            Without(NoRoute::Diagnostics)
        }
        Op::GetCalibration | Op::PutCalibration { .. } => Without(NoRoute::Calibration),
        Op::GetPlans | Op::PutPlans { .. } => Without(NoRoute::PlansToCome),
    }
}
