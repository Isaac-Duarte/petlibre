//! One request: who may run it, what it asks of the device, and its
//! compact answer.

use super::handshake::{find, fingerprint, invite};
use super::memory::Remembered;
use super::{Phase, Server, Waiting, slot_id};
use crate::clients::{Label, Role};
use crate::mailbox::{
    Calibration, CalibrationResult, LockResult, MealReminder, NtpServer, Origin, PLANS, Plan,
    PlansResult, PlayResult, SettingsFields, SettingsResult, Table, TimeFields, TimeResult,
    TimeSettingsResult, Weekdays, WifiResult, Window, Zone,
};
use crate::pairing::{self, Pairing};
use crate::wifi::Credentials;
use crate::{Device, KeyChange, REBOOT_MS, Record, UpdateRequest};
use wellfed_core::tone::Tune as CoreTune;
use wellfed_proto::api::{
    self as proto, Bowl, Capabilities, ClientMessage, EventBody, Op, Readiness, Refusal, RequestId,
    Source, StatsSection, Tune, UPDATE_CHUNK_MAX, UpdateRefusal, UpdateView,
};

impl Server {
    /// Answer or queue one request. `None` when the answer waits on the
    /// device (it then goes out from [`Self::poll_pending`]).
    #[expect(clippy::too_many_arguments, reason = "one request's whole context")]
    pub(super) fn request<D: Device>(
        &mut self,
        index: usize,
        id: RequestId,
        needs: Capabilities,
        action: Action,
        now_ms: u32,
        device: &mut D,
        pairing: &mut Pairing,
        record: &mut impl FnMut(Record),
    ) -> Option<Reply> {
        let Phase::Open {
            key,
            role,
            capabilities,
            memory,
            ..
        } = self.slots[index].phase
        else {
            return None;
        };

        if action.writes()
            && let Some(remembered) = self.memories[memory].find(&id)
        {
            record(Record::Retried {
                slot: slot_id(index),
            });
            return Some(match remembered {
                Remembered::Done(generation) => Reply::Done(generation),
                Remembered::Refused(refusal) => Reply::Refused(refusal),
                Remembered::Invite { id } => pairing
                    .invite_secret(now_ms, id)
                    .map_or(Reply::Refused(Refusal::NotFound), |secret| {
                        Reply::Invite(invite(id, secret))
                    }),
                Remembered::Feeding { feed } => Reply::Feeding(feed),
                Remembered::Queued { feed } => {
                    return self.wait(index, id, action, Waiting::Feed { feed });
                }
                Remembered::Update => device
                    .update_view()
                    .map_or(Reply::Refused(Refusal::Unavailable), Reply::Update),
            });
        }
        if !allowed(role, action) {
            let outcome = Reply::Refused(Refusal::Forbidden);
            return Some(self.remember(memory, id, action, outcome));
        }
        // An op outside the bits both sides claim is one this pair cannot
        // share: the feeder does not serve it, or the client could not
        // read what it leads to.
        if !capabilities.contains(needs) || !device.capabilities().contains(needs) {
            let outcome = Reply::Refused(Refusal::Unavailable);
            return Some(self.remember(memory, id, action, outcome));
        }

        let by = Source::Client(fingerprint(&key));
        let outcome = match action {
            Action::Status => Reply::Status,
            Action::GetSettings => match device.settings(SettingsFields::default()) {
                SettingsResult::Current { view, .. } => Reply::Settings(proto::Settings {
                    generation: view.generation,
                    lights: view.lights,
                    sound: view.sound,
                    volume: view.volume,
                    lights_window: wire_window(view.lights_window),
                    sound_window: wire_window(view.sound_window),
                    desiccant_days: view.desiccant_days,
                    desiccant_reset: view.desiccant_reset,
                    meal_reminder: wire_reminder(view.meal_reminder),
                    writable: view.writable,
                }),
                SettingsResult::Busy => Reply::Refused(Refusal::Busy),
                SettingsResult::ReadOnly => Reply::Refused(Refusal::NotWritable),
            },
            Action::PutSettings { base, fields } => match self.put_settings(base, fields, device) {
                Ok(()) => return self.wait(index, id, action, Waiting::Settings),
                Err(refusal) => Reply::Refused(refusal),
            },
            Action::ResetDesiccant => match reset_desiccant(now_ms, device) {
                Ok(()) => return self.wait(index, id, action, Waiting::Settings),
                Err(refusal) => Reply::Refused(refusal),
            },
            Action::PutWifi { base, wifi } => match put_wifi(base, wifi, device) {
                Ok(()) => return self.wait(index, id, action, Waiting::Wifi),
                Err(refusal) => Reply::Refused(refusal),
            },
            Action::SetLock { locked } => match device.lock(locked) {
                LockResult::Current { .. } => {
                    device.record(now_ms, EventBody::Lock { locked, by });
                    Reply::Done(None)
                }
                LockResult::Busy => Reply::Refused(Refusal::Busy),
                LockResult::Unserved => Reply::Refused(Refusal::Unavailable),
            },
            Action::Play { tune } => match device.play(tune) {
                Some(PlayResult::Started) => Reply::Done(None),
                Some(PlayResult::Muted) => Reply::Refused(Refusal::Muted),
                Some(PlayResult::Busy) => Reply::Refused(Refusal::Busy),
                None => Reply::Refused(Refusal::Unavailable),
            },
            Action::TimeSync(fields) => match device.time_sync(fields, origin(role)) {
                TimeResult::Synced { jump_ms } => {
                    let jump_ms = jump_ms.map(saturate);
                    device.record(now_ms, EventBody::ClockSync { by, jump_ms });
                    Reply::Done(None)
                }
                TimeResult::Refused(_) => Reply::Refused(Refusal::Time),
            },
            Action::CreateInvite { role: invited } => {
                let mut secret = [0; 32];
                if !device.random(&mut secret) {
                    return Some(Reply::Refused(Refusal::Busy));
                }
                let event = pairing.invite(now_ms, role, invited, secret);
                record(Record::Pairing(event));
                match event {
                    pairing::Event::Invited { id: invite_id } => {
                        let remembered = Remembered::Invite { id: invite_id };
                        self.memories[memory].remember(id, remembered);
                        return Some(Reply::Invite(invite(invite_id, secret)));
                    }
                    pairing::Event::Refused {
                        refusal: pairing::Refusal::Full,
                        ..
                    } => Reply::Refused(Refusal::Full),
                    _ => Reply::Refused(Refusal::Forbidden),
                }
            }
            Action::ListClients => return Some(Reply::Clients),
            Action::RevokeClient { key: revoked } => {
                if find(&self.clients, &revoked).is_none() {
                    Reply::Refused(Refusal::NotFound)
                } else if let Some(ticket) = device.keys(KeyChange::Revoke { key: revoked }) {
                    let waiting = Waiting::Keys { ticket, revoked };
                    return self.wait(index, id, action, waiting);
                } else {
                    Reply::Refused(Refusal::Busy)
                }
            }
            Action::GetTime => match device.time_settings(None) {
                TimeSettingsResult::Current { .. } => Reply::Time,
                TimeSettingsResult::Busy => Reply::Refused(Refusal::Busy),
                TimeSettingsResult::ReadOnly | TimeSettingsResult::Unserved => {
                    Reply::Refused(Refusal::Unavailable)
                }
            },
            Action::PutTime { base, zone, ntp } => match put_time(base, zone, ntp, device) {
                Ok(digest) => return self.wait(index, id, action, Waiting::Time { digest }),
                Err(refusal) => Reply::Refused(refusal),
            },
            Action::GetCalibration => match device.calibration(None) {
                CalibrationResult::Current { .. } => Reply::Calibration,
                CalibrationResult::Busy => Reply::Refused(Refusal::Busy),
                CalibrationResult::ReadOnly | CalibrationResult::Unserved => {
                    Reply::Refused(Refusal::Unavailable)
                }
            },
            // A build runs the one bowl mode it was verified for: another
            // would turn the rotor a way no bench run has checked.
            Action::PutCalibration { bowl, .. } if bowl != device.bowl() => {
                Reply::Refused(Refusal::Unverified)
            }
            Action::PutCalibration {
                base, calibration, ..
            } => match put_calibration(base, calibration, device) {
                Ok(digest) => {
                    return self.wait(index, id, action, Waiting::Calibration { digest });
                }
                Err(refusal) => Reply::Refused(refusal),
            },
            Action::GetPlans => match device.plans(None) {
                PlansResult::Current { .. } => Reply::Plans,
                PlansResult::Busy => Reply::Refused(Refusal::Busy),
                PlansResult::ReadOnly | PlansResult::Unserved => {
                    Reply::Refused(Refusal::Unavailable)
                }
            },
            Action::PutPlans { base } => match self.put_plans(base, device) {
                Ok(revision) => return self.wait(index, id, action, Waiting::Plans { revision }),
                Err(refusal) => Reply::Refused(refusal),
            },
            Action::Invalid => Reply::Refused(Refusal::Invalid),
            // Each of these may write the update store; none goes in while
            // a feed is queued or driven. A cancel during an install is not
            // held off: no feed runs then.
            Action::OfferUpdate | Action::CancelUpdate | Action::UpdateKeyStatement
                if self.feeding || device.feed_active() =>
            {
                Reply::Refused(Refusal::Busy)
            }
            Action::OfferUpdate | Action::CancelUpdate | Action::UpdateKeyStatement => {
                match self.update_request(action, device) {
                    Ok(ticket) => return self.wait(index, id, action, Waiting::Update { ticket }),
                    Err(refusal) => Reply::Refused(refusal),
                }
            }
            Action::UpdateChunk { offset } => match self.update_chunk(offset, device) {
                Ok(view) => Reply::Update(view),
                Err(refusal) => Reply::Refused(refusal),
            },
            Action::UpdateState => match device.update_view() {
                Some(view) => Reply::Update(view),
                None => Reply::Refused(Refusal::Unavailable),
            },
            Action::EnrollUpdateKey { key, label } => {
                let event = pairing.signer(now_ms, role);
                record(Record::Pairing(event));
                match event {
                    pairing::Event::Confirming { .. } => {
                        // Only this offer's press counts: a decision left
                        // over from an earlier one must not admit it.
                        self.signer = None;
                        let waiting = Waiting::Signer { key, label };
                        return self.wait(index, id, action, waiting);
                    }
                    pairing::Event::Refused {
                        refusal: pairing::Refusal::Busy,
                        ..
                    } => Reply::Refused(Refusal::Busy),
                    _ => Reply::Refused(Refusal::Forbidden),
                }
            }
            // Named before anything else is checked, so the refusal says
            // what to change rather than when to try again.
            Action::Feed { portions } if !(1..=proto::PORTIONS_MAX).contains(&portions) => {
                Reply::Refused(Refusal::Portions { asked: portions })
            }
            // A restart is due: a feed now would be cut by it.
            Action::Feed { .. } if self.reboot_at.is_some() => Reply::Refused(Refusal::Busy),
            Action::Feed { portions } => match device.readiness() {
                Readiness::Blocked(blocked) => Reply::Refused(blocked.refusal()),
                Readiness::Ready => match device.feed(portions, by) {
                    Ok(feed) => {
                        self.memories[memory].remember(id, Remembered::Queued { feed });
                        return self.wait(index, id, action, Waiting::Feed { feed });
                    }
                    Err(refusal) => Reply::Refused(refusal),
                },
            },
            Action::CancelFeed { feed } => {
                if device.cancel_feed(feed) {
                    Reply::Done(None)
                } else {
                    Reply::Refused(Refusal::NotFound)
                }
            }
            // A restart now would leave the feed uncertain.
            Action::Reboot if device.feed_active() => Reply::Refused(Refusal::Busy),
            Action::Reboot => {
                self.reboot_at = Some(now_ms.wrapping_add(REBOOT_MS));
                Reply::Done(None)
            }
            Action::TailLog { after } => {
                if let Phase::Open { log, .. } = &mut self.slots[index].phase {
                    *log = Some(after);
                }
                Reply::Done(None)
            }
            // The stack walk pauses the scheduler; none during a feed.
            Action::Stats {
                section: StatsSection::Memory,
            } if self.feeding || device.feed_active() => Reply::Refused(Refusal::Busy),
            Action::Stats { section } => return Some(Reply::Stats(section)),
            Action::FeedTrace { feed, offset } => return Some(Reply::FeedTrace { feed, offset }),
            Action::ReadBank1 { offset } => return Some(Reply::Bank1 { offset }),
        };
        Some(self.remember(memory, id, action, outcome))
    }

