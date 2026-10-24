//! FW45 BLE setup mode: a new owner's phone (or the `petlibre` CLI) gives
//! the feeder Wi-Fi and its key, and learns the feeder's.
//!
//! Lock and Mic held on an unlocked panel start it (`pairing`). The loop
//! then brings up the BLE stack, which a normal boot never starts, and
//! advertises while the feeder is on mains, as stock does. Each connection
//! runs `petlibre_feeder::setup`'s responder under the feeder's identity:
//! 1. The client's ask arrives in the handshake's third message; pairing
//!    turns it into a pending offer, and the client is told to press
//!    Dispense.
//! 2. The press admits it. The settings writer enrolls the key as an owner
//!    (`petlibre_hal::keys`), then saves any Wi-Fi for the next join
//!    (`petlibre_hal::network`).
//! 3. The client gets `Done`, and the feeder reboots without BLE after
//!    [`REBOOT_MS`]. With no session for [`IDLE_MS`] it reboots too.
//!
//! No record carries a key, a label or a credential.

use core::fmt;

use petlibre_core::named_enum;
use petlibre_core::noise::Keypair;
use petlibre_core::store::clients::Role;
use petlibre_core::transport::{WifiFields, WifiResult};
use petlibre_feeder::pairing::{self, Pairing, Path};
use petlibre_feeder::setup::{MAX_MESSAGE, Offer, Received, Responder};
use petlibre_proto::setup::{Enrolled, Refusal, Reply, WifiSaved};
use petlibre_sdk::hal::ble::{self, BleSetup, DropReason, Link};
use petlibre_sdk::hal::keys::{self, Change, KeyResult, Requested};
use petlibre_sdk::hal::{network, power, rng};

/// After `Done`, long enough for the reply to reach the client.
pub const REBOOT_MS: u32 = 3_000;
/// Setup mode with no session this long ends with a reboot.
pub const IDLE_MS: u32 = 15 * 60_000;
/// After a final refusal the client may hang up first, so the reply is not
/// lost to a disconnect that overtakes it on the air; then the feeder does.
pub const LINGER_MS: u32 = 2_000;

named_enum! {
    pub enum Ended {
        Done = "done",
        Idle = "idle",
    }
}

named_enum! {
    /// Why a session failed before an offer.
    pub enum Fault {
        /// The identity is not loaded yet.
        Identity = "identity",
        /// The generator has no output for an ephemeral key.
        Generator = "generator",
        Handshake = "handshake",
        /// The client sent again after its ask.
        Unexpected = "unexpected",
        /// A pressed offer is still being written; a new client waits.
        Busy = "busy",
        Reply = "reply",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Start,
    Ble(ble::Record),
    Session,
    /// A checked ask, waiting for its press.
    Asked {
        wifi: bool,
    },
    Enroll {
        result: KeyResult,
    },
    Wifi {
        saved: bool,
    },
    Replied {
        reply: Reply,
    },
    Fault {
        fault: Fault,
    },
    Reboot {
        ended: Ended,
    },
}

/// One record's body; `PLBLE1` for the link's, `PLSET1` for the rest.
pub fn describe(record: Record, f: &mut impl fmt::Write) -> fmt::Result {
    match record {
        Record::Start => f.write_str("start"),
        Record::Ble(record) => ble::describe(record, f),
        Record::Session => f.write_str("session"),
        Record::Asked { wifi } => write!(f, "asked wifi={}", u8::from(wifi)),
        Record::Enroll { result } => write!(f, "enroll result={result}"),
        Record::Wifi { saved } => write!(f, "wifi saved={}", u8::from(saved)),
        Record::Replied { reply } => write!(f, "reply {}", ReplyText(reply)),
        Record::Fault { fault } => write!(f, "fault stage={fault}"),
        Record::Reboot { ended } => write!(f, "reboot ended={ended}"),
    }
}

struct ReplyText(Reply);

impl fmt::Display for ReplyText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Reply::Confirming { seconds } => write!(f, "confirming seconds={seconds}"),
            Reply::Done { enrolled, wifi } => write!(
                f,
                "done enrolled={} wifi={}",
                match enrolled {
                    Enrolled::New => "new",
                    Enrolled::Already => "already",
                },
                match wifi {
                    WifiSaved::Saved => "saved",
                    WifiSaved::Unchanged => "unchanged",
                }
            ),
            Reply::Refused(refusal) => write!(
                f,
                "refused reason={}",
                match refusal {
                    Refusal::Busy => "busy",
                    Refusal::Unconfirmed => "unconfirmed",
                    Refusal::Full => "full",
                    Refusal::NotWritable => "not-writable",
                    Refusal::Invalid => "invalid",
                }
            ),
        }
    }
}

