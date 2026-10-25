//! Device API, the feeder's side ([`wellfed_proto::api`]).
//!
//! A [`Server`] holds [`SESSIONS`] fixed slots, one per TCP connection, and
//! never touches a socket: the firmware's driver copies received bytes into
//! a slot ([`Server::rx`], [`Server::received`]), sends what a slot holds
//! ([`Server::tx`], [`Server::sent`]) and closes it when asked
//! ([`Server::closing`]). Everything else happens in [`Server::step`], once
//! per pass of the loop task:
//! - **One handshake operation per pass,** across all slots, and none while
//!   a feed runs: an X25519 costs about 27 ms on the KM4, so four clients
//!   connecting at once cannot stack up to hold the loop.
//! - **One frame per slot per pass,** and a slot writes again only once its
//!   last frame is sent, so a flood costs only its own turn.
//! - **Every write has an id** the server remembers per client key, so a
//!   retry, even after a reconnect, gets the first answer instead of
//!   running again.
//! - **Writes carry their base generation;** a stale one is refused with
//!   the current generation.
//! - **A feed is answered once it is durably reserved,** with the number
//!   the journal holds it under; its result is a `Fed` event, which only
//!   sessions whose hello carries [`Capabilities::FEED`] are sent.
//!
//! Sessions authenticate with Noise IK under the feeder's identity, against
//! the enrolled clients the settings writer publishes. A revoked key's
//! sessions close as soon as the new table arrives. A new client redeems an
//! invite with XXpsk3 and is enrolled through the keys mailbox.
//!
//! The firmware reaches the rest of the feeder through [`Device`], and the
//! host tests through a fake of it.

use wellfed_core::frame;
use wellfed_core::noise::{Handshake, Keypair, Pattern, Role as NoiseRole, Transport};
use wellfed_core::store::clients::{CLIENTS, Client, Label, Role};
use wellfed_core::store::net::Credentials;
use wellfed_core::tone::Tune as CoreTune;
use wellfed_core::transport::{
    LockResult, PlayResult, SettingsFields, SettingsResult, TimeFields, TimeResult, WifiResult,
};
use wellfed_core::{deadline_passed, named_enum};
use wellfed_proto::api::{
    self as proto, Capabilities, ClientMessage, Clients, EventBody, FRAME_MAX, FeederMessage,
    Fingerprint, Hello, Joined, Op, Open, OpenRefusal, Outcome, PROLOGUE, PROTOCOL_MAJOR,
    PROTOCOL_MAJOR_MIN, Refusal, RequestId, Sample, Source, Status, Tune, Welcome,
};

use crate::pairing::{self, Pairing, Path};
use crate::setup::{self as rescue, Offer, Received, Responder};
use wellfed_proto::setup::{
    Enrolled as SetupEnrolled, Refusal as SetupRefusal, Reply as SetupReply,
    WifiSaved as SetupWifiSaved,
};

/// Connections served at once (lwIP leaves room for four).
pub const SESSIONS: usize = 4;
/// Write ids remembered per client.
pub const REMEMBERED: usize = 32;
/// A connection that has not finished its handshake by then is dropped.
pub const HANDSHAKE_MS: u32 = 10_000;
/// A session that sends nothing for this long is dropped; clients send a
/// `Status` as a keepalive.
pub const IDLE_MS: u32 = 90_000;
/// After `Reboot`, long enough for the answer to leave.
pub const REBOOT_MS: u32 = 1_000;

const BUFFER: usize = frame::PREFIX + FRAME_MAX;