    /// The update op in `plain`, read again for its bytes, queued with the
    /// device. Out of line: the decoded message is a frame of its own.
    #[inline(never)]
    fn update_request<D: Device>(&self, action: Action, device: &mut D) -> Result<u32, Refusal> {
        let message: ClientMessage<'_> =
            wellfed_proto::decode(&self.plain[..self.plain_len]).map_err(|_| Refusal::Invalid)?;
        let ClientMessage::Request { op, .. } = message else {
            return Err(Refusal::Invalid);
        };
        let request = match (action, op) {
            (
                Action::OfferUpdate,
                Op::OfferUpdate {
                    manifest,
                    downgrade,
                },
            ) => UpdateRequest::Offer {
                manifest,
                downgrade,
                device: self.identity_public.ok_or(Refusal::Busy)?,
            },
            (Action::UpdateKeyStatement, Op::UpdateKeyStatement { statement }) => {
                UpdateRequest::Statement { statement }
            }
            (Action::CancelUpdate, Op::CancelUpdate) => UpdateRequest::Cancel,
            _ => return Err(Refusal::Invalid),
        };
        device.update(request)
    }

    /// The chunk in `plain`, handed to the device.
    #[inline(never)]
    fn update_chunk<D: Device>(&self, offset: u32, device: &mut D) -> Result<UpdateView, Refusal> {
        let message: ClientMessage<'_> =
            wellfed_proto::decode(&self.plain[..self.plain_len]).map_err(|_| Refusal::Invalid)?;
        match message {
            ClientMessage::Request {
                op: Op::UpdateChunk { offset: at, data },
                ..
            } if at == offset => device.update_chunk(offset, data),
            _ => Err(Refusal::Invalid),
        }
    }

