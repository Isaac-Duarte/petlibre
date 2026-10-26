//! Opening a session: the clear `Open` frame, then Noise IK against the
//! enrolled clients, or XXpsk3 for an invite. At most one handshake
//! operation runs per pass, and none during a feed: an X25519 costs about
//! 27 ms on the KM4, so clients connecting at once cannot stack up to hold
//! the loop.

use super::recovery::Rescue;
use super::request::proto_role;
use super::{Phase, Server, proto_encode, slot_id};
use crate::clients::{CLIENTS, Client, Label, Role};
use crate::pairing::{self, Pairing};
use crate::{Device, Reason, Record, SESSIONS};
use wellfed_proto::api::{
    self as proto, Capabilities, Fingerprint, Hello, Open, OpenRefusal, PROLOGUE, PROTOCOL_MAJOR,
    PROTOCOL_MAJOR_MIN, Welcome,
};
use wellfed_session::noise::{Handshake, Keypair, Pattern, Role as NoiseRole};

impl Server {
    /// At most one handshake operation, for the first slot from `turn` that
    /// has one due.
    pub(super) fn handshake_turn<D: Device>(
        &mut self,
        now_ms: u32,
        identity: &Keypair,
        device: &mut D,
        pairing: &mut Pairing,
        record: &mut impl FnMut(Record),
    ) -> bool {
        for offset in 0..SESSIONS {
            let index = (self.turn + offset) % SESSIONS;
            let rescuing = matches!(
                self.slots[index].phase,
                Phase::Starting(Open::Recovery) | Phase::Recovery(Rescue::Handshake)
            );
            let result = if rescuing {
                self.rescue_op(index, now_ms, identity, device, pairing, record)
            } else {
                self.handshake_op(index, now_ms, identity, device, pairing, record)
            };
            match result {
                Ok(false) => continue,
                Ok(true) => {}
                Err(reason) => self.end(index, reason, record),
            }
            self.turn = (index + 1) % SESSIONS;
            return true;
        }
        false
    }