/// Where the connected client's session stands. The responder and the
/// offer stay in their fields: moving them would cost the loop's stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    None,
    Handshake,
    /// Pairing holds the offer until a Dispense press or its timeout.
    Asked,
    /// The enrollment is queued (or waits for the mailbox: `ticket` 0).
    Enrolling {
        ticket: u32,
    },
    /// The Wi-Fi is queued; the store's generation before it.
    Saving {
        enrolled: Enrolled,
        before: u32,
    },
}

pub struct Setup {
    ble: BleSetup,
    stage: Stage,
    responder: Responder,
    offer: Option<Offer>,
    /// The last time a session started or setup mode began.
    active_ms: u32,
    reboot_at: Option<u32>,
    /// When to drop a link whose session ended with a refusal.
    close_at: Option<u32>,
    /// This step's time.
    now: u32,
    /// The next message to send.
    out: [u8; MAX_MESSAGE],
}

impl Setup {
    pub fn new(now_ms: u32) -> Self {
        Self {
            ble: BleSetup::default(),
            stage: Stage::None,
            responder: Responder::idle(),
            offer: None,
            active_ms: now_ms,
            reboot_at: None,
            close_at: None,
            now: now_ms,
            out: [0; MAX_MESSAGE],
        }
    }

    /// Setup has ended and its reply has had time to go out: the loop
    /// should reboot now (`petlibre_sdk::request_reset`). KM0 counts the
    /// watchdog reset only for an unconfirmed build, and setup starts from a
    /// gesture long after confirmation.
    pub fn reboot_due(&self, now: u32) -> bool {
        self.reboot_at
            .is_some_and(|at| petlibre_core::deadline_passed(now, at))
    }

    /// One step: the link, then the session. `event` is a setup-path event
    /// pairing produced this step (a press admitted the offer, or it timed
    /// out). Returns the delay to the next step.
    pub fn step(
        &mut self,
        now: u32,
        pairing: &mut Pairing,
        event: Option<pairing::Event>,
        identity: Option<&Keypair>,
        record: &mut impl FnMut(Record),
    ) -> u32 {
        self.now = now;
        if let Some(at) = self.close_at
            && petlibre_core::deadline_passed(now, at)
        {
            self.close_at = None;
            record(Record::Ble(self.ble.close(DropReason::Done)));
        }
        let mains = power::health(now).is_some_and(|power| power.adapter);
        let step = self.ble.step(mains && self.reboot_at.is_none());
        if let Some(r) = step.record {
            record(Record::Ble(r));
        }
        match step.link {
            Some(Link::Opened) => self.open(now, identity, record),
            Some(Link::Message) => self.message(now, pairing, record),
            Some(Link::Closed) => self.closed(pairing),
            None => {}
        }
        if let Some(event) = event {
            self.decided(event, record);
        }
        self.advance(now, record);

        if self.reboot_at.is_none()
            && self.stage == Stage::None
            && petlibre_core::deadline_passed(now, self.active_ms.wrapping_add(IDLE_MS))
        {
            record(Record::Reboot { ended: Ended::Idle });
            self.reboot_at = Some(now);
        }
        step.delay_ms
    }