    /// Queue the table in `plain` if the plans are still at `base`; the
    /// revision it produces once saved. Out of line, as for an update: the
    /// table is read again from the plaintext rather than carried in the
    /// action.
    #[inline(never)]
    fn put_plans<D: Device>(&self, base: u32, device: &mut D) -> Result<u32, Refusal> {
        let message: ClientMessage<'_> =
            wellfed_proto::decode(&self.plain[..self.plain_len]).map_err(|_| Refusal::Invalid)?;
        let ClientMessage::Request {
            op: Op::PutPlans { plans, .. },
            ..
        } = message
        else {
            return Err(Refusal::Invalid);
        };
        let table = table(&plans)?;

        let view = match device.plans(None) {
            PlansResult::Current { pending: true, .. } | PlansResult::Busy => {
                return Err(Refusal::Busy);
            }
            PlansResult::ReadOnly => return Err(Refusal::NotWritable),
            PlansResult::Unserved => return Err(Refusal::Unavailable),
            PlansResult::Current { view, .. } => view,
        };
        if view.revision != base {
            return Err(Refusal::Stale {
                current: view.revision,
            });
        }

        match device.plans(Some(table)) {
            PlansResult::Current { pending: true, .. } => Ok(base.wrapping_add(1)),
            PlansResult::Current { .. } | PlansResult::Busy => Err(Refusal::Busy),
            PlansResult::ReadOnly => Err(Refusal::NotWritable),
            PlansResult::Unserved => Err(Refusal::Unavailable),
        }
    }