/// The rest of the feeder, as the server needs it. Every call returns at
/// once: a write is queued for its owner, and the server polls for the
/// result on later passes.
pub trait Device {
    /// Fill `out` from the vetted generator; false when it has no output.
    fn random(&mut self, out: &mut [u8; 32]) -> bool;
    /// The build identity, for the welcome and status.
    fn build(&self) -> &'static str;
    /// Everything [`Status`] reports but the session count.
    fn status(&mut self, now_ms: u32) -> Status<'static>;
    /// The settings, or one change queued for the writer.
    fn settings(&mut self, change: SettingsFields) -> SettingsResult;
    /// The Wi-Fi store; with `Some`, one change (`Some(None)` clears) queued.
    fn wifi(&mut self, change: Option<Option<Credentials>>) -> WifiResult;
    fn lock(&mut self, locked: bool) -> LockResult;
    /// `None` when this firmware does not play tunes outside setup mode.
    fn play(&mut self, tune: CoreTune) -> Option<PlayResult>;
    fn time_sync(&mut self, fields: TimeFields) -> TimeResult;
    /// Queue a client store change; its ticket, or `None` while the mailbox
    /// is busy.
    fn keys(&mut self, change: KeyChange) -> Option<u32>;
    /// The result of the change with `ticket`, once written.
    fn keys_outcome(&mut self, ticket: u32) -> Option<KeyOutcome>;
    /// What this firmware serves beyond major version 1's first ops.
    fn capabilities(&self) -> Capabilities;
    /// Queue one feed for the feeding owner on `by`'s behalf: the number
    /// its journal will hold it under.
    fn feed(&mut self, portions: u8, by: Source) -> Result<u32, Refusal>;
    /// Whether the feed `feed` queued is durably reserved yet: `None` while
    /// it waits, or why it ended before it was.
    fn feed_reserved(&mut self, feed: u32) -> Option<Result<(), Refusal>>;
    /// Stop the feed `feed` if it is still running; false once it has ended.
    fn cancel_feed(&mut self, feed: u32) -> bool;
    /// Record one durable event.
    fn record(&mut self, now_ms: u32, body: EventBody);
    /// The first durable event after `after` (`None`: from the oldest).
    fn event_after(&mut self, after: Option<u64>) -> EventRead;
    /// The event ring's epoch, `None` until the ring is open. A session is
    /// welcomed only once it is known.
    fn epoch(&self) -> Option<u32>;
    /// Restart the feeder.
    fn reboot(&mut self);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyChange {
    Enroll {
        key: [u8; 32],
        label: Label,
        role: Role,
    },
    Revoke {
        key: [u8; 32],
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyOutcome {
    Done,
    Full,
    Duplicate,
    NotFound,
    NotWritable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventRead {
    Event(proto::Event),
    /// Events `from..=to` are gone from the ring.
    Gap {
        from: u64,
        to: u64,
    },
    /// Nothing newer yet.
    None,
    /// Ask again on a later pass (the writer is reading flash).
    Wait,
}

named_enum! {
    /// Why a connection ended or was refused.
    pub enum Reason {
        /// The client closed it.
        Closed = "closed",
        /// Every slot is taken.
        Full = "full",
        /// A frame longer than the limit, or bytes that do not decode.
        Malformed = "malformed",
        /// A handshake or transport message failed to open.
        Noise = "noise",
        Unknown = "unknown-key",
        Version = "version",
        /// No live invite with that id.
        Invite = "invite",
        /// The recovery window is closed (or not served here yet).
        Recovery = "recovery",
        Timeout = "timeout",
        Revoked = "revoked",
        /// The identity is not loaded, or the generator has no output.
        NotReady = "not-ready",
        /// An invite redemption finished.
        Joined = "joined",
    }
}

/// What the firmware logs (`PLAPI1`). No record carries a key or a secret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Accepted {
        slot: u8,
    },
    Opened {
        slot: u8,
        role: Role,
        client: Fingerprint,
    },
    Request {
        slot: u8,
        op: &'static str,
        result: &'static str,
    },
    Retried {
        slot: u8,
    },
    Ended {
        slot: u8,
        reason: Reason,
    },
    Pairing(pairing::Event),
}

/// One pass's inputs from the loop.
pub struct Pass<'a, D> {
    pub now_ms: u32,
    pub device: &'a mut D,
    pub pairing: &'a mut Pairing,
    pub identity: Option<&'a Keypair>,
    /// A feed is running: no handshake operation this pass.
    pub feeding: bool,
    /// Pairing's decision on a LAN recovery offer this pass: its code keyed
    /// in, or its timeout.
    pub recovery: Option<pairing::Event>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Remembered {
    Done(Option<u32>),
    Refused(Refusal),
    Invite { id: u32 },
    Feeding { feed: u32 },
}

/// The write ids one client's sessions have answered.
#[derive(Clone, Copy)]
struct Memory {
    key: [u8; 32],
    ids: [Option<(RequestId, Remembered)>; REMEMBERED],
    next: usize,
    /// For eviction: the pass that last used it.
    used: u32,
}

impl Memory {
    const EMPTY: Self = Self {
        key: [0; 32],
        ids: [None; REMEMBERED],
        next: 0,
        used: 0,
    };

    fn find(&self, id: &RequestId) -> Option<Remembered> {
        self.ids
            .iter()
            .flatten()
            .find(|(seen, _)| seen == id)
            .map(|&(_, result)| result)
    }

    fn remember(&mut self, id: RequestId, result: Remembered) {
        self.ids[self.next] = Some((id, result));
        self.next = (self.next + 1) % REMEMBERED;
    }
}

/// A write the device is still completing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Waiting {
    Settings,
    Wifi,
    Keys { ticket: u32, revoked: [u8; 32] },
    Feed { feed: u32 },
}

#[derive(Clone, Copy)]
struct Pending {
    id: RequestId,
    op: &'static str,
    waiting: Waiting,
}

enum Phase {
    Free,
    /// Connected; the clear [`Open`] frame comes first.
    Opening,
    /// The connection asked for an invite; the handshake starts next turn.
    Starting(Open),
    Handshake {
        handshake: Handshake,
        open: Open,
        /// The client's message is read; ours is due.
        reply: bool,
        /// The IK client's hello, once read.
        refusal: Option<OpenRefusal>,
        capabilities: Capabilities,
    },
    Open {
        transport: Transport,
        key: [u8; 32],
        role: Role,
        /// What the client's hello said it understands.
        capabilities: Capabilities,
        memory: usize,
        pending: Option<Pending>,
        /// Push events after this cursor (`Some(None)`: from the oldest).
        events: Option<Option<u64>>,
        samples: bool,
        sample: Option<Sample>,
    },
    /// An invite's handshake is done: enroll the key, then answer.
    Joining {
        transport: Transport,
        key: [u8; 32],
        label: Label,
        role: Role,
        ticket: Option<u32>,
    },
    /// The recovery window's session: the setup exchange over this
    /// stream, so the owner keys the client's code in before it is enrolled.
    Recovery(Rescue),
    /// Send what is left, then close.
    Closing,
}

struct Slot {
    phase: Phase,
    since_ms: u32,
    wire: Wire,
}

impl Slot {
    const FREE: Self = Self {
        phase: Phase::Free,
        since_ms: 0,
        wire: Wire::EMPTY,
    };
}

/// One connection's bytes each way, apart from its phase so a step can
/// hold both.
struct Wire {
    rx: [u8; BUFFER],
    rx_len: usize,
    tx: [u8; BUFFER],
    tx_len: usize,
    tx_sent: usize,
}

impl Wire {
    const EMPTY: Self = Self {
        rx: [0; BUFFER],
        rx_len: 0,
        tx: [0; BUFFER],
        tx_len: 0,
        tx_sent: 0,
    };