    /// The offer has its press and is with the settings writer.
    fn admitted(&self) -> bool {
        matches!(self.stage, Stage::Enrolling { .. } | Stage::Saving { .. })
    }

    fn open(&mut self, now: u32, identity: Option<&Keypair>, record: &mut impl FnMut(Record)) {
        if self.admitted() {
            // A pressed offer is still being written; it finishes first.
            return self.drop_link(Fault::Busy, record);
        }
        self.active_ms = now;
        let Some(identity) = identity else {
            return self.fail(Fault::Identity, record);
        };
        let mut ephemeral = [0u8; 32];
        if !rng::fill(&mut ephemeral) {
            return self.fail(Fault::Generator, record);
        }
        if self.responder.restart(identity, ephemeral).is_err() {
            return self.fail(Fault::Handshake, record);
        }
        self.stage = Stage::Handshake;
        record(Record::Session);
    }

    fn message(&mut self, now: u32, pairing: &mut Pairing, record: &mut impl FnMut(Record)) {
        if self.stage != Stage::Handshake {
            // The client speaks once; anything more ends its link. An offer
            // waiting for its press goes with it, a pressed one does not.
            self.ble.consume();
            if self.admitted() {
                return self.drop_link(Fault::Unexpected, record);
            }
            if self.stage == Stage::Asked {
                pairing.cancel(Path::Setup);
            }
            return self.fail(Fault::Unexpected, record);
        }
        let Some(message) = self.ble.message() else {
            return;
        };
        let received = self.responder.receive(message, &mut self.out);
        self.ble.consume();

        match received {
            Ok(Received::Send(len)) => {
                if self.ble.send(&self.out[..len]).is_err() {
                    self.fail(Fault::Reply, record);
                }
            }
            Ok(Received::Offer(offer)) => {
                record(Record::Asked {
                    wifi: offer.wifi.is_some(),
                });
                if let pairing::Event::Confirming { .. } = pairing.setup(now) {
                    let seconds = u16::try_from(pairing::Config::DEFAULT.confirm_ms / 1000)
                        .unwrap_or(u16::MAX);
                    self.offer = Some(offer);
                    self.stage = Stage::Asked;
                    self.reply(Reply::Confirming { seconds }, record);
                } else {
                    self.finish(Reply::Refused(Refusal::Busy), record);
                }
            }
            Err(_) => self.fail(Fault::Handshake, record),
        }
    }

    /// The client left. An admitted offer still completes: the owner
    /// pressed Dispense, and the client can find the feeder on Wi-Fi.
    fn closed(&mut self, pairing: &mut Pairing) {
        self.close_at = None;
        match self.stage {
            Stage::Handshake => self.end(),
            Stage::Asked => {
                pairing.cancel(Path::Setup);
                self.end();
            }
            _ => {}
        }
    }

    /// Pairing decided the pending offer.
    fn decided(&mut self, event: pairing::Event, record: &mut impl FnMut(Record)) {
        if self.stage != Stage::Asked {
            return;
        }
        match event {
            pairing::Event::Admit {
                path: Path::Setup,
                role: Role::Owner,
            } => self.stage = Stage::Enrolling { ticket: 0 },
            pairing::Event::Refused {
                path: Path::Setup, ..
            } => self.finish(Reply::Refused(Refusal::Unconfirmed), record),
            _ => {}
        }
    }