    fn put_settings<D: Device>(
        &mut self,
        base: u32,
        fields: SettingsFields,
        device: &mut D,
    ) -> Result<(), Refusal> {
        match device.settings(SettingsFields::default()) {
            SettingsResult::Current { pending: true, .. } | SettingsResult::Busy => {
                Err(Refusal::Busy)
            }
            SettingsResult::ReadOnly => Err(Refusal::NotWritable),
            SettingsResult::Current { view, .. } if view.generation != base => {
                Err(Refusal::Stale {
                    current: view.generation,
                })
            }
            SettingsResult::Current { .. } => match device.settings(fields) {
                SettingsResult::Current { pending: true, .. } => Ok(()),
                SettingsResult::Current { .. } | SettingsResult::Busy => Err(Refusal::Busy),
                SettingsResult::ReadOnly => Err(Refusal::NotWritable),
            },
        }
    }
}

/// Queue a desiccant replacement dated today. The day comes from the clock
/// alone: with no valid clock the request is refused rather than dated by
/// a guess. No `base`: a replacement is a fact about today, whatever the
/// settings hold, and the settings event it records names their new
/// generation.
fn reset_desiccant<D: Device>(now_ms: u32, device: &mut D) -> Result<(), Refusal> {
    let today = device.today(now_ms).ok_or(Refusal::NoClock)?;

    match device.settings(SettingsFields::default()) {
        SettingsResult::Current { pending: true, .. } | SettingsResult::Busy => {
            return Err(Refusal::Busy);
        }
        SettingsResult::ReadOnly => return Err(Refusal::NotWritable),
        SettingsResult::Current { .. } => {}
    }
    let fields = SettingsFields {
        desiccant_reset: Some(today),
        ..SettingsFields::default()
    };
    match device.settings(fields) {
        SettingsResult::Current { pending: true, .. } => Ok(()),
        SettingsResult::Current { .. } | SettingsResult::Busy => Err(Refusal::Busy),
        SettingsResult::ReadOnly => Err(Refusal::NotWritable),
    }
}

fn put_wifi<D: Device>(
    base: u32,
    credentials: Option<Credentials>,
    device: &mut D,
) -> Result<(), Refusal> {
    match device.wifi(None) {
        WifiResult::Current { pending: true, .. } | WifiResult::Busy => Err(Refusal::Busy),
        WifiResult::ReadOnly => Err(Refusal::NotWritable),
        WifiResult::Invalid => Err(Refusal::Invalid),
        WifiResult::Current { view, .. } if view.generation != base => Err(Refusal::Stale {
            current: view.generation,
        }),
        WifiResult::Current { .. } => match device.wifi(Some(credentials)) {
            WifiResult::Current { pending: true, .. } => Ok(()),
            WifiResult::Current { .. } | WifiResult::Busy => Err(Refusal::Busy),
            WifiResult::ReadOnly => Err(Refusal::NotWritable),
            WifiResult::Invalid => Err(Refusal::Invalid),
        },
    }
}

