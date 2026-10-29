//! An open session's turn each pass: finish a pending write, else read one
//! request, else push a frame of events, a sample or log lines. A redeemed invite's key is
//! enrolled here before its connection closes.

use super::handshake::fingerprint;
use super::recovery::Rescue;
use super::request::{Action, proto_role};
use super::{Phase, Server, proto_encode, slot_id};
use crate::pairing::Pairing;
use crate::{Device, EventRead, HANDSHAKE_MS, IDLE_MS, KeyChange, KeyOutcome, Reason, Record};
use wellfed_core::deadline_passed;
use wellfed_proto::api::{
    self as proto, ClientMessage, Clients, EventBatchWriter, EventBody, FeederMessage, Joined,
    LOG_TEXT_MAX, Open, Outcome, Refusal, RequestId,
};

/// The most events one pass reads for a session. A batch fills a frame in
/// fewer from a device whose reads come in batches; the bound keeps a pass
/// short where reads never wait and the session skips every body it meets.
const READS_PER_PASS: usize = 64;

/// What a pass's events came to.
enum Push {
    /// A batch or a gap is sealed.
    Sent,
    /// Nothing sealed, but the events are not done: a read is on its way,
    /// or the pass skipped bodies the session cannot decode.
    Busy,
    /// The session's cursor is at the newest event.
    Nothing,
}

/// Why a batch stopped growing.
enum Stop {
    /// The pass read its bound.
    Bound,
    /// The next event might not fit, or a gap ends the batch.
    Full,
    /// The device is reading the next ones.
    Wait,
    /// No newer event.
    End,
}

impl Server {
    /// Everything but handshake operations, for one slot.
    pub(super) fn session<D: Device>(
        &mut self,
        index: usize,
        now_ms: u32,
        device: &mut D,
        pairing: &mut Pairing,
        record: &mut impl FnMut(Record),
    ) -> bool {
        let result = self.session_step(index, now_ms, device, pairing, record);
        match result {
            Ok(busy) => busy,
            Err(reason) => {
                self.end(index, reason, record);
                true
            }
        }
    }

    fn session_step<D: Device>(
        &mut self,
        index: usize,
        now_ms: u32,
        device: &mut D,
        pairing: &mut Pairing,
        record: &mut impl FnMut(Record),
    ) -> Result<bool, Reason> {
        let slot = &mut self.slots[index];
        match &mut slot.phase {
            Phase::Free | Phase::Closing => Ok(false),
            Phase::Opening => {
                if deadline_passed(now_ms, slot.since_ms.wrapping_add(HANDSHAKE_MS)) {
                    return Err(Reason::Timeout);
                }
                let Some(len) = slot.wire.frame()? else {
                    return Ok(false);
                };
                let open: Open =
                    wellfed_proto::decode(slot.wire.message(len)).map_err(|_| Reason::Malformed)?;
                slot.wire.consume(len);
                let rescuing = self.slots.iter().any(|s| {
                    matches!(
                        s.phase,
                        Phase::Recovery(_) | Phase::Starting(Open::Recovery)
                    )
                });
                if open == Open::Recovery && (rescuing || !pairing.recovery_open()) {
                    return Err(Reason::Recovery);
                }
                self.slots[index].phase = Phase::Starting(open);
                Ok(true)
            }
            // Handshake operations report their own progress through
            // `handshake_turn`; a slot waiting on its client is not work.
            Phase::Starting(_) | Phase::Handshake { .. } => {
                if deadline_passed(now_ms, slot.since_ms.wrapping_add(HANDSHAKE_MS)) {
                    return Err(Reason::Timeout);
                }
                Ok(false)
            }
            Phase::Joining { .. } => self.joining(index, now_ms, device, record),
            Phase::Open { .. } => self.open(index, now_ms, device, pairing, record),
            Phase::Recovery(Rescue::Handshake) => {
                if deadline_passed(now_ms, slot.since_ms.wrapping_add(HANDSHAKE_MS)) {
                    return Err(Reason::Timeout);
                }
                Ok(false)
            }
            Phase::Recovery(_) => self.rescue_step(index, now_ms, device, pairing, record),
        }
    }

