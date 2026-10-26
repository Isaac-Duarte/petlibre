//! The wire's values to this crate's, and back where a host sends or
//! stores the wire form, in one place.
//!
//! Every match here is exhaustive and every struct is destructured
//! without `..`, so a variant or field added to `wellfed-proto` does not
//! build until it has a host meaning.

use jiff::Timestamp;
use wellfed_proto::api as proto;
use wellfed_proto::setup as proto_setup;

use crate::{
    Answer, Capabilities, Client, ClientRole, EnrollPath, Event, EventBody, FeedOutcome,
    FeedReason, Generation, Hello, Hex, Invite, Joined, NtpResult, NtpServer, Op, OpenRefusal,
    Power, Refusal, Sample, SampleValue, Settings, SettingsPatch, Source, Status, TimeSource,
    TimeSync, TimeView, Tune, UpdatePhase, UpdateRefusal, UpdateStop, UpdateView, WifiSource,
    setup,
};

/// One event of the history `epoch` counts.
pub fn event(epoch: u32, event: proto::Event) -> Event {
    let proto::Event { seq, at, body } = event;
    Event {
        epoch,
        seq,
        at: at.map(seconds),
        body: body.into(),
    }
}

/// An event's time: the feeder's clock in Unix seconds.
fn seconds(at: u32) -> Timestamp {
    Timestamp::from_second(i64::from(at)).expect("a u32 of seconds is inside jiff's range")
}

impl From<proto::Status<'_>> for Status {
    fn from(status: proto::Status<'_>) -> Self {
        let proto::Status {
            build,
            bank,
            uptime_ms,
            boot_count,
            reset_flags,
            faults,
            rssi,
            wifi,
            link_losses,
            dns_losses,
            clock_ms,
            power,
            locked,
            settings_generation,
            wifi_generation,
            last_event,
            sessions,
        } = status;
        Self {
            build: build.to_owned(),
            bank,
            uptime_ms,
            boot_count,
            reset_flags,
            faults,
            rssi,
            wifi: wifi.into(),
            link_losses,
            dns_losses,
            clock: clock_ms.and_then(|ms| {
                i64::try_from(ms)
                    .ok()
                    .and_then(|ms| Timestamp::from_millisecond(ms).ok())
            }),
            power: power.map(Power::from),
            locked,
            settings_generation: Generation(settings_generation),
            wifi_generation: Generation(wifi_generation),
            last_event,
            sessions,
        }
    }
}

impl From<proto::WifiSource> for WifiSource {
    fn from(source: proto::WifiSource) -> Self {
        match source {
            proto::WifiSource::Build => Self::Build,
            proto::WifiSource::Stored => Self::Stored,
            proto::WifiSource::Fallback => Self::Fallback,
        }
    }
}

impl From<proto::Power> for Power {
    fn from(power: proto::Power) -> Self {
        let proto::Power {
            adapter,
            battery_mv,
        } = power;
        Self {
            adapter,
            battery_mv,
        }
    }
}

impl From<proto::Settings> for Settings {
    fn from(settings: proto::Settings) -> Self {
        let proto::Settings {
            generation,
            lights,
            sound,
            volume,
            writable,
        } = settings;
        Self {
            generation: Generation(generation),
            lights,
            sound,
            volume,
            writable,
        }
    }
}

impl From<proto::EventBody> for EventBody {
    fn from(body: proto::EventBody) -> Self {
        match body {
            proto::EventBody::Boot { bank, reset_flags } => Self::Boot { bank, reset_flags },
            proto::EventBody::Lock { locked, by } => Self::Lock {
                locked,
                by: by.into(),
            },
            proto::EventBody::Settings { generation, by } => Self::Settings {
                generation: Generation(generation),
                by: by.into(),
            },
            proto::EventBody::Wifi { generation, by } => Self::Wifi {
                generation: Generation(generation),
                by: by.into(),
            },
            proto::EventBody::Enrolled { client, role, path } => Self::Enrolled {
                client: Hex(client),
                role: role.into(),
                path: path.into(),
            },
            proto::EventBody::Revoked { client, by } => Self::Revoked {
                client: Hex(client),
                by: by.into(),
            },
            proto::EventBody::ClockSync { by, jump_ms } => Self::ClockSync {
                by: by.into(),
                jump_ms,
            },
            proto::EventBody::Lost { count } => Self::Lost { count },
            proto::EventBody::Fed {
                feed,
                requested,
                completed,
                outcome,
                reason,
                by,
            } => Self::Fed {
                feed,
                requested,
                completed,
                outcome: outcome.into(),
                reason: reason.into(),
                by: by.into(),
            },
        }
    }
}