/// Queue the time settings `base` names with the changes applied; the
/// digest of what was asked for.
fn put_time<D: Device>(
    base: u32,
    zone: Option<Option<Zone>>,
    ntp: Option<NtpServer>,
    device: &mut D,
) -> Result<u32, Refusal> {
    let view = match device.time_settings(None) {
        TimeSettingsResult::Current { pending: true, .. } | TimeSettingsResult::Busy => {
            return Err(Refusal::Busy);
        }
        TimeSettingsResult::ReadOnly => return Err(Refusal::NotWritable),
        TimeSettingsResult::Unserved => return Err(Refusal::Unavailable),
        TimeSettingsResult::Current { view, .. } => view,
    };
    if view.generation != base {
        return Err(Refusal::Stale {
            current: view.generation,
        });
    }

    let mut settings = view.settings;
    if let Some(zone) = zone {
        settings.zone = zone;
    }
    if let Some(ntp) = ntp {
        settings.ntp = ntp;
    }
    match device.time_settings(Some(settings)) {
        TimeSettingsResult::Current { pending: true, .. } => Ok(settings.digest()),
        TimeSettingsResult::Current { .. } | TimeSettingsResult::Busy => Err(Refusal::Busy),
        TimeSettingsResult::ReadOnly => Err(Refusal::NotWritable),
        TimeSettingsResult::Unserved => Err(Refusal::Unavailable),
    }
}

/// Queue `calibration` if the store is still at `base`; the digest of what
/// was asked for.
fn put_calibration<D: Device>(
    base: u32,
    calibration: Calibration,
    device: &mut D,
) -> Result<u32, Refusal> {
    let view = match device.calibration(None) {
        CalibrationResult::Current { pending: true, .. } | CalibrationResult::Busy => {
            return Err(Refusal::Busy);
        }
        CalibrationResult::ReadOnly => return Err(Refusal::NotWritable),
        CalibrationResult::Unserved => return Err(Refusal::Unavailable),
        CalibrationResult::Current { view, .. } => view,
    };
    if view.generation != base {
        return Err(Refusal::Stale {
            current: view.generation,
        });
    }

    match device.calibration(Some(calibration)) {
        CalibrationResult::Current { pending: true, .. } => Ok(calibration.digest()),
        CalibrationResult::Current { .. } | CalibrationResult::Busy => Err(Refusal::Busy),
        CalibrationResult::ReadOnly => Err(Refusal::NotWritable),
        CalibrationResult::Unserved => Err(Refusal::Unavailable),
    }
}

/// An owner's sync may step the clock; anyone else's only narrows it.
const fn origin(role: Role) -> Origin {
    match role {
        Role::Owner => Origin::Owner,
        Role::Member | Role::Server | Role::UpdateSigner => Origin::Client,
    }
}

/// Whether `role` may run `action`. Owners do everything; members and the
/// server feed, change settings and the lock, and read; nobody else opens a
/// session at all.
const fn allowed(role: Role, action: Action) -> bool {
    match role {
        Role::Owner => true,
        Role::Member | Role::Server => !matches!(
            action,
            Action::PutWifi { .. }
                | Action::RevokeClient { .. }
                | Action::Reboot
                | Action::OfferUpdate
                | Action::UpdateChunk { .. }
                | Action::UpdateState
                | Action::CancelUpdate
                | Action::EnrollUpdateKey { .. }
                | Action::UpdateKeyStatement
                | Action::PutTime { .. }
                | Action::PutCalibration { .. }
                | Action::TailLog { .. }
                | Action::Stats {
                    section: StatsSection::Memory,
                }
                // The stock backup is the owner's copy to keep.
                | Action::ReadBank1 { .. }
        ),
        Role::UpdateSigner => false,
    }
}

pub const fn proto_role(role: Role) -> proto::Role {
    match role {
        Role::Owner => proto::Role::Owner,
        Role::Member => proto::Role::Member,
        Role::Server => proto::Role::Server,
        Role::UpdateSigner => proto::Role::UpdateSigner,
    }
}

const fn store_role(role: proto::Role) -> Role {
    match role {
        proto::Role::Owner => Role::Owner,
        proto::Role::Member => Role::Member,
        proto::Role::Server => Role::Server,
        proto::Role::UpdateSigner => Role::UpdateSigner,
    }
}

const fn core_tune(tune: Tune) -> CoreTune {
    match tune {
        Tune::Chime => CoreTune::Chime,
        Tune::Meal => CoreTune::Meal,
        Tune::Coin => CoreTune::Coin,
        Tune::Alert => CoreTune::Alert,
        Tune::Test => CoreTune::Test,
        Tune::Twinkle => CoreTune::Twinkle,
    }
}

