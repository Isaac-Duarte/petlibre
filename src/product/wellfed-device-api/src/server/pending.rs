//! Writes the device finishes on a later pass. The session keeps the
//! request pending and polls it each pass, then answers it and remembers
//! the answer once the device is done.

use super::handshake::{find, fingerprint};
use super::memory::remembered;
use super::request::{Action, Name, Reply};
use super::{Pending, Phase, Server, Waiting, slot_id};
use crate::clients::Role;
use crate::mailbox::{SettingsFields, SettingsResult, WifiResult};
use crate::pairing::{self, Path};
use crate::{Device, KeyChange, KeyOutcome, Reason, Record, UpdateAnswer, UpdateKeyChange};
use wellfed_core::ota::offer::key_id;
use wellfed_proto::api::{
    EventBody, FeederMessage, Outcome, Refusal, RequestId, Source, UpdateRefusal,
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
                Outcome::Status(status)
            }
        };
        self.seal_and_send(index, &FeederMessage::Response { id, result })
    }
}