/// The wire form, which is also how the server stores a body: the
/// feeder's own encoding, so a stored history decodes the way the feeder
/// meant it.
impl From<EventBody> for proto::EventBody {
    fn from(body: EventBody) -> Self {
        match body {
            EventBody::Boot { bank, reset_flags } => Self::Boot { bank, reset_flags },
            EventBody::Lock { locked, by } => Self::Lock {
                locked,
                by: by.into(),
            },
            EventBody::Settings { generation, by } => Self::Settings {
                generation: generation.0,
                by: by.into(),
            },
            EventBody::Wifi { generation, by } => Self::Wifi {
                generation: generation.0,
                by: by.into(),
            },
            EventBody::Enrolled { client, role, path } => Self::Enrolled {
                client: client.0,
                role: role.into(),
                path: path.into(),
            },
            EventBody::Revoked { client, by } => Self::Revoked {
                client: client.0,
                by: by.into(),
            },
            EventBody::ClockSync { by, jump_ms } => Self::ClockSync {
                by: by.into(),
                jump_ms,
            },
            EventBody::Lost { count } => Self::Lost { count },
            EventBody::Fed {
                feed,
                requested,
                completed,
                outcome,
                reason,
                by,
            } => Self::Fed {
                feed,
                requested,
                completed,
                outcome: outcome.into(),
                reason: reason.into(),
                by: by.into(),
            },
        }
    }
}

impl From<proto::Source> for Source {
    fn from(source: proto::Source) -> Self {
        match source {
            proto::Source::Panel => Self::Panel,
            proto::Source::Client(client) => Self::Client {
                client: Hex(client),
            },
            proto::Source::Feeder => Self::Feeder,
        }
    }
}

impl From<Source> for proto::Source {
    fn from(source: Source) -> Self {
        match source {
            Source::Panel => Self::Panel,
            Source::Client { client } => Self::Client(client.0),
            Source::Feeder => Self::Feeder,
        }
    }
}

impl From<proto::Role> for ClientRole {
    fn from(role: proto::Role) -> Self {
        match role {
            proto::Role::Owner => Self::Owner,
            proto::Role::Member => Self::Member,
            proto::Role::Server => Self::Server,
            proto::Role::UpdateSigner => Self::UpdateSigner,
        }
    }
}

impl From<ClientRole> for proto::Role {
    fn from(role: ClientRole) -> Self {
        match role {
            ClientRole::Owner => Self::Owner,
            ClientRole::Member => Self::Member,
            ClientRole::Server => Self::Server,
            ClientRole::UpdateSigner => Self::UpdateSigner,
        }
    }
}

impl From<proto::Path> for EnrollPath {
    fn from(path: proto::Path) -> Self {
        match path {
            proto::Path::Setup => Self::Setup,
            proto::Path::Invite => Self::Invite,
            proto::Path::Recovery => Self::Recovery,
        }
    }
}

impl From<EnrollPath> for proto::Path {
    fn from(path: EnrollPath) -> Self {
        match path {
            EnrollPath::Setup => Self::Setup,
            EnrollPath::Invite => Self::Invite,
            EnrollPath::Recovery => Self::Recovery,
        }
    }
}

impl From<proto::FeedOutcome> for FeedOutcome {
    fn from(outcome: proto::FeedOutcome) -> Self {
        match outcome {
            proto::FeedOutcome::Done => Self::Done,
            proto::FeedOutcome::Cancelled => Self::Cancelled,
            proto::FeedOutcome::Expired => Self::Expired,
            proto::FeedOutcome::Failed => Self::Failed,
            proto::FeedOutcome::Uncertain => Self::Uncertain,
        }
    }
}

impl From<FeedOutcome> for proto::FeedOutcome {
    fn from(outcome: FeedOutcome) -> Self {
        match outcome {
            FeedOutcome::Done => Self::Done,
            FeedOutcome::Cancelled => Self::Cancelled,
            FeedOutcome::Expired => Self::Expired,
            FeedOutcome::Failed => Self::Failed,
            FeedOutcome::Uncertain => Self::Uncertain,
        }
    }
}