/// What a request asks, owned: the decoded message borrows the plaintext
/// buffer the answer reuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Status,
    GetSettings,
    PutSettings {
        base: u32,
        fields: SettingsFields,
    },
    PutWifi {
        base: u32,
        wifi: Option<Credentials>,
    },
    SetLock {
        locked: bool,
    },
    Play {
        tune: CoreTune,
    },
    TimeSync(TimeFields),
    CreateInvite {
        role: Role,
    },
    ListClients,
    RevokeClient {
        key: [u8; 32],
    },
    Feed {
        portions: u8,
    },
    Reboot,
    CancelFeed {
        feed: u32,
    },
    OfferUpdate,
    UpdateChunk {
        offset: u32,
    },
    UpdateState,
    CancelUpdate,
    EnrollUpdateKey {
        key: [u8; 32],
        label: Label,
    },
    UpdateKeyStatement,
    GetTime,
    PutTime {
        base: u32,
        /// `Some(None)` clears the zone.
        zone: Option<Option<Zone>>,
        ntp: Option<NtpServer>,
    },
    TailLog {
        after: Option<u32>,
    },
    Stats {
        section: StatsSection,
    },
    FeedTrace {
        feed: u32,
        offset: u16,
    },
    GetCalibration,
    PutCalibration {
        base: u32,
        calibration: Calibration,
        bowl: Bowl,
    },
    ReadBank1 {
        offset: u32,
    },
    GetPlans,
    /// The table stays in the plaintext buffer, read again when the op
    /// runs.
    PutPlans {
        base: u32,
    },
    ResetDesiccant,
    /// A field failed the stores' checks before anything ran.
    Invalid,
}

impl From<Op<'_>> for Action {
    fn from(op: Op<'_>) -> Self {
        match op {
            Op::Status => Self::Status,
            Op::GetSettings => Self::GetSettings,
            Op::PutSettings { base, changes } => {
                let fields = SettingsFields {
                    lights: changes.lights,
                    sound: changes.sound,
                    volume: changes.volume,
                    lights_window: changes.lights_window.map(window),
                    sound_window: changes.sound_window.map(window),
                    desiccant_days: changes.desiccant_days,
                    desiccant_reset: None,
                    meal_reminder: changes.meal_reminder.map(reminder),
                };
                if !fields.valid() {
                    return Self::Invalid;
                }
                Self::PutSettings { base, fields }
            }
            Op::PutWifi { base, wifi: None } => Self::PutWifi { base, wifi: None },
            Op::PutWifi {
                base,
                wifi: Some(wifi),
            } => match Credentials::new(wifi.ssid, wifi.psk.as_bytes()) {
                Some(credentials) => Self::PutWifi {
                    base,
                    wifi: Some(credentials),
                },
                None => Self::Invalid,
            },
            Op::SetLock { locked } => Self::SetLock { locked },
            Op::Play { tune } => Self::Play {
                tune: core_tune(tune),
            },
            Op::TimeSync(sync) => Self::TimeSync(TimeFields {
                unix_ms: sync.unix_ms,
                offset_s: sync.offset_s,
                next_change_s: sync.next_change_s,
                next_offset_s: sync.next_offset_s,
            }),
            Op::CreateInvite { role } => Self::CreateInvite {
                role: store_role(role),
            },
            Op::ListClients => Self::ListClients,
            Op::RevokeClient { key } => Self::RevokeClient { key },
            Op::Feed { portions } => Self::Feed { portions },
            Op::Reboot => Self::Reboot,
            Op::CancelFeed { feed } => Self::CancelFeed { feed },
            // The bytes stay in the plaintext buffer, read again when the
            // op runs, so the action stays small.
            Op::OfferUpdate { .. } => Self::OfferUpdate,
            Op::UpdateChunk { offset, data } => {
                if data.is_empty() || data.len() > UPDATE_CHUNK_MAX {
                    return Self::Invalid;
                }
                Self::UpdateChunk { offset }
            }
            Op::UpdateState => Self::UpdateState,
            Op::CancelUpdate => Self::CancelUpdate,
            Op::EnrollUpdateKey { key, label } => match Label::new(label.as_bytes()) {
                Some(label) => Self::EnrollUpdateKey { key, label },
                None => Self::Invalid,
            },
            Op::UpdateKeyStatement { .. } => Self::UpdateKeyStatement,
            Op::GetTime => Self::GetTime,
            Op::PutTime { base, changes } => {
                let zone = match changes.zone {
                    None => None,
                    Some("") => Some(None),
                    Some(text) => match Zone::parse(text.as_bytes()) {
                        Ok(zone) => Some(Some(zone)),
                        Err(_) => return Self::Invalid,
                    },
                };
                let ntp = changes.ntp.map(|ntp| match ntp {
                    proto::NtpServer::Off => NtpServer::Off,
                    proto::NtpServer::Gateway { port } => NtpServer::Gateway { port },
                    proto::NtpServer::Address { ip, port } => NtpServer::Address { ip, port },
                });
                if matches!(
                    ntp,
                    Some(NtpServer::Gateway { port: 0 } | NtpServer::Address { port: 0, .. })
                ) {
                    return Self::Invalid;
                }
                Self::PutTime { base, zone, ntp }
            }
            Op::TailLog { after } => Self::TailLog { after },
            Op::Stats { section } => Self::Stats { section },
            Op::FeedTrace { feed, offset } => Self::FeedTrace { feed, offset },
            Op::GetCalibration => Self::GetCalibration,
            Op::PutCalibration { base, calibration } => {
                let bowl = calibration.bowl;
                let calibration = Calibration {
                    portion_mg: calibration.portion_mg,
                };
                if !calibration.valid() {
                    return Self::Invalid;
                }
                Self::PutCalibration {
                    base,
                    calibration,
                    bowl,
                }
            }
            Op::ReadBank1 { offset } => Self::ReadBank1 { offset },
            Op::GetPlans => Self::GetPlans,
            // Checked when it runs ([`table`]), which names a plan's
            // portions in its refusal.
            Op::PutPlans { base, .. } => Self::PutPlans { base },
            Op::ResetDesiccant => Self::ResetDesiccant,
        }
    }
}