    /// Move an admitted offer through the writer: enroll, then Wi-Fi.
    fn advance(&mut self, now: u32, record: &mut impl FnMut(Record)) {
        let Some(offer) = self.offer else {
            return;
        };
        match self.stage {
            Stage::Enrolling { ticket: 0 } => {
                let enroll = Change::Enroll {
                    key: offer.key,
                    label: offer.label,
                    role: Role::Owner,
                };
                // A busy mailbox (another change waits): ask again next step.
                if keys::request(enroll) == Requested::Queued {
                    let ticket = keys::ticket().max(1);
                    self.stage = Stage::Enrolling { ticket };
                }
            }
            Stage::Enrolling { ticket } => {
                if let Some(result) = keys::outcome(ticket) {
                    record(Record::Enroll { result });
                    self.enrolled(now, offer, result, record);
                }
            }
            Stage::Saving { enrolled, before } if !network::saving() => {
                let saved = generation() != Some(before);
                record(Record::Wifi { saved });
                if saved {
                    let wifi = WifiSaved::Saved;
                    self.done(now, Reply::Done { enrolled, wifi }, record);
                } else {
                    self.finish(Reply::Refused(Refusal::NotWritable), record);
                }
            }
            _ => {}
        }
    }

    fn enrolled(
        &mut self,
        now: u32,
        offer: Offer,
        result: KeyResult,
        record: &mut impl FnMut(Record),
    ) {
        let enrolled = match result {
            KeyResult::Done => Enrolled::New,
            KeyResult::Duplicate => Enrolled::Already,
            KeyResult::Full => return self.finish(Reply::Refused(Refusal::Full), record),
            _ => return self.finish(Reply::Refused(Refusal::NotWritable), record),
        };

        let Some(credentials) = offer.wifi else {
            let wifi = WifiSaved::Unchanged;
            return self.done(now, Reply::Done { enrolled, wifi }, record);
        };
        let before = generation().unwrap_or(0);
        match network::save(credentials) {
            WifiResult::Current { .. } => self.stage = Stage::Saving { enrolled, before },
            WifiResult::Invalid => self.finish(Reply::Refused(Refusal::Invalid), record),
            WifiResult::Busy | WifiResult::ReadOnly => {
                self.finish(Reply::Refused(Refusal::NotWritable), record);
            }
        }
    }

    /// The client is an owner: answer, then reboot without BLE.
    fn done(&mut self, now: u32, reply: Reply, record: &mut impl FnMut(Record)) {
        self.reply(reply, record);
        self.end();
        record(Record::Reboot { ended: Ended::Done });
        self.reboot_at = Some(now.wrapping_add(REBOOT_MS));
    }

    /// A last reply, then the link closes after [`LINGER_MS`]; setup mode
    /// stays for another try.
    fn finish(&mut self, reply: Reply, record: &mut impl FnMut(Record)) {
        self.reply(reply, record);
        self.close_at = Some(self.now.wrapping_add(LINGER_MS));
        self.end();
    }

    fn reply(&mut self, reply: Reply, record: &mut impl FnMut(Record)) {
        record(Record::Replied { reply });
        let sent = self
            .responder
            .reply(&reply, &mut self.out)
            .ok()
            .is_some_and(|len| self.ble.send(&self.out[..len]).is_ok());
        if !sent {
            record(Record::Fault {
                fault: Fault::Reply,
            });
        }
    }

    /// End the session and drop its link.
    fn fail(&mut self, fault: Fault, record: &mut impl FnMut(Record)) {
        self.end();
        self.drop_link(fault, record);
    }

    /// Drop the link but keep the session (a pressed offer completes).
    fn drop_link(&mut self, fault: Fault, record: &mut impl FnMut(Record)) {
        record(Record::Fault { fault });
        record(Record::Ble(self.ble.close(DropReason::Protocol)));
    }

    /// Forget the session: its keys, its offer and the credentials in it.
    fn end(&mut self) {
        self.stage = Stage::None;
        self.responder.stop();
        self.offer = None;
    }
}

/// The Wi-Fi store's generation, once the writer has loaded it.
fn generation() -> Option<u32> {
    match network::request(WifiFields::default()) {
        WifiResult::Current { view, .. } => Some(view.generation),
        _ => None,
    }
}