impl From<proto::FeedReason> for FeedReason {
    fn from(reason: proto::FeedReason) -> Self {
        match reason {
            proto::FeedReason::Done => Self::Done,
            proto::FeedReason::Cancelled => Self::Cancelled,
            proto::FeedReason::Clock => Self::Clock,
            proto::FeedReason::ObservationGap => Self::ObservationGap,
            proto::FeedReason::Timeout => Self::Timeout,
            proto::FeedReason::NoFeedback => Self::NoFeedback,
            proto::FeedReason::Power => Self::Power,
            proto::FeedReason::Sensor => Self::Sensor,
            proto::FeedReason::Outlet => Self::Outlet,
            proto::FeedReason::Overcurrent => Self::Overcurrent,
            proto::FeedReason::FeedbackStream => Self::FeedbackStream,
            proto::FeedReason::Output => Self::Output,
        }
    }
}

impl From<FeedReason> for proto::FeedReason {
    fn from(reason: FeedReason) -> Self {
        match reason {
            FeedReason::Done => Self::Done,
            FeedReason::Cancelled => Self::Cancelled,
            FeedReason::Clock => Self::Clock,
            FeedReason::ObservationGap => Self::ObservationGap,
            FeedReason::Timeout => Self::Timeout,
            FeedReason::NoFeedback => Self::NoFeedback,
            FeedReason::Power => Self::Power,
            FeedReason::Sensor => Self::Sensor,
            FeedReason::Outlet => Self::Outlet,
            FeedReason::Overcurrent => Self::Overcurrent,
            FeedReason::FeedbackStream => Self::FeedbackStream,
            FeedReason::Output => Self::Output,
        }
    }
}

impl From<proto::Sample> for Sample {
    fn from(sample: proto::Sample) -> Self {
        let proto::Sample { at_ms, metric } = sample;
        Self {
            uptime_ms: at_ms,
            value: metric.into(),
        }
    }
}

impl From<proto::Metric> for SampleValue {
    fn from(metric: proto::Metric) -> Self {
        match metric {
            proto::Metric::Power(proto::Power {
                adapter,
                battery_mv,
            }) => Self::Power {
                adapter,
                battery_mv,
            },
            proto::Metric::Rssi(dbm) => Self::Rssi { dbm },
        }
    }
}

/// The wire form, which is also how the server stores a sample's value.
impl From<SampleValue> for proto::Metric {
    fn from(value: SampleValue) -> Self {
        match value {
            SampleValue::Power {
                adapter,
                battery_mv,
            } => Self::Power(proto::Power {
                adapter,
                battery_mv,
            }),
            SampleValue::Rssi { dbm } => Self::Rssi(dbm),
        }
    }
}

impl From<proto::Outcome<'_>> for Answer {
    fn from(outcome: proto::Outcome<'_>) -> Self {
        match outcome {
            proto::Outcome::Done { generation } => Self::Done {
                generation: generation.map(Generation),
            },
            proto::Outcome::Status(status) => Self::Status(status.into()),
            proto::Outcome::Settings(settings) => Self::Settings(settings.into()),
            proto::Outcome::Clients(proto::Clients { clients }) => Self::Clients(
                clients
                    .iter()
                    .flatten()
                    .map(
                        |&proto::Client {
                             key,
                             label,
                             role,
                             you,
                         }| Client {
                            key: Hex(key),
                            label: label.to_owned(),
                            role: role.into(),
                            you,
                        },
                    )
                    .collect(),
            ),
            proto::Outcome::Invite(proto::Invite {
                id,
                secret,
                seconds,
            }) => Self::Invite(Invite {
                id,
                secret: Hex(secret),
                seconds,
            }),
            proto::Outcome::Refused(refusal) => Self::Refused(refusal.into()),
            proto::Outcome::Feeding { feed } => Self::Feeding { feed },
            proto::Outcome::Update(view) => Self::Update(view.into()),
            proto::Outcome::Time(view) => Self::Time(view.into()),
        }
    }
}