    const fn tx_idle(&self) -> bool {
        self.tx_sent >= self.tx_len
    }

    /// The first complete frame's length, if one is in.
    fn frame(&self) -> Result<Option<usize>, Reason> {
        if self.rx_len < frame::PREFIX {
            return Ok(None);
        }
        let prefix = [self.rx[0], self.rx[1]];
        let len = frame::parse_prefix(prefix, FRAME_MAX).ok_or(Reason::Malformed)?;
        Ok((self.rx_len >= frame::PREFIX + len).then_some(len))
    }

    /// The first frame's bytes, `len` after its prefix.
    fn message(&self, len: usize) -> &[u8] {
        &self.rx[frame::PREFIX..frame::PREFIX + len]
    }

    /// Drop the first frame, `len` bytes after its prefix.
    fn consume(&mut self, len: usize) {
        let used = frame::PREFIX + len;
        self.rx.copy_within(used..self.rx_len, 0);
        self.rx_len -= used;
    }

    /// Queue `bytes` as one frame; the caller checked [`Self::tx_idle`].
    fn send(&mut self, bytes: &[u8]) -> Result<(), Reason> {
        let prefix = frame::prefix(bytes.len(), FRAME_MAX).ok_or(Reason::Malformed)?;
        self.tx[..frame::PREFIX].copy_from_slice(&prefix);
        self.tx[frame::PREFIX..frame::PREFIX + bytes.len()].copy_from_slice(bytes);
        self.tx_len = frame::PREFIX + bytes.len();
        self.tx_sent = 0;
        Ok(())
    }
}

pub struct Server {
    slots: [Slot; SESSIONS],
    memories: [Memory; SESSIONS],
    clients: [Option<Client>; CLIENTS],
    clients_generation: u32,
    /// Where the next handshake turn starts looking, so no slot starves.
    turn: usize,
    pass: u32,
    reboot_at: Option<u32>,
    /// The one recovery session's responder; its buffer would not fit a slot.
    rescue: Responder,
    /// Pairing's decision on the recovery offer this pass.
    decision: Option<pairing::Event>,
    /// A recovery client left with its offer pending: cancel it next pass.
    cancel_rescue: bool,
    /// A message's plaintext, and the next one's encoding: here rather than
    /// on the loop's stack.
    plain: [u8; FRAME_MAX],
    out: [u8; FRAME_MAX],
}

impl core::fmt::Debug for Server {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Server")
            .field("sessions", &self.sessions())
            .finish_non_exhaustive()
    }
}