impl Action {
    /// The chunk op's name, which the answer's record leaves out.
    pub(super) const CHUNK: &'static str = "update-chunk";

    /// Whether the request changes something, so its id is remembered.
    pub(super) const fn writes(self) -> bool {
        !matches!(
            self,
            Self::Status
                | Self::GetSettings
                | Self::ListClients
                | Self::Invalid
                | Self::UpdateState
                | Self::UpdateChunk { .. }
                | Self::GetTime
                | Self::TailLog { .. }
                | Self::Stats { .. }
                | Self::FeedTrace { .. }
                | Self::GetCalibration
                | Self::ReadBank1 { .. }
                | Self::GetPlans
        )
    }

    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::GetSettings => "get-settings",
            Self::PutSettings { .. } => "put-settings",
            Self::PutWifi { .. } => "put-wifi",
            Self::SetLock { .. } => "set-lock",
            Self::Play { .. } => "play",
            Self::TimeSync(_) => "time-sync",
            Self::CreateInvite { .. } => "create-invite",
            Self::ListClients => "list-clients",
            Self::RevokeClient { .. } => "revoke-client",
            Self::Feed { .. } => "feed",
            Self::Reboot => "reboot",
            Self::CancelFeed { .. } => "cancel-feed",
            Self::OfferUpdate => "offer-update",
            Self::UpdateChunk { .. } => Self::CHUNK,
            Self::UpdateState => "update-state",
            Self::CancelUpdate => "cancel-update",
            Self::EnrollUpdateKey { .. } => "enroll-update-key",
            Self::UpdateKeyStatement => "update-key-statement",
            Self::GetTime => "get-time",
            Self::PutTime { .. } => "put-time",
            Self::TailLog { .. } => "tail-log",
            Self::Stats { .. } => "stats",
            Self::FeedTrace { .. } => "feed-trace",
            Self::GetCalibration => "get-calibration",
            Self::PutCalibration { .. } => "put-calibration",
            Self::ReadBank1 { .. } => "read-bank1",
            Self::GetPlans => "get-plans",
            Self::PutPlans { .. } => "put-plans",
            Self::ResetDesiccant => "reset-desiccant",
            Self::Invalid => "invalid",
        }
    }
}

/// A request's answer, compact: the status, the client list and the
/// statistics are read only when the answer is sealed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reply {
    Done(Option<u32>),
    Refused(Refusal),
    Status,
    Settings(proto::Settings),
    Invite(proto::Invite),
    Clients,
    Feeding(u32),
    Update(UpdateView),
    /// The time view, read when the answer is sealed.
    Time,
    Stats(StatsSection),
    /// A trace's page, read when the answer is sealed.
    FeedTrace {
        feed: u32,
        offset: u16,
    },
    /// The calibration view, read when the answer is sealed.
    Calibration,
    /// A page of bank 1, read when the answer is sealed.
    Bank1 {
        offset: u32,
    },
    /// The plans, read when the answer is sealed.
    Plans,
}

fn saturate(jump_ms: i64) -> i32 {
    i32::try_from(jump_ms).unwrap_or(if jump_ms < 0 { i32::MIN } else { i32::MAX })
}

/// Names for records.
pub(super) trait Name {
    fn name(&self) -> &'static str;
}

