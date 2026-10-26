//! The recovery window's session: the setup exchange over this stream, so
//! the owner keys the client's code in on the buttons before it is enrolled
//! as an owner. Its responder's buffer would not fit a slot, so the server
//! holds one, and one recovery session runs at a time.

use super::handshake::fingerprint;
use super::{Phase, Server, slot_id};
use crate::clients::Role;
use crate::pairing::{self, Pairing, Path};
use crate::setup::{self as rescue, Offer, Received};
use crate::{Device, KeyChange, KeyOutcome, Reason, Record};
use wellfed_core::deadline_passed;
use wellfed_proto::api::{self as proto, EventBody, Open};
use wellfed_proto::setup::{
    Enrolled as SetupEnrolled, Refusal as SetupRefusal, Reply as SetupReply,
    WifiSaved as SetupWifiSaved,
};
use wellfed_session::noise::Keypair;

impl Server {
    /// The recovery session's handshake operation, out of line: the setup
    /// responder's frame is a kilobyte, and `handshake_op`'s another, so the
    /// two must never nest on the loop's stack.
    #[inline(never)]
    pub(super) fn rescue_op<D: Device>(
        &mut self,
        index: usize,
        now_ms: u32,
        identity: &Keypair,
        device: &mut D,
        pairing: &mut Pairing,
        record: &mut impl FnMut(Record),
    ) -> Result<bool, Reason> {
        let slot = &mut self.slots[index];
        match slot.phase {
            Phase::Starting(Open::Recovery) => {
                let mut ephemeral = [0; 32];
                if !device.random(&mut ephemeral) {
                    return Err(Reason::NotReady);
                }
                self.rescue
                    .restart(identity, ephemeral)
                    .map_err(|_| Reason::Noise)?;
                slot.phase = Phase::Recovery(Rescue::Handshake);
                Ok(true)
            }
            Phase::Recovery(Rescue::Handshake) => {
                if !slot.wire.tx_idle() {
                    return Ok(false);
                }
                let Some(len) = slot.wire.frame()? else {
                    return Ok(false);
                };
                let out = &mut self.out[..rescue::MAX_MESSAGE];
                let received = self.rescue.receive(slot.wire.message(len), out);
                slot.wire.consume(len);
                match received.map_err(|_| Reason::Noise)? {
                    Received::Send(n) => slot.wire.send(&self.out[..n])?,
                    Received::Offer(offer) => {
                        let event = pairing.recovery(now_ms);
                        record(Record::Pairing(event));
                        let reply = if let pairing::Event::Confirming { .. } = event {
                            slot.phase = Phase::Recovery(Rescue::Asked { offer });
                            let seconds = pairing::Config::DEFAULT.confirm_ms / 1000;
                            SetupReply::EnterCode {
                                seconds: u16::try_from(seconds).unwrap_or(u16::MAX),
                            }
                        } else {
                            slot.phase = Phase::Closing;
                            SetupReply::Refused(SetupRefusal::Busy)
                        };
                        let n = self
                            .rescue
                            .reply(&reply, &mut self.out)
                            .map_err(|_| Reason::Noise)?;
                        slot.wire.send(&self.out[..n])?;
                    }
                    Received::Proven { .. } => return Err(Reason::Malformed),
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// The recovery session after its offer: wait for the code, prove it,
    /// enroll the client as an owner, answer, then close.
    pub(super) fn rescue_step<D: Device>(
        &mut self,
        index: usize,
        now_ms: u32,
        device: &mut D,
        pairing: &mut Pairing,
        record: &mut impl FnMut(Record),
    ) -> Result<bool, Reason> {
        let slot = &mut self.slots[index];
        if !slot.wire.tx_idle() {
            return Ok(true);
        }
        let Phase::Recovery(stage) = slot.phase else {
            return Ok(false);
        };

        match stage {
            Rescue::Handshake => Ok(true),
            Rescue::Asked { offer } => {
                // The client speaks only in the handshake and the proof.
                if slot.wire.frame()?.is_some() {
                    pairing.cancel(Path::Recovery);
                    return Err(Reason::Malformed);
                }
                match self.decision.take() {
                    Some(pairing::Event::Entered { code, .. }) => {
                        let mut seed = [0; 32];
                        if !device.random(&mut seed) {
                            return Err(Reason::NotReady);
                        }
                        let out = &mut self.out[..rescue::MAX_MESSAGE];
                        let n = self
                            .rescue
                            .prove(code, seed, out)
                            .map_err(|_| Reason::Noise)?;
                        let slot = &mut self.slots[index];
                        slot.wire.send(&self.out[..n])?;
                        slot.phase = Phase::Recovery(Rescue::Proving {
                            offer,
                            until: now_ms.wrapping_add(PROVE_MS),
                        });
                        Ok(true)
                    }
                    Some(_) => self.rescued(
                        index,
                        SetupReply::Refused(SetupRefusal::Unconfirmed),
                        record,
                    ),
                    None => Ok(false),
                }
            }
            Rescue::Proving { offer, until } => {
                if deadline_passed(now_ms, until) {
                    return self.rescued(
                        index,
                        SetupReply::Refused(SetupRefusal::Unconfirmed),
                        record,
                    );
                }
                let Some(len) = slot.wire.frame()? else {
                    return Ok(false);
                };
                let out = &mut self.out[..rescue::MAX_MESSAGE];
                let received = self.rescue.receive(slot.wire.message(len), out);
                slot.wire.consume(len);
                match received {
                    Ok(Received::Send(n)) => {
                        self.slots[index].wire.send(&self.out[..n])?;
                        Ok(true)
                    }
                    // Recovery restores access; Wi-Fi changes go over a session.
                    Ok(Received::Proven { wifi: Some(_) }) => {
                        self.rescued(index, SetupReply::Refused(SetupRefusal::Invalid), record)
                    }
                    Ok(Received::Proven { wifi: None }) => {
                        let stage = Rescue::Enrolling {
                            offer,
                            ticket: None,
                        };
                        self.slots[index].phase = Phase::Recovery(stage);
                        Ok(true)
                    }
                    Err(rescue::Error::WrongCode) => {
                        self.rescued(index, SetupReply::Refused(SetupRefusal::WrongCode), record)
                    }
                    Ok(Received::Offer(_)) | Err(_) => Err(Reason::Noise),
                }
            }
            Rescue::Enrolling { offer, ticket } => {
                let Some(ticket) = ticket else {
                    let change = KeyChange::Enroll {
                        key: offer.key,
                        label: offer.label,
                        role: Role::Owner,
                    };
                    if let Some(ticket) = device.keys(change) {
                        let stage = Rescue::Enrolling {
                            offer,
                            ticket: Some(ticket),
                        };
                        self.slots[index].phase = Phase::Recovery(stage);
                    }
                    return Ok(true);
                };
                let Some(outcome) = device.keys_outcome(ticket) else {
                    return Ok(true);
                };
                let enrolled = match outcome {
                    KeyOutcome::Done => SetupEnrolled::New,
                    KeyOutcome::Duplicate => SetupEnrolled::Already,
                    KeyOutcome::Full => {
                        return self.rescued(
                            index,
                            SetupReply::Refused(SetupRefusal::Full),
                            record,
                        );
                    }
                    KeyOutcome::NotFound | KeyOutcome::NotWritable => {
                        return self.rescued(
                            index,
                            SetupReply::Refused(SetupRefusal::NotWritable),
                            record,
                        );
                    }
                };
                let body = EventBody::Enrolled {
                    client: fingerprint(&offer.key),
                    role: proto::Role::Owner,
                    path: proto::Path::Recovery,
                };
                device.record(now_ms, body);
                let done = SetupReply::Done {
                    enrolled,
                    wifi: SetupWifiSaved::Unchanged,
                };
                self.rescued(index, done, record)
            }
        }
    }

    /// The recovery session's last reply; the connection then closes.
    fn rescued(
        &mut self,
        index: usize,
        reply: SetupReply,
        record: &mut impl FnMut(Record),
    ) -> Result<bool, Reason> {
        let n = self
            .rescue
            .reply(&reply, &mut self.out)
            .map_err(|_| Reason::Noise)?;
        self.rescue.stop();
        let slot = &mut self.slots[index];
        slot.wire.send(&self.out[..n])?;
        slot.phase = Phase::Closing;
        let reason = match reply {
            SetupReply::Done { .. } => Reason::Joined,
            _ => Reason::Recovery,
        };
        record(Record::Ended {
            slot: slot_id(index),
            reason,
        });
        Ok(true)
    }
}
/// Where the recovery session stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Rescue {
    Handshake,
    /// Pairing holds the offer until the owner keys its code in.
    Asked {
        offer: Offer,
    },
    /// The code proof runs until the client's last step or `until`.
    Proving {
        offer: Offer,
        until: u32,
    },
    /// The enrollment is queued (or waits for the mailbox: `ticket` None).
    Enrolling {
        offer: Offer,
        ticket: Option<u32>,
    },
}

/// From the last press of the code to the client's last step, as BLE
/// setup allows.
const PROVE_MS: u32 = 15_000;