impl Server {
    /// Built at compile time: the table is about 10 KB, more than the loop
    /// task's stack.
    pub const fn new() -> Self {
        Self {
            slots: [Slot::FREE, Slot::FREE, Slot::FREE, Slot::FREE],
            memories: [Memory::EMPTY; SESSIONS],
            clients: [None; CLIENTS],
            clients_generation: 0,
            turn: 0,
            pass: 0,
            reboot_at: None,
            rescue: Responder::idle(),
            decision: None,
            cancel_rescue: false,
            plain: [0; FRAME_MAX],
            out: [0; FRAME_MAX],
        }
    }

    /// The table to refresh from the writer, and its generation; call
    /// [`Self::clients_changed`] after refreshing it.
    pub fn clients_table(&mut self) -> (&mut [Option<Client>; CLIENTS], u32) {
        (&mut self.clients, self.clients_generation)
    }

    /// The table changed: close every session whose key is gone.
    pub fn clients_changed(&mut self, generation: u32, mut record: impl FnMut(Record)) {
        self.clients_generation = generation;
        for index in 0..SESSIONS {
            let Phase::Open { key, .. } = self.slots[index].phase else {
                continue;
            };
            if !self.clients.iter().flatten().any(|c| c.key == key) {
                self.end(index, Reason::Revoked, &mut record);
            }
        }
    }

