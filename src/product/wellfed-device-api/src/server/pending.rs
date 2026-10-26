//! Writes the device finishes on a later pass. The session keeps the
//! request pending and polls it each pass, then answers it and remembers
//! the answer once the device is done.

use super::handshake::{find, fingerprint};
use super::memory::remembered;
use super::request::{Action, Name, Reply};
use super::{Pending, Phase, Server, Waiting, slot_id};
use crate::clients::Role;
use crate::mailbox::{
    CalibrationResult, NtpResult, NtpServer, Origin, SettingsFields, SettingsResult,
    TimeSettingsResult, WifiResult,
};
use crate::pairing::{self, Path};
use crate::{Device, KeyChange, KeyOutcome, Reason, Record, UpdateAnswer, UpdateKeyChange};
use wellfed_core::ota::offer::key_id;
use wellfed_proto::api::{
    self as proto, Capabilities, EventBody, FeederMessage, Outcome, Readiness, Refusal, RequestId,
    Source, TimeView, UpdateRefusal,
};

impl Server {
    /// Hold the answer until the device finishes the write.
    pub(super) fn wait(
        &mut self,
        index: usize,
        id: RequestId,
        action: Action,
        waiting: Waiting,
    ) -> Option<Reply> {
        if let Phase::Open { pending, .. } = &mut self.slots[index].phase {
            *pending = Some(Pending {
                id,
                op: action.name(),
                waiting,
            });
        }
        None
    }

    /// A pending write's answer, once the device has finished it.
    pub(super) fn poll_pending<D: Device>(
        &mut self,
        index: usize,
        now_ms: u32,
        device: &mut D,
        record: &mut impl FnMut(Record),
    ) -> Result<Option<(RequestId, &'static str, Reply)>, Reason> {
        let Phase::Open {
            pending: Some(pending),
            key,
            memory,
            ..
        } = self.slots[index].phase
        else {
            return Ok(None);
        };
        let by = Source::Client(fingerprint(&key));

        let outcome = match pending.waiting {
            Waiting::Settings => match device.settings(SettingsFields::default()) {
                SettingsResult::Current {
                    view,
                    pending: false,
                } => {
                    let generation = view.generation;
                    device.record(now_ms, EventBody::Settings { generation, by });
                    Reply::Done(Some(generation))
                }
                _ => return Ok(None),
            },
            Waiting::Wifi => match device.wifi(None) {
                WifiResult::Current {
                    view,
                    pending: false,
                } => {
                    let generation = view.generation;
                    device.record(now_ms, EventBody::Wifi { generation, by });
                    Reply::Done(Some(generation))
                }
                WifiResult::Current { pending: true, .. } | WifiResult::Busy => return Ok(None),
                WifiResult::ReadOnly | WifiResult::Invalid => Reply::Refused(Refusal::NotWritable),
            },
            // No event: the history's settings events carry the settings
            // store's generation, and the time settings have their own. A
            // write the writer could not make (unarmed, or a failed save)
            // leaves the store as it was: refused, never `Done`.
            Waiting::Time { digest } => match device.time_settings(None) {
                TimeSettingsResult::Current {
                    view,
                    pending: false,
                } if view.settings.digest() == digest => Reply::Done(Some(view.generation)),
                TimeSettingsResult::Current { pending: false, .. } => {
                    Reply::Refused(Refusal::NotWritable)
                }
                TimeSettingsResult::Current { pending: true, .. } | TimeSettingsResult::Busy => {
                    return Ok(None);
                }
                TimeSettingsResult::ReadOnly | TimeSettingsResult::Unserved => {
                    Reply::Refused(Refusal::NotWritable)
                }
            },
            // No event either: a calibration has its own generation.
            Waiting::Calibration { digest } => match device.calibration(None) {
                CalibrationResult::Current {
                    view,
                    pending: false,
                } if view.calibration.digest() == digest => Reply::Done(Some(view.generation)),
                CalibrationResult::Current { pending: false, .. } => {
                    Reply::Refused(Refusal::NotWritable)
                }
                CalibrationResult::Current { pending: true, .. } | CalibrationResult::Busy => {
                    return Ok(None);
                }
                CalibrationResult::ReadOnly | CalibrationResult::Unserved => {
                    Reply::Refused(Refusal::NotWritable)
                }
            },
            Waiting::Keys { ticket, revoked } => match device.keys_outcome(ticket) {
                None => return Ok(None),
                Some(KeyOutcome::Done) => {
                    let body = EventBody::Revoked {
                        client: fingerprint(&revoked),
                        by,
                    };
                    device.record(now_ms, body);
                    Reply::Done(None)
                }
                Some(KeyOutcome::NotFound) => Reply::Refused(Refusal::NotFound),
                Some(_) => Reply::Refused(Refusal::NotWritable),
            },
            Waiting::Feed { feed } => match device.feed_reserved(feed) {
                None => return Ok(None),
                Some(Ok(())) => Reply::Feeding(feed),
                Some(Err(refusal)) => Reply::Refused(refusal),
            },
            Waiting::Update { ticket } => match device.update_outcome(ticket) {
                None => return Ok(None),
                Some(Ok(UpdateAnswer::View(view))) => Reply::Update(view),
                Some(Ok(UpdateAnswer::Key { change, id })) => {
                    record(Record::UpdateKey {
                        slot: slot_id(index),
                        change,
                        key: id,
                    });
                    Reply::Done(None)
                }
                Some(Err(refusal)) => Reply::Refused(refusal),
            },
            Waiting::Signer { key, label } => match self.signer {
                Some(pairing::Event::Admit {
                    path: Path::Signer, ..
                }) => {
                    let change = KeyChange::Enroll {
                        key,
                        label,
                        role: Role::UpdateSigner,
                    };
                    // The mailbox may be busy; the press is kept until it is not.
                    if let Some(ticket) = device.keys(change) {
                        self.signer = None;
                        self.waiting(index, Waiting::SignerKeys { ticket, key });
                    }
                    return Ok(None);
                }
                Some(pairing::Event::Refused {
                    path: Path::Signer, ..
                }) => {
                    self.signer = None;
                    Reply::Refused(Refusal::Update(UpdateRefusal::Unconfirmed))
                }
                _ => return Ok(None),
            },
            Waiting::SignerKeys { ticket, key } => match device.keys_outcome(ticket) {
                None => return Ok(None),
                // Already enrolled under another role: the key is a session
                // key, never an update signer, and stays what it was.
                Some(KeyOutcome::Duplicate)
                    if find(&self.clients, &key).is_some_and(|c| c.role != Role::UpdateSigner) =>
                {
                    Reply::Refused(Refusal::Invalid)
                }
                Some(KeyOutcome::Done | KeyOutcome::Duplicate) => {
                    record(Record::UpdateKey {
                        slot: slot_id(index),
                        change: UpdateKeyChange::Enrolled,
                        key: key_id(&key),
                    });
                    Reply::Done(None)
                }
                Some(KeyOutcome::Full) => Reply::Refused(Refusal::Full),
                Some(KeyOutcome::NotFound | KeyOutcome::NotWritable) => {
                    Reply::Refused(Refusal::NotWritable)
                }
            },
        };

        if let Phase::Open { pending, .. } = &mut self.slots[index].phase {
            *pending = None;
        }
        self.memories[memory].remember(pending.id, remembered(outcome));
        Ok(Some((pending.id, pending.op, outcome)))
    }