    /// One operation for `index` if it has one due; whether it did.
    fn handshake_op<D: Device>(
        &mut self,
        index: usize,
        now_ms: u32,
        identity: &Keypair,
        device: &mut D,
        pairing: &mut Pairing,
        record: &mut impl FnMut(Record),
    ) -> Result<bool, Reason> {
        let slot = &mut self.slots[index];
        match &mut slot.phase {
            Phase::Starting(open) => {
                let open = *open;
                let mut ephemeral = [0; 32];
                if !device.random(&mut ephemeral) {
                    return Err(Reason::NotReady);
                }
                let pattern = match open {
                    Open::Session => Pattern::Ik,
                    Open::Invite { .. } => Pattern::XxPsk3,
                    // `handshake_turn` sends recovery slots to `rescue_op`.
                    Open::Recovery => return Err(Reason::Recovery),
                };
                let mut handshake = Handshake::new(
                    pattern,
                    NoiseRole::Responder,
                    PROLOGUE,
                    identity,
                    None,
                    ephemeral,
                )
                .map_err(|_| Reason::Noise)?;
                if let Open::Invite { id } = open {
                    let psk = pairing.invite_secret(now_ms, id).ok_or(Reason::Invite)?;
                    handshake.set_psk(psk);
                }
                slot.phase = Phase::Handshake {
                    handshake,
                    open,
                    reply: false,
                    refusal: None,
                    capabilities: Capabilities::V1,
                };
                Ok(true)
            }
            Phase::Handshake {
                handshake,
                open,
                reply,
                refusal,
                capabilities,
            } => {
                if *reply {
                    if !slot.wire.tx_idle() {
                        return Ok(false);
                    }
                    let payload = match (*open, *refusal) {
                        (Open::Session, refusal) => {
                            let role = handshake
                                .remote_static()
                                .and_then(|key| find(&self.clients, key))
                                .map(|client| client.role);
                            // The ring opens early in boot; until then the
                            // handshake waits, within its own deadline.
                            let Some(epoch) = device.epoch() else {
                                return Ok(false);
                            };
                            let welcome = welcome(
                                refusal,
                                role,
                                device.capabilities(),
                                device.build(),
                                epoch,
                            );
                            let len = proto_encode(&welcome, &mut self.plain)?;
                            &self.plain[..len]
                        }
                        _ => &[][..],
                    };
                    let len = handshake
                        .write_message(payload, &mut self.out)
                        .map_err(|_| Reason::Noise)?;
                    *reply = false;
                    slot.wire.send(&self.out[..len])?;
                    return self.handshake_written(index, now_ms, record).map(|()| true);
                }

                let Some(len) = slot.wire.frame()? else {
                    return Ok(false);
                };
                let message = slot.wire.message(len);
                let read = handshake
                    .read_message(message, &mut self.plain)
                    .map_err(|_| Reason::Noise)?;
                slot.wire.consume(len);
                if handshake.is_finished() {
                    // XXpsk3's third message: the invite's join.
                    let key = *handshake.remote_static().ok_or(Reason::Noise)?;
                    let join: proto::Join<'_> = wellfed_proto::decode(&self.plain[..read])
                        .map_err(|_| Reason::Malformed)?;
                    let label = Label::new(join.label.as_bytes()).ok_or(Reason::Malformed)?;
                    let Open::Invite { id } = *open else {
                        return Err(Reason::Malformed);
                    };
                    let event = pairing.redeem(now_ms, id);
                    record(Record::Pairing(event));
                    let pairing::Event::Admit { role, .. } = event else {
                        return Err(Reason::Invite);
                    };
                    let Phase::Handshake { handshake, .. } =
                        core::mem::replace(&mut slot.phase, Phase::Closing)
                    else {
                        return Err(Reason::Noise);
                    };
                    slot.phase = Phase::Joining {
                        transport: handshake.into_transport().map_err(|_| Reason::Noise)?,
                        key,
                        label,
                        role,
                        ticket: None,
                    };
                    return Ok(true);
                }
                if *open == Open::Session {
                    let hello: Hello = wellfed_proto::decode(&self.plain[..read])
                        .map_err(|_| Reason::Malformed)?;
                    *refusal = admit(hello, handshake.remote_static(), &self.clients);
                    *capabilities = hello.capabilities;
                }
                *reply = true;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// After the feeder's handshake message: an IK session opens (or ends
    /// with its refusal).
    fn handshake_written(
        &mut self,
        index: usize,
        now_ms: u32,
        record: &mut impl FnMut(Record),
    ) -> Result<(), Reason> {
        let slot = &mut self.slots[index];
        let Phase::Handshake {
            handshake, refusal, ..
        } = &slot.phase
        else {
            return Ok(());
        };
        if !handshake.is_finished() {
            return Ok(());
        }
        if let Some(refusal) = *refusal {
            slot.phase = Phase::Closing;
            record(Record::Ended {
                slot: slot_id(index),
                reason: match refusal {
                    OpenRefusal::Unknown => Reason::Unknown,
                    OpenRefusal::Version { .. } => Reason::Version,
                    OpenRefusal::Full => Reason::Full,
                },
            });
            return Ok(());
        }

        let Phase::Handshake {
            handshake,
            capabilities,
            ..
        } = core::mem::replace(&mut slot.phase, Phase::Closing)
        else {
            return Ok(());
        };
        let key = *handshake.remote_static().ok_or(Reason::Noise)?;
        let role = find(&self.clients, &key)
            .map(|client| client.role)
            .ok_or(Reason::Unknown)?;
        let transport = handshake.into_transport().map_err(|_| Reason::Noise)?;
        let memory = self.memory(&key);
        let slot = &mut self.slots[index];
        slot.since_ms = now_ms;
        slot.phase = Phase::Open {
            transport,
            key,
            role,
            capabilities,
            memory,
            pending: None,
            events: None,
            samples: false,
            sample: None,
        };
        record(Record::Opened {
            slot: slot_id(index),
            role,
            client: fingerprint(&key),
        });
        Ok(())
    }
}
/// The IK client's hello against the table: why to refuse it, if at all.
fn admit(
    hello: Hello,
    key: Option<&[u8; 32]>,
    clients: &[Option<Client>; CLIENTS],
) -> Option<OpenRefusal> {
    if !(PROTOCOL_MAJOR_MIN..=PROTOCOL_MAJOR).contains(&hello.major) {
        return Some(OpenRefusal::Version {
            supported: PROTOCOL_MAJOR,
        });
    }
    let session_role = key
        .and_then(|key| find(clients, key))
        .map(|client| client.role)
        .filter(|&role| role != Role::UpdateSigner);
    session_role.is_none().then_some(OpenRefusal::Unknown)
}

fn welcome(
    refusal: Option<OpenRefusal>,
    role: Option<Role>,
    capabilities: Capabilities,
    build: &str,
    epoch: u32,
) -> Welcome<'_> {
    match (refusal, role) {
        (None, Some(role)) => Welcome::Open {
            major: PROTOCOL_MAJOR,
            capabilities,
            build,
            role: proto_role(role),
            epoch,
        },
        (Some(refusal), _) => Welcome::Refused(refusal),
        (None, None) => Welcome::Refused(OpenRefusal::Unknown),
    }
}

pub(super) fn find<'a>(
    clients: &'a [Option<Client>; CLIENTS],
    key: &[u8; 32],
) -> Option<&'a Client> {
    clients.iter().flatten().find(|client| &client.key == key)
}

pub(super) fn invite(id: u32, secret: [u8; 32]) -> proto::Invite {
    let seconds = pairing::Config::DEFAULT.invite_ms / 1000;
    proto::Invite {
        id,
        secret,
        seconds: u16::try_from(seconds).unwrap_or(u16::MAX),
    }
}

pub fn fingerprint(key: &[u8; 32]) -> Fingerprint {
    [key[0], key[1], key[2], key[3]]
}