impl From<proto::TimeView<'_>> for TimeView {
    fn from(view: proto::TimeView<'_>) -> Self {
        let proto::TimeView {
            generation,
            writable,
            zone,
            ntp,
            clock_ms,
            error_ms,
            offset_s,
            rate_ppb,
            rate_bound_ppb,
            learned,
            syncs,
            last,
            last_age_s,
            ntp_last,
        } = view;
        Self {
            generation: Generation(generation),
            writable,
            zone: (!zone.is_empty()).then(|| zone.to_owned()),
            ntp: ntp.into(),
            clock_ms,
            error_ms,
            offset_s,
            rate_ppb,
            rate_bound_ppb,
            learned,
            syncs,
            last: last.map(|source| match source {
                proto::TimeSource::Owner => TimeSource::Owner,
                proto::TimeSource::Client => TimeSource::Client,
                proto::TimeSource::Ntp => TimeSource::Ntp,
            }),
            last_age_s,
            ntp_last: ntp_last.map(|result| match result {
                proto::NtpResult::Used => NtpResult::Used,
                proto::NtpResult::Bounded => NtpResult::Bounded,
                proto::NtpResult::Lost => NtpResult::Lost,
                proto::NtpResult::Refused => NtpResult::Refused,
            }),
        }
    }
}

impl From<proto::NtpServer> for NtpServer {
    fn from(ntp: proto::NtpServer) -> Self {
        match ntp {
            proto::NtpServer::Off => Self::Off,
            proto::NtpServer::Gateway { port } => Self::Gateway { port },
            proto::NtpServer::Address { ip, port } => Self::Address {
                ip: ip.into(),
                port,
            },
        }
    }
}

impl From<NtpServer> for proto::NtpServer {
    fn from(ntp: NtpServer) -> Self {
        match ntp {
            NtpServer::Off => Self::Off,
            NtpServer::Gateway { port } => Self::Gateway { port },
            NtpServer::Address { ip, port } => Self::Address {
                ip: ip.octets(),
                port,
            },
        }
    }
}

impl From<proto::UpdateView> for UpdateView {
    fn from(view: proto::UpdateView) -> Self {
        let proto::UpdateView {
            phase,
            bank,
            min_version,
            digest,
            version,
            image_len,
            written,
            wanted,
            stopped,
        } = view;
        Self {
            phase: phase.into(),
            bank,
            min_version,
            digest: digest.map(Hex),
            version,
            image_len,
            written,
            wanted: wanted.into_iter().flatten().collect(),
            stopped: stopped.map(UpdateStop::from),
        }
    }
}

impl From<proto::UpdatePhase> for UpdatePhase {
    fn from(phase: proto::UpdatePhase) -> Self {
        match phase {
            proto::UpdatePhase::Idle => Self::Idle,
            proto::UpdatePhase::Checking => Self::Checking,
            proto::UpdatePhase::GivingWay => Self::GivingWay,
            proto::UpdatePhase::Receiving => Self::Receiving,
            proto::UpdatePhase::Activated => Self::Activated,
        }
    }
}

impl From<proto::UpdateStop> for UpdateStop {
    fn from(stop: proto::UpdateStop) -> Self {
        match stop {
            proto::UpdateStop::Timeout => Self::Timeout,
            proto::UpdateStop::Cancelled => Self::Cancelled,
            proto::UpdateStop::Order => Self::Order,
            proto::UpdateStop::Marker => Self::Marker,
            proto::UpdateStop::Write => Self::Write,
            proto::UpdateStop::Hash => Self::Hash,
            proto::UpdateStop::Activate => Self::Activate,
            proto::UpdateStop::Store => Self::Store,
        }
    }
}

impl From<proto::UpdateRefusal> for UpdateRefusal {
    fn from(refusal: proto::UpdateRefusal) -> Self {
        match refusal {
            proto::UpdateRefusal::Manifest => Self::Manifest,
            proto::UpdateRefusal::Key => Self::Key,
            proto::UpdateRefusal::Revoked => Self::Revoked,
            proto::UpdateRefusal::Signature => Self::Signature,
            proto::UpdateRefusal::Target => Self::Target,
            proto::UpdateRefusal::Same => Self::Same,
            proto::UpdateRefusal::Downgrade => Self::Downgrade,
            proto::UpdateRefusal::Statement => Self::Statement,
            proto::UpdateRefusal::Pending => Self::Pending,
            proto::UpdateRefusal::Failed => Self::Failed,
            proto::UpdateRefusal::Fallback => Self::Fallback,
            proto::UpdateRefusal::Attempt => Self::Attempt,
            proto::UpdateRefusal::Installing => Self::Installing,
            proto::UpdateRefusal::NotInstalling => Self::NotInstalling,
            proto::UpdateRefusal::Store => Self::Store,
            proto::UpdateRefusal::Unconfirmed => Self::Unconfirmed,
        }
    }
}