    /// Enroll a redeemed invite's key, answer, then close.
    fn joining<D: Device>(
        &mut self,
        index: usize,
        now_ms: u32,
        device: &mut D,
        record: &mut impl FnMut(Record),
    ) -> Result<bool, Reason> {
        let slot = &mut self.slots[index];
        let Phase::Joining {
            transport,
            key,
            label,
            role,
            ticket,
        } = &mut slot.phase
        else {
            return Ok(false);
        };
        let Some(ticket_id) = *ticket else {
            *ticket = device.keys(KeyChange::Enroll {
                key: *key,
                label: *label,
                role: *role,
            });
            return Ok(true);
        };
        let Some(outcome) = device.keys_outcome(ticket_id) else {
            return Ok(true);
        };

        let joined = match outcome {
            KeyOutcome::Done | KeyOutcome::Duplicate => Joined::Enrolled {
                role: proto_role(*role),
            },
            KeyOutcome::Full => Joined::Refused(Refusal::Full),
            KeyOutcome::NotFound | KeyOutcome::NotWritable => Joined::Refused(Refusal::NotWritable),
        };
        if matches!(joined, Joined::Enrolled { .. }) {
            let body = EventBody::Enrolled {
                client: fingerprint(key),
                role: proto_role(*role),
                path: proto::Path::Invite,
            };
            device.record(now_ms, body);
        }
        let len = proto_encode(&joined, &mut self.plain)?;
        let sealed = transport
            .seal(&self.plain[..len], &mut self.out)
            .map_err(|_| Reason::Noise)?;
        slot.wire.send(&self.out[..sealed])?;
        slot.phase = Phase::Closing;
        record(Record::Ended {
            slot: slot_id(index),
            reason: Reason::Joined,
        });
        Ok(true)
    }

    /// An open session: finish a pending write, else read one request, else
    /// push a frame of events, a sample or log lines.
    fn open<D: Device>(
        &mut self,
        index: usize,
        now_ms: u32,
        device: &mut D,
        pairing: &mut Pairing,
        record: &mut impl FnMut(Record),
    ) -> Result<bool, Reason> {
        if !self.slots[index].wire.tx_idle() {
            return Ok(true);
        }
        if let Some((id, op, reply)) = self.poll_pending(index, now_ms, device, record)? {
            return self
                .answer(index, id, op, reply, now_ms, device, record)
                .map(|()| true);
        }
        if let Phase::Open {
            pending: Some(_), ..
        } = self.slots[index].phase
        {
            return Ok(true);
        }

        let slot = &mut self.slots[index];
        if let Some(len) = slot.wire.frame()? {
            let Phase::Open { transport, .. } = &mut slot.phase else {
                return Ok(false);
            };
            let message = slot.wire.message(len);
            let read = transport
                .open(message, &mut self.plain)
                .map_err(|_| Reason::Noise)?;
            slot.wire.consume(len);
            slot.since_ms = now_ms;
            self.message(index, read, now_ms, device, pairing, record)?;
            return Ok(true);
        }
        if deadline_passed(now_ms, slot.since_ms.wrapping_add(IDLE_MS)) {
            return Err(Reason::Timeout);
        }

        self.push(index, device)
    }

    /// One decoded client message, from `self.plain[..len]`.
    fn message<D: Device>(
        &mut self,
        index: usize,
        len: usize,
        now_ms: u32,
        device: &mut D,
        pairing: &mut Pairing,
        record: &mut impl FnMut(Record),
    ) -> Result<(), Reason> {
        // The message borrows `plain`, which the answer reuses: take what
        // it says into an owned action first.
        self.plain_len = len;
        let message: ClientMessage<'_> =
            wellfed_proto::decode(&self.plain[..len]).map_err(|_| Reason::Malformed)?;
        let (id, needs, action) = match message {
            ClientMessage::Subscribe { after, samples } => {
                if let Phase::Open {
                    events, samples: s, ..
                } = &mut self.slots[index].phase
                {
                    *events = Some(after);
                    *s = samples;
                }
                return Ok(());
            }
            ClientMessage::Request { id, op } => (id, op.capability(), Action::from(op)),
        };

        match self.request(index, id, needs, action, now_ms, device, pairing, record) {
            Some(reply) => self.answer(index, id, action.name(), reply, now_ms, device, record),
            None => Ok(()),
        }
    }

    /// The client list borrows labels from the table, so it is encoded here.
    pub(super) fn list_clients(&mut self, index: usize, id: RequestId) -> Result<(), Reason> {
        let Phase::Open { key: me, .. } = self.slots[index].phase else {
            return Ok(());
        };
        let mut clients = [None; proto::CLIENTS_MAX];
        for (out, client) in clients.iter_mut().zip(self.clients.iter().flatten()) {
            *out = Some(proto::Client {
                key: client.key,
                label: core::str::from_utf8(client.label.as_bytes()).unwrap_or(""),
                role: proto_role(client.role),
                you: client.key == me,
            });
        }
        let message = FeederMessage::Response {
            id,
            result: Outcome::Clients(Clients { clients }),
        };
        let len = proto_encode(&message, &mut self.plain)?;
        self.seal_plain(index, len)
    }