    /// Whether the session on `index` and the feeder both claim `bit`.
    fn shares<D: Device>(&self, index: usize, bit: Capabilities, device: &D) -> bool {
        let Phase::Open { capabilities, .. } = self.slots[index].phase else {
            return false;
        };
        capabilities.contains(bit) && device.capabilities().contains(bit)
    }

    /// The bits the session at `index` claimed in its hello; none before
    /// it is open.
    fn claimed(&self, index: usize) -> Capabilities {
        match self.slots[index].phase {
            Phase::Open { capabilities, .. } => capabilities,
            _ => Capabilities::V1,
        }
    }

    /// Move a pending request on to its next wait.
    fn waiting(&mut self, index: usize, next: Waiting) {
        if let Phase::Open {
            pending: Some(pending),
            ..
        } = &mut self.slots[index].phase
        {
            pending.waiting = next;
        }
    }

    /// Seal one request's answer. Kept out of line: the wire message is as
    /// large as its largest variant, so only this frame holds one.
    #[inline(never)]
    #[expect(clippy::too_many_arguments, reason = "one answer's whole context")]
    pub(super) fn answer<D: Device>(
        &mut self,
        index: usize,
        id: RequestId,
        op: &'static str,
        reply: Reply,
        now_ms: u32,
        device: &mut D,
        record: &mut impl FnMut(Record),
    ) -> Result<(), Reason> {
        // A chunk that went in is not worth a line each: an image is about
        // two thousand of them, and the log would cost the loop seconds.
        if !(op == Action::CHUNK && matches!(reply, Reply::Update(_))) {
            record(Record::Request {
                slot: slot_id(index),
                op,
                result: reply.name(),
            });
        }
        // The time view's zone borrows this until the answer is sealed.
        let zone;
        let result = match reply {
            Reply::Clients => return self.list_clients(index, id),
            Reply::Done(generation) => Outcome::Done { generation },
            Reply::Refused(refusal) => Outcome::Refused(refusal),
            Reply::Settings(settings) => Outcome::Settings(settings),
            Reply::Invite(invite) => Outcome::Invite(invite),
            Reply::Feeding(feed) => Outcome::Feeding { feed },
            Reply::Update(view) => Outcome::Update(view),
            Reply::Status => {
                let mut status = device.status(now_ms);
                status.sessions = u8::try_from(self.sessions()).unwrap_or(u8::MAX);
                if self.shares(index, Capabilities::READINESS, device) {
                    let readiness = match device.readiness() {
                        Readiness::Blocked(blocked) => {
                            Readiness::Blocked(blocked.seen_by(self.claimed(index)))
                        }
                        Readiness::Ready => Readiness::Ready,
                    };
                    Outcome::FeederStatus { status, readiness }
                } else {
                    Outcome::Status(status)
                }
            }
            Reply::Time => match time_view(device, now_ms) {
                Some((view, text)) => {
                    zone = text;
                    Outcome::Time(TimeView {
                        zone: zone.as_str(),
                        ..view
                    })
                }
                None => Outcome::Refused(Refusal::Unavailable),
            },
            Reply::Calibration => match device.calibration(None) {
                CalibrationResult::Current { view, .. } => {
                    Outcome::Calibration(proto::CalibrationView {
                        generation: view.generation,
                        writable: view.writable,
                        calibration: proto::Calibration {
                            portion_mg: view.calibration.portion_mg,
                            bowl: device.bowl(),
                        },
                        phase: device.phase(),
                    })
                }
                _ => Outcome::Refused(Refusal::Unavailable),
            },
            Reply::Stats(section) => {
                let message = FeederMessage::Response {
                    id,
                    result: Outcome::Stats(device.stats(section, now_ms)),
                };
                return self.seal_and_send(index, &message);
            }
            Reply::FeedTrace { feed, offset } => {
                let result = match device.feed_trace(feed, offset) {
                    Ok(page) => Outcome::FeedTrace(page),
                    Err(refusal) => Outcome::Refused(refusal),
                };
                return self.seal_and_send(index, &FeederMessage::Response { id, result });
            }
        };
        self.seal_and_send(index, &FeederMessage::Response { id, result })
    }
}