impl From<proto::Refusal> for Refusal {
    fn from(refusal: proto::Refusal) -> Self {
        match refusal {
            proto::Refusal::Forbidden => Self::Forbidden,
            proto::Refusal::Stale { current } => Self::Stale {
                current: Generation(current),
            },
            proto::Refusal::Busy => Self::Busy,
            proto::Refusal::Unavailable => Self::Unavailable,
            proto::Refusal::Invalid => Self::Invalid,
            proto::Refusal::NotWritable => Self::NotWritable,
            proto::Refusal::Full => Self::Full,
            proto::Refusal::NotFound => Self::NotFound,
            proto::Refusal::Muted => Self::Muted,
            proto::Refusal::Time => Self::Time,
            proto::Refusal::Fault => Self::Fault,
            proto::Refusal::Update(refusal) => Self::Update(refusal.into()),
        }
    }
}

impl Op {
    /// The wire form, borrowing this op's strings.
    pub fn to_proto(&self) -> proto::Op<'_> {
        match self {
            Self::Status => proto::Op::Status,
            Self::GetSettings => proto::Op::GetSettings,
            Self::PutSettings { base, changes } => proto::Op::PutSettings {
                base: base.0,
                changes: (*changes).into(),
            },
            Self::PutWifi { base, wifi } => proto::Op::PutWifi {
                base: base.0,
                wifi: wifi.as_ref().map(|wifi| proto::Wifi {
                    ssid: &wifi.ssid,
                    psk: &wifi.psk,
                }),
            },
            Self::SetLock { locked } => proto::Op::SetLock { locked: *locked },
            Self::Play { tune } => proto::Op::Play {
                tune: (*tune).into(),
            },
            Self::TimeSync(sync) => proto::Op::TimeSync((*sync).into()),
            Self::CreateInvite { role } => proto::Op::CreateInvite {
                role: (*role).into(),
            },
            Self::ListClients => proto::Op::ListClients,
            Self::RevokeClient { key } => proto::Op::RevokeClient { key: *key },
            Self::Feed { portions } => proto::Op::Feed {
                portions: *portions,
            },
            Self::Reboot => proto::Op::Reboot,
            Self::CancelFeed { feed } => proto::Op::CancelFeed { feed: *feed },
            Self::OfferUpdate {
                manifest,
                downgrade,
            } => proto::Op::OfferUpdate {
                manifest,
                downgrade: downgrade.as_deref(),
            },
            Self::UpdateChunk { offset, data } => proto::Op::UpdateChunk {
                offset: *offset,
                data,
            },
            Self::UpdateState => proto::Op::UpdateState,
            Self::CancelUpdate => proto::Op::CancelUpdate,
            Self::EnrollUpdateKey { key, label } => proto::Op::EnrollUpdateKey { key: *key, label },
            Self::UpdateKeyStatement { statement } => proto::Op::UpdateKeyStatement { statement },
            Self::GetTime => proto::Op::GetTime,
            Self::PutTime { base, changes } => proto::Op::PutTime {
                base: base.0,
                changes: proto::TimeChanges {
                    zone: changes.zone.as_deref(),
                    ntp: changes.ntp.map(Into::into),
                },
            },
        }
    }
}

impl From<SettingsPatch> for proto::SettingsChanges {
    fn from(patch: SettingsPatch) -> Self {
        let SettingsPatch {
            lights,
            sound,
            volume,
        } = patch;
        Self {
            lights,
            sound,
            volume,
        }
    }
}

impl From<Tune> for proto::Tune {
    fn from(tune: Tune) -> Self {
        match tune {
            Tune::Chime => Self::Chime,
            Tune::Meal => Self::Meal,
            Tune::Coin => Self::Coin,
            Tune::Alert => Self::Alert,
            Tune::Test => Self::Test,
            Tune::Twinkle => Self::Twinkle,
        }
    }
}

impl From<TimeSync> for proto::TimeSync {
    fn from(sync: TimeSync) -> Self {
        let TimeSync {
            unix_ms,
            offset_s,
            next_change_s,
            next_offset_s,
        } = sync;
        Self {
            unix_ms,
            offset_s,
            next_change_s,
            next_offset_s,
        }
    }
}

impl From<proto::Capabilities> for Capabilities {
    fn from(bits: proto::Capabilities) -> Self {
        Self {
            feed: bits.contains(proto::Capabilities::FEED),
            update: bits.contains(proto::Capabilities::UPDATE),
            time: bits.contains(proto::Capabilities::TIME),
        }
    }
}