    /// Push the next events, else the latest sample, else the next log
    /// lines: one frame, and only once the last is out.
    fn push<D: Device>(&mut self, index: usize, device: &mut D) -> Result<bool, Reason> {
        let Phase::Open { events, log, .. } = &self.slots[index].phase else {
            return Ok(false);
        };
        let (after, log) = (*events, *log);
        if let Some(after) = after {
            match self.push_events(index, after, device)? {
                Push::Sent | Push::Busy => return Ok(true),
                Push::Nothing => {}
            }
        }

        let Phase::Open { sample, .. } = &mut self.slots[index].phase else {
            return Ok(false);
        };
        if let Some(next) = sample.take() {
            return self
                .seal_and_send(index, &FeederMessage::Sample(next))
                .map(|()| true);
        }
        match log {
            Some(after) => self.push_log(index, after, device),
            None => Ok(false),
        }
    }

    /// The events after `after` that fit one frame, encoded into the
    /// sealing buffer (free until the message is sealed), so no array of
    /// them is kept. The session's cursor moves past each event the batch
    /// takes, and past each it skips.
    fn push_events<D: Device>(
        &mut self,
        index: usize,
        mut after: Option<u64>,
        device: &mut D,
    ) -> Result<Push, Reason> {
        let Phase::Open {
            events,
            capabilities,
            ..
        } = &mut self.slots[index].phase
        else {
            return Ok(Push::Nothing);
        };
        let capabilities = *capabilities;
        let mut writer = EventBatchWriter::new(&mut self.out);
        let mut stop = Stop::Bound;

        for _ in 0..READS_PER_PASS {
            if !writer.has_room() {
                stop = Stop::Full;
                break;
            }
            match device.event_after(after) {
                EventRead::Event(event) => {
                    // A body the session's hello did not claim would not
                    // decode there: skip it, as an old client would want.
                    if capabilities.contains(event.body.capability()) {
                        writer.push(&event).map_err(|_| Reason::Malformed)?;
                    }
                    after = Some(event.seq);
                    *events = Some(after);
                }
                // A gap goes in a message of its own. Met behind a batch's
                // events it is left where it is: the next pass meets it
                // first.
                EventRead::Gap { from, to } => {
                    if !writer.is_empty() {
                        stop = Stop::Full;
                        break;
                    }
                    *events = Some(Some(to));
                    return self
                        .seal_and_send(index, &FeederMessage::Gap { from, to })
                        .map(|()| Push::Sent);
                }
                EventRead::Wait => {
                    stop = Stop::Wait;
                    break;
                }
                EventRead::None => {
                    stop = Stop::End;
                    break;
                }
            }
        }

        if writer.is_empty() {
            // Only a cursor at the newest event leaves the frame to the
            // samples and the log; a read on its way, or a run of skipped
            // events, carries on next pass.
            return Ok(match stop {
                Stop::End => Push::Nothing,
                Stop::Bound | Stop::Full | Stop::Wait => Push::Busy,
            });
        }
        let len = proto_encode(&FeederMessage::Events(writer.batch()), &mut self.plain)?;
        self.seal_plain(index, len).map(|()| Push::Sent)
    }

    /// The lines after `after` that fit one frame. The text is read into
    /// the sealing buffer, which is free until the message is sealed.
    fn push_log<D: Device>(
        &mut self,
        index: usize,
        after: Option<u32>,
        device: &mut D,
    ) -> Result<bool, Reason> {
        let Some(batch) = device.log_after(after, &mut self.out[..LOG_TEXT_MAX]) else {
            return Ok(false);
        };
        let text = self.out.get(..batch.len).ok_or(Reason::Malformed)?;
        let message = FeederMessage::Log {
            seq: batch.seq,
            dropped: batch.dropped,
            text: core::str::from_utf8(text).map_err(|_| Reason::Malformed)?,
        };
        let len = proto_encode(&message, &mut self.plain)?;
        if let Phase::Open { log, .. } = &mut self.slots[index].phase {
            *log = Some(Some(batch.seq));
        }
        self.seal_plain(index, len).map(|()| true)
    }
}