impl Name for Reply {
    fn name(&self) -> &'static str {
        match self {
            Self::Done(_) => "done",
            Self::Status => "status",
            Self::Settings(_) => "settings",
            Self::Clients => "clients",
            Self::Invite(_) => "invite",
            Self::Feeding(_) => "feeding",
            Self::Update(_) => "update",
            Self::Time => "time",
            Self::Stats(_) => "stats",
            Self::FeedTrace { .. } => "feed-trace",
            Self::Calibration => "calibration",
            Self::Bank1 { .. } => "bank1",
            Self::Plans => "plans",
            Self::Refused(refusal) => match refusal {
                Refusal::Forbidden => "forbidden",
                Refusal::Stale { .. } => "stale",
                Refusal::Busy => "busy",
                Refusal::Unavailable => "unavailable",
                Refusal::Invalid => "invalid",
                Refusal::NotWritable => "not-writable",
                Refusal::Full => "full",
                Refusal::NotFound => "not-found",
                Refusal::Muted => "muted",
                Refusal::Time => "time",
                Refusal::Fault => "fault",
                Refusal::Unverified => "unverified",
                Refusal::Portions { .. } => "portions",
                Refusal::NoClock => "no-clock",
                Refusal::Update(refusal) => update_refusal(*refusal),
            },
        }
    }
}

fn update_refusal(refusal: UpdateRefusal) -> &'static str {
    match refusal {
        UpdateRefusal::Manifest => "update-manifest",
        UpdateRefusal::Key => "update-key",
        UpdateRefusal::Revoked => "update-revoked",
        UpdateRefusal::Signature => "update-signature",
        UpdateRefusal::Target => "update-target",
        UpdateRefusal::Same => "update-same",
        UpdateRefusal::Downgrade => "update-downgrade",
        UpdateRefusal::Statement => "update-statement",
        UpdateRefusal::Pending => "update-pending",
        UpdateRefusal::Failed => "update-failed",
        UpdateRefusal::Fallback => "update-fallback",
        UpdateRefusal::Attempt => "update-attempt",
        UpdateRefusal::Installing => "update-installing",
        UpdateRefusal::NotInstalling => "update-not-installing",
        UpdateRefusal::Store => "update-store",
        UpdateRefusal::Unconfirmed => "update-unconfirmed",
        UpdateRefusal::Battery => "update-battery",
    }
}

/// A window as the settings mailbox carries it.
fn window(window: proto::Window) -> Window {
    match window {
        proto::Window::Always => Window::Always,
        proto::Window::Between { start, end } => Window::Between { start, end },
    }
}

/// A window as the wire carries it.
fn wire_window(window: Window) -> proto::Window {
    match window {
        Window::Always => proto::Window::Always,
        Window::Between { start, end } => proto::Window::Between { start, end },
    }
}

fn reminder(reminder: proto::MealReminder) -> MealReminder {
    match reminder {
        proto::MealReminder::Off => MealReminder::Off,
        proto::MealReminder::Before { minutes } => MealReminder::Before { minutes },
    }
}

/// A meal reminder as the wire carries it.
fn wire_reminder(reminder: MealReminder) -> proto::MealReminder {
    match reminder {
        MealReminder::Off => proto::MealReminder::Off,
        MealReminder::Before { minutes } => proto::MealReminder::Before { minutes },
    }
}

const _: () = assert!(PLANS == proto::PLANS_MAX);
const _: () = assert!(wellfed_core::schedule::PORTIONS_MAX == proto::PORTIONS_MAX);
const _: () = assert!(
    wellfed_core::desiccant::PERIOD_MIN == proto::DESICCANT_DAYS_MIN
        && wellfed_core::desiccant::PERIOD_MAX == proto::DESICCANT_DAYS_MAX
        && wellfed_core::desiccant::PERIOD_DEFAULT == proto::DESICCANT_DAYS_DEFAULT
);
const _: () = assert!(
    wellfed_core::settings::REMINDER_MINUTES_MIN == proto::MEAL_REMINDER_MINUTES_MIN
        && wellfed_core::settings::REMINDER_MINUTES_MAX == proto::MEAL_REMINDER_MINUTES_MAX
);

/// The wire's plans as the store keeps them. A plan's portions outside
/// 1 to [`proto::PORTIONS_MAX`] are refused by name, as a feed's are; any
/// other field that fails its checks is `Invalid`.
fn table(plans: &[Option<proto::Plan>; PLANS]) -> Result<Table, Refusal> {
    let mut table: Table = [None; PLANS];
    for (slot, plan) in table.iter_mut().zip(plans) {
        *slot = match *plan {
            None => None,
            Some(plan) => {
                let plan = Plan {
                    minute: plan.minute,
                    portions: plan.portions,
                    days: Weekdays(plan.days),
                    enabled: plan.enabled,
                };
                if !(1..=proto::PORTIONS_MAX).contains(&plan.portions) {
                    return Err(Refusal::Portions {
                        asked: plan.portions,
                    });
                }
                if !plan.valid() {
                    return Err(Refusal::Invalid);
                }
                Some(plan)
            }
        };
    }
    Ok(table)
}

/// The store's plans as the wire carries them.
pub(super) fn wire_plans(table: &Table) -> [Option<proto::Plan>; PLANS] {
    table.map(|plan| {
        plan.map(|plan| proto::Plan {
            minute: plan.minute,
            portions: plan.portions,
            days: plan.days.0,
            enabled: plan.enabled,
        })
    })
}