impl From<Capabilities> for proto::Capabilities {
    fn from(capabilities: Capabilities) -> Self {
        let Capabilities { feed, update, time } = capabilities;
        let mut bits = proto::Capabilities::V1.0;
        if feed {
            bits |= proto::Capabilities::FEED.0;
        }
        if update {
            bits |= proto::Capabilities::UPDATE.0;
        }
        if time {
            bits |= proto::Capabilities::TIME.0;
        }
        Self(bits)
    }
}

impl From<Hello> for proto::Hello {
    fn from(hello: Hello) -> Self {
        let Hello {
            major,
            capabilities,
        } = hello;
        Self {
            major,
            capabilities: capabilities.into(),
        }
    }
}

impl From<proto::OpenRefusal> for OpenRefusal {
    fn from(refusal: proto::OpenRefusal) -> Self {
        match refusal {
            proto::OpenRefusal::Unknown => Self::Unknown,
            proto::OpenRefusal::Version { supported } => Self::Version { supported },
            proto::OpenRefusal::Full => Self::Full,
        }
    }
}

impl From<proto::Joined> for Joined {
    fn from(joined: proto::Joined) -> Self {
        match joined {
            proto::Joined::Enrolled { role } => Self::Enrolled { role: role.into() },
            proto::Joined::Refused(refusal) => Self::Refused(refusal.into()),
        }
    }
}