/// The zone's text, copied out of the device's view for the answer to
/// borrow.
struct ZoneText {
    len: usize,
    bytes: [u8; wellfed_core::clock::zone::TEXT_MAX],
}

impl ZoneText {
    fn as_str(&self) -> &str {
        // The rule parser accepts ASCII only.
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

/// The time view, its zone left empty for the caller to fill from the
/// returned text.
fn time_view<D: Device>(device: &mut D, now_ms: u32) -> Option<(TimeView<'static>, ZoneText)> {
    let TimeSettingsResult::Current { view, .. } = device.time_settings(None) else {
        return None;
    };
    let state = device.time_state(now_ms)?;

    let mut text = ZoneText {
        len: 0,
        bytes: [0; wellfed_core::clock::zone::TEXT_MAX],
    };
    if let Some(zone) = view.settings.zone {
        let bytes = zone.text();
        text.bytes[..bytes.len()].copy_from_slice(bytes);
        text.len = bytes.len();
    }

    let clock = state.clock;
    let reading = clock.reading;
    let view = TimeView {
        generation: view.generation,
        writable: view.writable,
        zone: "",
        ntp: proto_ntp(view.settings.ntp),
        clock_ms: reading.map(|r| r.unix_ms),
        error_ms: reading.map(|r| u32::try_from(r.error_ms).unwrap_or(u32::MAX)),
        offset_s: reading.and_then(|r| r.offset_s),
        rate_ppb: clock.rate_ppb,
        rate_bound_ppb: clock.rate_bound_ppb,
        learned: clock.learned,
        syncs: clock.syncs,
        last: clock.last.map(|(origin, _)| match origin {
            Origin::Owner => proto::TimeSource::Owner,
            Origin::Client => proto::TimeSource::Client,
            Origin::Ntp => proto::TimeSource::Ntp,
        }),
        last_age_s: clock
            .last
            .map(|(_, at)| state.now_ms.wrapping_sub(at) / 1000),
        ntp_last: state.ntp_last.map(|result| match result {
            NtpResult::Used => proto::NtpResult::Used,
            NtpResult::Bounded => proto::NtpResult::Bounded,
            NtpResult::Lost => proto::NtpResult::Lost,
            NtpResult::Refused => proto::NtpResult::Refused,
        }),
    };
    Some((view, text))
}

const fn proto_ntp(ntp: NtpServer) -> proto::NtpServer {
    match ntp {
        NtpServer::Off => proto::NtpServer::Off,
        NtpServer::Gateway { port } => proto::NtpServer::Gateway { port },
        NtpServer::Address { ip, port } => proto::NtpServer::Address { ip, port },
    }
}
