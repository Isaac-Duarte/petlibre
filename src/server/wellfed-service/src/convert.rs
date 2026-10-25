//! Device-protocol values to the HTTP API's, in one place.

use jiff::Timestamp;
use wellfed_api as api;
use wellfed_api::{Generation, Hex};
use wellfed_proto::api as proto;

pub(crate) fn status(status: &proto::Status<'_>) -> api::FeederStatus {
    api::FeederStatus {
        build: status.build.to_owned(),
        bank: status.bank,
        uptime_ms: status.uptime_ms,
        boot_count: status.boot_count,
        reset_flags: status.reset_flags,
        faults: status.faults,
        rssi: status.rssi,
        wifi: match status.wifi {
            proto::WifiSource::Build => api::WifiSource::Build,
            proto::WifiSource::Stored => api::WifiSource::Stored,
            proto::WifiSource::Fallback => api::WifiSource::Fallback,
        },
        link_losses: status.link_losses,
        dns_losses: status.dns_losses,
        clock: status.clock_ms.and_then(|ms| {
            i64::try_from(ms)
                .ok()
                .and_then(|ms| Timestamp::from_millisecond(ms).ok())
        }),
        power: status.power.map(power),
        locked: status.locked,
        settings_generation: Generation(status.settings_generation),
        wifi_generation: Generation(status.wifi_generation),
        last_event: status.last_event,
        sessions: status.sessions,
    }
}

fn power(power: proto::Power) -> api::Power {
    api::Power {
        adapter: power.adapter,
        battery_mv: power.battery_mv,
    }
}

pub(crate) fn settings(settings: proto::Settings) -> api::Settings {
    api::Settings {
        generation: Generation(settings.generation),
        lights: settings.lights,
        sound: settings.sound,
        volume: settings.volume,
        writable: settings.writable,
    }
}

pub(crate) fn event(epoch: u32, event: &proto::Event, received: Timestamp) -> api::FeederEvent {
    api::FeederEvent {
        epoch,
        seq: event.seq,
        at: event.at.map(seconds),
        received,
        body: body(event.body),
    }
}

/// An event's time: the feeder's clock in Unix seconds.
pub(crate) fn seconds(at: u32) -> Timestamp {
    Timestamp::from_second(i64::from(at)).expect("a u32 of seconds is inside jiff's range")
}

pub(crate) fn body(body: proto::EventBody) -> api::EventBody {
    match body {
        proto::EventBody::Boot { bank, reset_flags } => api::EventBody::Boot { bank, reset_flags },
        proto::EventBody::Lock { locked, by } => api::EventBody::Lock {
            locked,
            by: source(by),
        },
        proto::EventBody::Settings { generation, by } => api::EventBody::Settings {
            generation: Generation(generation),
            by: source(by),
        },
        proto::EventBody::Wifi { generation, by } => api::EventBody::Wifi {
            generation: Generation(generation),
            by: source(by),
        },
        proto::EventBody::Enrolled { client, role, path } => api::EventBody::Enrolled {
            client: Hex(client),
            role: client_role(role),
            path: match path {
                proto::Path::Setup => api::EnrollPath::Setup,
                proto::Path::Invite => api::EnrollPath::Invite,
                proto::Path::Recovery => api::EnrollPath::Recovery,
            },
        },
        proto::EventBody::Revoked { client, by } => api::EventBody::Revoked {
            client: Hex(client),
            by: source(by),
        },
        proto::EventBody::ClockSync { by, jump_ms } => api::EventBody::ClockSync {
            by: source(by),
            jump_ms,
        },
        proto::EventBody::Lost { count } => api::EventBody::Lost { count },
        proto::EventBody::Fed {
            feed,
            requested,
            completed,
            outcome,
            reason,
            by,
        } => api::EventBody::Fed {
            feed,
            requested,
            completed,
            outcome: feed_outcome(outcome),
            reason: feed_reason(reason),
            by: source(by),
        },
    }
}

const fn feed_outcome(outcome: proto::FeedOutcome) -> api::FeedOutcome {
    match outcome {
        proto::FeedOutcome::Done => api::FeedOutcome::Done,
        proto::FeedOutcome::Cancelled => api::FeedOutcome::Cancelled,
        proto::FeedOutcome::Expired => api::FeedOutcome::Expired,
        proto::FeedOutcome::Failed => api::FeedOutcome::Failed,
        proto::FeedOutcome::Uncertain => api::FeedOutcome::Uncertain,
    }
}

const fn feed_reason(reason: proto::FeedReason) -> api::FeedReason {
    match reason {
        proto::FeedReason::Done => api::FeedReason::Done,
        proto::FeedReason::Cancelled => api::FeedReason::Cancelled,
        proto::FeedReason::Clock => api::FeedReason::Clock,
        proto::FeedReason::ObservationGap => api::FeedReason::ObservationGap,
        proto::FeedReason::Timeout => api::FeedReason::Timeout,
        proto::FeedReason::NoFeedback => api::FeedReason::NoFeedback,
        proto::FeedReason::Power => api::FeedReason::Power,
        proto::FeedReason::Sensor => api::FeedReason::Sensor,
        proto::FeedReason::Outlet => api::FeedReason::Outlet,
        proto::FeedReason::Overcurrent => api::FeedReason::Overcurrent,
        proto::FeedReason::FeedbackStream => api::FeedReason::FeedbackStream,
        proto::FeedReason::Output => api::FeedReason::Output,
    }
}

fn source(source: proto::Source) -> api::Source {
    match source {
        proto::Source::Panel => api::Source::Panel,
        proto::Source::Client(client) => api::Source::Client {
            client: Hex(client),
        },
        proto::Source::Feeder => api::Source::Feeder,
    }
}

pub(crate) fn client_role(role: proto::Role) -> api::ClientRole {
    match role {
        proto::Role::Owner => api::ClientRole::Owner,
        proto::Role::Member => api::ClientRole::Member,
        proto::Role::Server => api::ClientRole::Server,
        proto::Role::UpdateSigner => api::ClientRole::UpdateSigner,
    }
}

pub(crate) fn refusal(refusal: proto::Refusal) -> api::FeederRefusal {
    match refusal {
        proto::Refusal::Forbidden => api::FeederRefusal::Forbidden,
        proto::Refusal::Stale { .. } => api::FeederRefusal::Stale,
        proto::Refusal::Busy => api::FeederRefusal::Busy,
        proto::Refusal::Unavailable => api::FeederRefusal::Unavailable,
        proto::Refusal::Invalid => api::FeederRefusal::Invalid,
        proto::Refusal::NotWritable => api::FeederRefusal::NotWritable,
        proto::Refusal::Full => api::FeederRefusal::Full,
        proto::Refusal::NotFound => api::FeederRefusal::NotFound,
        proto::Refusal::Muted => api::FeederRefusal::Muted,
        proto::Refusal::Time => api::FeederRefusal::Time,
        proto::Refusal::Fault => api::FeederRefusal::Fault,
    }
}

pub(crate) fn sample(sample: proto::Sample, received: Timestamp) -> api::FeederSample {
    api::FeederSample {
        received,
        uptime_ms: sample.at_ms,
        value: metric(sample.metric),
    }
}

pub(crate) fn metric(metric: proto::Metric) -> api::SampleValue {
    match metric {
        proto::Metric::Power(power) => api::SampleValue::Power {
            adapter: power.adapter,
            battery_mv: power.battery_mv,
        },
        proto::Metric::Rssi(dbm) => api::SampleValue::Rssi { dbm },
    }
}