/// A setup reply a client acts on; `None` for a step of the code proof,
/// which the client's session answers itself.
pub fn setup_reply(reply: proto_setup::Reply) -> Option<setup::Reply> {
    Some(match reply {
        proto_setup::Reply::EnterCode { seconds } => setup::Reply::EnterCode { seconds },
        proto_setup::Reply::Round { .. } => return None,
        proto_setup::Reply::Done { enrolled, wifi } => setup::Reply::Done {
            enrolled: match enrolled {
                proto_setup::Enrolled::New => setup::Enrolled::New,
                proto_setup::Enrolled::Already => setup::Enrolled::Already,
            },
            wifi: match wifi {
                proto_setup::WifiSaved::Saved => setup::WifiSaved::Saved,
                proto_setup::WifiSaved::Unchanged => setup::WifiSaved::Unchanged,
            },
        },
        proto_setup::Reply::Refused(refusal) => setup::Reply::Refused(match refusal {
            proto_setup::Refusal::Busy => setup::Refusal::Busy,
            proto_setup::Refusal::Unconfirmed => setup::Refusal::Unconfirmed,
            proto_setup::Refusal::WrongCode => setup::Refusal::WrongCode,
            proto_setup::Refusal::Full => setup::Refusal::Full,
            proto_setup::Refusal::NotWritable => setup::Refusal::NotWritable,
            proto_setup::Refusal::Invalid => setup::Refusal::Invalid,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIENT: [u8; 4] = [0xde, 0xad, 0xbe, 0xef];

    /// Every body, so a round trip covers each variant's fields.
    fn bodies() -> Vec<proto::EventBody> {
        let by = proto::Source::Client(CLIENT);
        vec![
            proto::EventBody::Boot {
                bank: 1,
                reset_flags: 0x40,
            },
            proto::EventBody::Lock { locked: true, by },
            proto::EventBody::Settings {
                generation: 7,
                by: proto::Source::Panel,
            },
            proto::EventBody::Wifi {
                generation: 2,
                by: proto::Source::Feeder,
            },
            proto::EventBody::Enrolled {
                client: CLIENT,
                role: proto::Role::UpdateSigner,
                path: proto::Path::Recovery,
            },
            proto::EventBody::Revoked { client: CLIENT, by },
            proto::EventBody::ClockSync {
                by,
                jump_ms: Some(-5),
            },
            proto::EventBody::Lost { count: 3 },
            proto::EventBody::Fed {
                feed: 9,
                requested: 2,
                completed: 1,
                outcome: proto::FeedOutcome::Uncertain,
                reason: proto::FeedReason::FeedbackStream,
                by,
            },
        ]
    }

    #[test]
    fn a_stored_body_decodes_to_the_body_received() {
        for body in bodies() {
            let model = EventBody::from(body);
            assert_eq!(proto::EventBody::from(model), body);
        }
    }

    #[test]
    fn a_stored_sample_value_decodes_to_the_value_received() {
        for metric in [
            proto::Metric::Power(proto::Power {
                adapter: false,
                battery_mv: Some(4100),
            }),
            proto::Metric::Rssi(-61),
        ] {
            assert_eq!(proto::Metric::from(SampleValue::from(metric)), metric);
        }
    }

    #[test]
    fn a_body_reads_as_the_http_api_spells_it() {
        let body = EventBody::from(proto::EventBody::Enrolled {
            client: CLIENT,
            role: proto::Role::Owner,
            path: proto::Path::Invite,
        });
        assert_eq!(
            serde_json::to_value(body).unwrap(),
            serde_json::json!({
                "kind": "enrolled",
                "client": "deadbeef",
                "role": "owner",
                "path": "invite",
            })
        );
    }

    #[test]
    fn a_clock_past_jiffs_range_reads_as_unknown() {
        let status = proto::Status {
            build: "b",
            bank: 0,
            uptime_ms: 0,
            boot_count: 0,
            reset_flags: 0,
            faults: 0,
            rssi: 0,
            wifi: proto::WifiSource::Build,
            link_losses: 0,
            dns_losses: 0,
            clock_ms: Some(u64::MAX),
            power: None,
            locked: None,
            settings_generation: 0,
            wifi_generation: 0,
            last_event: None,
            sessions: 1,
        };
        assert_eq!(Status::from(status).clock, None);
    }

    #[test]
    fn a_client_list_keeps_only_the_enrolled_slots_in_order() {
        let mut clients = [None; proto::CLIENTS_MAX];
        let client = |label, you| proto::Client {
            key: [1; 32],
            label,
            role: proto::Role::Member,
            you,
        };
        clients[1] = Some(client("phone", true));
        clients[4] = Some(client("server", false));

        let Answer::Clients(listed) =
            Answer::from(proto::Outcome::Clients(proto::Clients { clients }))
        else {
            panic!("a client list");
        };
        let labels: Vec<_> = listed.iter().map(|c| (c.label.as_str(), c.you)).collect();
        assert_eq!(labels, [("phone", true), ("server", false)]);
    }

    #[test]
    fn an_update_view_keeps_the_wanted_offsets_in_order_and_reads_as_hex() {
        let mut wanted = [None; proto::WANTED_MAX];
        wanted[0] = Some(4096);
        wanted[1] = Some(4608);
        let view = proto::UpdateView {
            phase: proto::UpdatePhase::Receiving,
            bank: 0,
            min_version: 3,
            digest: Some([0xab; 32]),
            version: 4,
            image_len: 8192,
            written: 4096,
            wanted,
            stopped: None,
        };

        let Answer::Update(view) = Answer::from(proto::Outcome::Update(view)) else {
            panic!("an update view");
        };
        assert_eq!(view.wanted, [4096, 4608]);
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["phase"], "receiving");
        assert_eq!(json["digest"], "ab".repeat(32));
    }

    #[test]
    fn capabilities_keep_their_bits_and_a_session_runs_on_both_sides() {
        for capabilities in [Capabilities::V1, Capabilities::KNOWN] {
            let bits = proto::Capabilities::from(capabilities);
            assert_eq!(Capabilities::from(bits), capabilities);
        }
        assert_eq!(
            proto::Capabilities::from(Capabilities::KNOWN),
            proto::Capabilities::KNOWN
        );
        assert_eq!(Capabilities::KNOWN.both(Capabilities::V1), Capabilities::V1);
    }

    #[test]
    fn a_proof_round_is_no_reply_a_client_acts_on() {
        let round = proto_setup::Reply::Round {
            open: None,
            commit: Some([2; 32]),
        };
        assert_eq!(setup_reply(round), None);
        assert_eq!(
            setup_reply(proto_setup::Reply::Refused(proto_setup::Refusal::WrongCode)),
            Some(setup::Reply::Refused(setup::Refusal::WrongCode))
        );
    }

    #[test]
    fn an_op_borrows_its_credentials_for_the_wire() {
        let op = Op::PutWifi {
            base: Generation(4),
            wifi: Some(crate::Wifi {
                ssid: b"home".to_vec(),
                psk: "password1".to_owned(),
            }),
        };
        assert_eq!(
            op.to_proto(),
            proto::Op::PutWifi {
                base: 4,
                wifi: Some(proto::Wifi {
                    ssid: b"home",
                    psk: "password1",
                }),
            }
        );
        assert!(!format!("{op:?}").contains("password1"));
    }
}