    /// Open sessions and connections still opening.
    pub fn sessions(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| !matches!(slot.phase, Phase::Free))
            .count()
    }

    /// A new connection: its slot, or `None` to refuse it (close at once).
    pub fn accept(&mut self, now_ms: u32, mut record: impl FnMut(Record)) -> Option<usize> {
        let Some(index) = self
            .slots
            .iter()
            .position(|slot| matches!(slot.phase, Phase::Free))
        else {
            record(Record::Ended {
                slot: u8::MAX,
                reason: Reason::Full,
            });
            return None;
        };
        let slot = &mut self.slots[index];
        *slot = Slot::FREE;
        slot.phase = Phase::Opening;
        slot.since_ms = now_ms;
        record(Record::Accepted {
            slot: slot_id(index),
        });
        Some(index)
    }

    /// Room for received bytes in `slot`.
    pub fn rx(&mut self, slot: usize) -> &mut [u8] {
        let slot = &mut self.slots[slot].wire;
        &mut slot.rx[slot.rx_len..]
    }

    pub fn received(&mut self, slot: usize, len: usize) {
        let slot = &mut self.slots[slot].wire;
        slot.rx_len = (slot.rx_len + len).min(BUFFER);
    }

    /// Bytes waiting to be sent on `slot`.
    pub fn tx(&self, slot: usize) -> &[u8] {
        let slot = &self.slots[slot].wire;
        &slot.tx[slot.tx_sent..slot.tx_len]
    }

    pub fn sent(&mut self, slot: usize, len: usize) {
        let slot = &mut self.slots[slot].wire;
        slot.tx_sent = (slot.tx_sent + len).min(slot.tx_len);
    }

    /// The driver should close `slot` now: it ended and its last bytes are
    /// out. Call [`Self::closed`] once it has.
    pub fn closing(&self, slot: usize) -> bool {
        let slot = &self.slots[slot];
        matches!(slot.phase, Phase::Closing) && slot.wire.tx_idle()
    }

    /// The connection is gone (the client hung up, or the driver closed it).
    pub fn closed(&mut self, slot: usize, mut record: impl FnMut(Record)) {
        match self.slots[slot].phase {
            Phase::Free => {}
            Phase::Closing => self.slots[slot] = Slot::FREE,
            Phase::Recovery(_) => {
                // An offer waiting for its code goes with its client.
                self.cancel_rescue = true;
                self.rescue.stop();
                self.end(slot, Reason::Closed, &mut record);
                self.slots[slot] = Slot::FREE;
            }
            _ => {
                self.end(slot, Reason::Closed, &mut record);
                self.slots[slot] = Slot::FREE;
            }
        }
    }

    /// A live sample for subscribers; one that is not sent before the next
    /// replaces it.
    pub fn sample(&mut self, sample: Sample) {
        for slot in &mut self.slots {
            if let Phase::Open {
                samples: true,
                sample: next,
                ..
            } = &mut slot.phase
            {
                *next = Some(sample);
            }
        }
    }

    /// One pass. Returns true while work is waiting, so the loop should
    /// step again soon.
    pub fn step<D: Device>(&mut self, pass: Pass<'_, D>, mut record: impl FnMut(Record)) -> bool {
        let Pass {
            now_ms,
            device,
            pairing,
            identity,
            feeding,
            recovery,
        } = pass;
        self.pass = self.pass.wrapping_add(1);
        self.decision = recovery;
        if core::mem::take(&mut self.cancel_rescue) {
            pairing.cancel(Path::Recovery);
        }

        if let Some(at) = self.reboot_at
            && deadline_passed(now_ms, at)
        {
            self.reboot_at = None;
            device.reboot();
        }

        let mut busy = false;
        if !feeding && let Some(identity) = identity {
            busy |= self.handshake_turn(now_ms, identity, device, pairing, &mut record);
        }
        for index in 0..SESSIONS {
            busy |= self.session(index, now_ms, device, pairing, &mut record);
        }
        busy
    }

    /// At most one handshake operation, for the first slot from `turn` that
    /// has one due.
    fn handshake_turn<D: Device>(
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

    /// The recovery session's handshake operation, out of line: the setup
    /// responder's frame is a kilobyte, and `handshake_op`'s another, so the
    /// two must never nest on the loop's stack.
    #[inline(never)]
    fn rescue_op<D: Device>(
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

    /// The memory for `key`, or the least recently used one not serving an
    /// open session, cleared for it.
    fn memory(&mut self, key: &[u8; 32]) -> usize {
        let pass = self.pass;
        if let Some(index) = self
            .memories
            .iter()
            .position(|m| m.used != 0 && &m.key == key)
        {
            self.memories[index].used = pass;
            return index;
        }
        let in_use = |index: usize| {
            self.slots
                .iter()
                .any(|slot| matches!(slot.phase, Phase::Open { memory, .. } if memory == index))
        };
        let index = (0..SESSIONS)
            .filter(|&index| !in_use(index))
            .min_by_key(|&index| self.memories[index].used)
            .unwrap_or(0);
        self.memories[index] = Memory {
            key: *key,
            used: pass.max(1),
            ..Memory::EMPTY
        };
        index
    }

    /// Everything but handshake operations, for one slot.
    fn session<D: Device>(
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
                let open: Open = wellfed_proto::decode(slot.wire.message(len))
                    .map_err(|_| Reason::Malformed)?;
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

    /// The recovery session after its offer: wait for the code, prove it,
    /// enroll the client as an owner, answer, then close.
    fn rescue_step<D: Device>(
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
    /// push one event or sample.
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
        if let Some((id, op, reply)) = self.poll_pending(index, now_ms, device)? {
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

    /// Answer or queue one request. `None` when the answer waits on the
    /// device (it then goes out from [`Self::poll_pending`]).
    #[expect(clippy::too_many_arguments, reason = "one request's whole context")]
    fn request<D: Device>(
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
                    writable: view.writable,
                }),
                SettingsResult::Busy => Reply::Refused(Refusal::Busy),
                SettingsResult::ReadOnly => Reply::Refused(Refusal::NotWritable),
            },
            Action::PutSettings { base, fields } => match self.put_settings(base, fields, device) {
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
            Action::TimeSync(fields) => match device.time_sync(fields) {
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
            Action::Invalid => Reply::Refused(Refusal::Invalid),
            Action::Feed { portions } => match device.feed(portions, by) {
                Ok(feed) => return self.wait(index, id, action, Waiting::Feed { feed }),
                Err(refusal) => Reply::Refused(refusal),
            },
            Action::CancelFeed { feed } => {
                if device.cancel_feed(feed) {
                    Reply::Done(None)
                } else {
                    Reply::Refused(Refusal::NotFound)
                }
            }
            Action::Reboot => {
                self.reboot_at = Some(now_ms.wrapping_add(REBOOT_MS));
                Reply::Done(None)
            }
        };
        Some(self.remember(memory, id, action, outcome))
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

    /// Hold the answer until the device finishes the write.
    fn wait(
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
    fn poll_pending<D: Device>(
        &mut self,
        index: usize,
        now_ms: u32,
        device: &mut D,
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
        };

        if let Phase::Open { pending, .. } = &mut self.slots[index].phase {
            *pending = None;
        }
        self.memories[memory].remember(pending.id, remembered(outcome));
        Ok(Some((pending.id, pending.op, outcome)))
    }

    /// Remember a write's outcome under its id.
    fn remember(&mut self, memory: usize, id: RequestId, action: Action, outcome: Reply) -> Reply {
        if action.writes() {
            self.memories[memory].remember(id, remembered(outcome));
        }
        outcome
    }

    /// Seal one request's answer. Kept out of line: the wire message is as
    /// large as its largest variant, so only this frame holds one.
    #[inline(never)]
    #[expect(clippy::too_many_arguments, reason = "one answer's whole context")]
    fn answer<D: Device>(
        &mut self,
        index: usize,
        id: RequestId,
        op: &'static str,
        reply: Reply,
        now_ms: u32,
        device: &mut D,
        record: &mut impl FnMut(Record),
    ) -> Result<(), Reason> {
        record(Record::Request {
            slot: slot_id(index),
            op,
            result: reply.name(),
        });
        let result = match reply {
            Reply::Clients => return self.list_clients(index, id),
            Reply::Done(generation) => Outcome::Done { generation },
            Reply::Refused(refusal) => Outcome::Refused(refusal),
            Reply::Settings(settings) => Outcome::Settings(settings),
            Reply::Invite(invite) => Outcome::Invite(invite),
            Reply::Feeding(feed) => Outcome::Feeding { feed },
            Reply::Status => {
                let mut status = device.status(now_ms);
                status.sessions = u8::try_from(self.sessions()).unwrap_or(u8::MAX);
                Outcome::Status(status)
            }
        };
        self.seal_and_send(index, &FeederMessage::Response { id, result })
    }

    /// The client list borrows labels from the table, so it is encoded here.
    fn list_clients(&mut self, index: usize, id: RequestId) -> Result<(), Reason> {
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

    /// Push the next event, or the latest sample, to a subscriber.
    fn push<D: Device>(&mut self, index: usize, device: &mut D) -> Result<bool, Reason> {
        let Phase::Open {
            events,
            sample,
            capabilities,
            ..
        } = &mut self.slots[index].phase
        else {
            return Ok(false);
        };
        if let Some(after) = *events {
            let message = match device.event_after(after) {
                EventRead::Event(event) => {
                    *events = Some(Some(event.seq));
                    // A body the session's hello did not claim would not
                    // decode there: skip it, as an old client would want.
                    if !capabilities.contains(event.body.capability()) {
                        return Ok(true);
                    }
                    Some(FeederMessage::Event(event))
                }
                EventRead::Gap { from, to } => {
                    *events = Some(Some(to));
                    Some(FeederMessage::Gap { from, to })
                }
                EventRead::Wait => return Ok(true),
                EventRead::None => None,
            };
            if let Some(message) = message {
                return self.seal_and_send(index, &message).map(|()| true);
            }
        }
        if let Some(next) = sample.take() {
            return self
                .seal_and_send(index, &FeederMessage::Sample(next))
                .map(|()| true);
        }
        Ok(false)
    }

    fn seal_and_send(&mut self, index: usize, message: &FeederMessage<'_>) -> Result<(), Reason> {
        let len = proto_encode(message, &mut self.plain)?;
        self.seal_plain(index, len)
    }

    /// Seal `self.plain[..len]` into the slot's transmit buffer.
    fn seal_plain(&mut self, index: usize, len: usize) -> Result<(), Reason> {
        let slot = &mut self.slots[index];
        let Phase::Open { transport, .. } = &mut slot.phase else {
            return Ok(());
        };
        let sealed = transport
            .seal(&self.plain[..len], &mut self.out)
            .map_err(|_| Reason::Noise)?;
        slot.wire.send(&self.out[..sealed])
    }

    /// End `index` with `reason`: anything queued still goes out, then the
    /// driver closes it.
    fn end(&mut self, index: usize, reason: Reason, record: &mut impl FnMut(Record)) {
        let slot = &mut self.slots[index];
        if matches!(slot.phase, Phase::Free | Phase::Closing) {
            return;
        }
        slot.phase = Phase::Closing;
        record(Record::Ended {
            slot: slot_id(index),
            reason,
        });
    }
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
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

/// Whether `role` may run `action`. Owners do everything; members and the
/// server feed, change settings and the lock, and read; nobody else opens a
/// session at all.
const fn allowed(role: Role, action: Action) -> bool {
    match role {
        Role::Owner => true,
        Role::Member | Role::Server => !matches!(
            action,
            Action::PutWifi { .. } | Action::RevokeClient { .. } | Action::Reboot
        ),
        Role::UpdateSigner => false,
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

fn find<'a>(clients: &'a [Option<Client>; CLIENTS], key: &[u8; 32]) -> Option<&'a Client> {
    clients.iter().flatten().find(|client| &client.key == key)
}

fn invite(id: u32, secret: [u8; 32]) -> proto::Invite {
    let seconds = pairing::Config::DEFAULT.invite_ms / 1000;
    proto::Invite {
        id,
        secret,
        seconds: u16::try_from(seconds).unwrap_or(u16::MAX),
    }
}

fn remembered(reply: Reply) -> Remembered {
    match reply {
        Reply::Refused(refusal) => Remembered::Refused(refusal),
        Reply::Done(generation) => Remembered::Done(generation),
        Reply::Invite(invite) => Remembered::Invite { id: invite.id },
        Reply::Feeding(feed) => Remembered::Feeding { feed },
        Reply::Status | Reply::Settings(_) | Reply::Clients => Remembered::Done(None),
    }
}

pub fn fingerprint(key: &[u8; 32]) -> Fingerprint {
    [key[0], key[1], key[2], key[3]]
}

fn slot_id(index: usize) -> u8 {
    u8::try_from(index).unwrap_or(u8::MAX)
}

fn proto_encode<T: serde::Serialize>(value: &T, out: &mut [u8]) -> Result<usize, Reason> {
    wellfed_proto::encode(value, out).map_err(|_| Reason::Malformed)
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
enum Action {
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
    /// A field failed the stores' checks before anything ran.
    Invalid,
}

impl From<Op<'_>> for Action {
    fn from(op: Op<'_>) -> Self {
        match op {
            Op::Status => Self::Status,
            Op::GetSettings => Self::GetSettings,
            Op::PutSettings { base, changes } => {
                if changes
                    .volume
                    .is_some_and(|v| v > wellfed_core::transport::MAX_VOLUME)
                {
                    return Self::Invalid;
                }
                let fields = SettingsFields {
                    lights: changes.lights,
                    sound: changes.sound,
                    volume: changes.volume,
                };
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
            Op::Feed { portions: 0 } => Self::Invalid,
            Op::Feed { portions } => Self::Feed { portions },
            Op::Reboot => Self::Reboot,
            Op::CancelFeed { feed } => Self::CancelFeed { feed },
        }
    }
}

impl Action {
    /// Whether the request changes something, so its id is remembered.
    const fn writes(self) -> bool {
        !matches!(
            self,
            Self::Status | Self::GetSettings | Self::ListClients | Self::Invalid
        )
    }

    const fn name(self) -> &'static str {
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
            Self::Invalid => "invalid",
        }
    }
}

/// A request's answer, compact: the status and the client list are read
/// only when the answer is sealed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reply {
    Done(Option<u32>),
    Refused(Refusal),
    Status,
    Settings(proto::Settings),
    Invite(proto::Invite),
    Clients,
    Feeding(u32),
}

fn saturate(jump_ms: i64) -> i32 {
    i32::try_from(jump_ms).unwrap_or(if jump_ms < 0 { i32::MIN } else { i32::MAX })
}

/// Names for records.
trait Name {
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
            },
        }
    }
}

/// Where the recovery session stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rescue {
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
