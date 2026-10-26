//! BLE setup mode: a new owner's phone (or the `wellfed` CLI) gives
//! the feeder Wi-Fi and its key, and learns the feeder's.
//!
//! Lock and Mic held on an unlocked panel start it (`pairing`). The loop
//! then brings up the BLE stack, which a normal boot never starts, and
//! advertises while the feeder is on mains, as stock does. Each connection
//! runs `wellfed_feeder::setup`'s responder under the feeder's identity:
//! 1. The client's ask arrives in the handshake's third message; pairing
//!    turns it into a pending offer, and the client is told to have the
//!    owner key its code in.
//! 2. With the code in, the responder and the client prove it to each
//!    other (`wellfed_session::setup_code`), within [`PROVE_MS`]. A client
//!    that proves it is the one the owner set up; one that does not, a
//!    relay among them, is refused.
//! 3. The proven client's last step carries its Wi-Fi. The settings writer
//!    enrolls the key as an owner (`wellfed_hal::keys`), then saves any
//!    Wi-Fi for the next join (`wellfed_hal::network`).
//! 4. The client gets `Done`, and the feeder reboots without BLE after
//!    [`REBOOT_MS`]. With no session for [`IDLE_MS`] it reboots too.
//!
//! The speaker answers the owner at the feeder: a `key` tick for each press
//! of the code, then `coin` when the client is enrolled, or `alert` when the
//! code was wrong or never finished. The feeder's two firmware tasks are
//! taken, so this loop steps the speaker itself, and only in setup mode;
//! the `sound` setting mutes it as it mutes every tune.
//!
//! No record carries a key, a label, a credential or the code.

use core::fmt;

use wellfed_core::named_enum;
use wellfed_core::tone::Tune;
use wellfed_feeder::mailbox::{PlayResult, WifiResult};
use wellfed_feeder::pairing::{self, Pairing, Path};
use wellfed_feeder::setup::{self as session, MAX_MESSAGE, Offer, Received, Responder};
use wellfed_feeder::store::clients::Role;
use wellfed_feeder::store::net::Credentials;
use wellfed_proto::setup::{Enrolled, Refusal, Reply, WifiSaved};
use wellfed_sdk::hal::audio::{self, Speaker};
use wellfed_sdk::hal::ble::{self, BleSetup, DropReason, Link};
use wellfed_sdk::hal::keys::{self, Change, KeyResult, Requested};
use wellfed_sdk::hal::{network, power, rng};
use wellfed_session::noise::Keypair;
use wellfed_session::setup_code::Code;

/// After `Done`, long enough for the reply to reach the client.
pub const REBOOT_MS: u32 = 3_000;
/// From the last press of the code to the client's last step: the proof
/// is a few round trips, so a client that stalls longer is refused.
pub const PROVE_MS: u32 = 15_000;
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
        /// A proven offer is still being written; a new client waits.
        Busy = "busy",
        /// The code proof broke off: a malformed or misordered step.
        Proof = "proof",
        Reply = "reply",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Start,
    Ble(ble::Record),
    /// The speaker's feedback (`PLAUD1`).
    Audio(audio::Record),
    Session,
    /// A checked ask, waiting for its code.
    Asked,
    /// The code is in; the proof runs.
    Proving,
    /// The client proved the code.
    Proven {
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
        Record::Audio(record) => audio::describe(record, f),
        Record::Session => f.write_str("session"),
        Record::Asked => f.write_str("asked"),
        Record::Proving => f.write_str("proving"),
        Record::Proven { wifi } => write!(f, "proven wifi={}", u8::from(wifi)),
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
            Reply::EnterCode { seconds } => write!(f, "enter-code seconds={seconds}"),
            Reply::Round { .. } => f.write_str("round"),
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
                    Refusal::WrongCode => "wrong-code",
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
    /// Pairing holds the offer until its code is in or it times out.
    Asked,
    /// The code proof runs until the client's last step or `until`.
    Proving {
        until: u32,
    },
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
    on: bool,
    ble: BleSetup,
    stage: Stage,
    responder: Responder,
    offer: Option<Offer>,
    /// The proven client's Wi-Fi.
    wifi: Option<Credentials>,
    /// The last time a session started or setup mode began.
    active_ms: u32,
    reboot_at: Option<u32>,
    /// When to drop a link whose session ended with a refusal.
    close_at: Option<u32>,
    speaker: Speaker,
    /// Counted presses whose tick has not started yet.
    ticks: u8,
    /// The session's outcome tune, after the ticks.
    outcome: Option<Tune>,
    /// This step's time.
    now: u32,
    /// The next message to send.
    out: [u8; MAX_MESSAGE],
}

impl Setup {
    /// Setup mode off. Built at compile time and started in place
    /// ([`Self::start`]): the state is two kilobytes, and moving it put two
    /// copies on the loop's stack.
    pub const fn off() -> Self {
        Self {
            on: false,
            ble: BleSetup::new(),
            stage: Stage::None,
            responder: Responder::idle(),
            offer: None,
            wifi: None,
            active_ms: 0,
            reboot_at: None,
            close_at: None,
            speaker: Speaker::quiet(),
            ticks: 0,
            outcome: None,
            now: 0,
            out: [0; MAX_MESSAGE],
        }
    }

    /// Setup mode, started at `now_ms` (host tests).
    pub fn new(now_ms: u32) -> Self {
        let mut setup = Self::off();
        setup.start(now_ms);
        setup
    }

    /// Start setup mode in place; it runs until the reboot that ends it.
    pub fn start(&mut self, now_ms: u32) {
        self.on = true;
        self.ble = BleSetup::new();
        self.stage = Stage::None;
        self.responder.stop();
        self.offer = None;
        self.wifi = None;
        self.active_ms = now_ms;
        self.reboot_at = None;
        self.close_at = None;
        self.speaker = Speaker::quiet();
        self.ticks = 0;
        self.outcome = None;
        self.now = now_ms;
    }

    pub const fn is_on(&self) -> bool {
        self.on
    }

    /// A press counted towards the code: tick for it.
    pub fn pressed(&mut self) {
        self.ticks = self.ticks.saturating_add(1);
    }

    /// Setup has ended and its reply has had time to go out: the loop
    /// should reboot now (`wellfed_sdk::request_reset`). KM0 counts the
    /// watchdog reset only for an unconfirmed build, and setup starts from a
    /// gesture long after confirmation.
    pub fn reboot_due(&self, now: u32) -> bool {
        self.on
            && self
                .reboot_at
                .is_some_and(|at| wellfed_core::deadline_passed(now, at))
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
            && wellfed_core::deadline_passed(now, at)
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
        if let Stage::Proving { until } = self.stage
            && wellfed_core::deadline_passed(now, until)
        {
            self.finish(Reply::Refused(Refusal::Unconfirmed), record);
        }
        self.advance(now, record);

        if self.reboot_at.is_none()
            && self.stage == Stage::None
            && wellfed_core::deadline_passed(now, self.active_ms.wrapping_add(IDLE_MS))
        {
            record(Record::Reboot { ended: Ended::Idle });
            self.reboot_at = Some(now);
        }
        let sound = self.sound(record);
        sound.map_or(step.delay_ms, |delay| step.delay_ms.min(delay))
    }

    /// Start the next feedback tune once the speaker is free, and step it
    /// while it plays; the speaker's delay then, `None` when it is idle.
    fn sound(&mut self, record: &mut impl FnMut(Record)) -> Option<u32> {
        let next = if self.ticks > 0 {
            Some(Tune::Key)
        } else {
            self.outcome
        };
        if let Some(tune) = next
            && self.speaker.is_idle()
        {
            match audio::request(tune) {
                PlayResult::Started if self.ticks > 0 => self.ticks -= 1,
                PlayResult::Started => self.outcome = None,
                PlayResult::Muted => {
                    self.ticks = 0;
                    self.outcome = None;
                }
                // The settings are not loaded yet; try again next step.
                PlayResult::Busy => {}
            }
        }
        if self.speaker.is_idle() {
            return None;
        }

        let step = self.speaker.step();
        if let Some(r) = step.record {
            record(Record::Audio(r));
        }
        Some(step.delay_ms)
    }

    /// The offer is proven and with the settings writer.
    fn admitted(&self) -> bool {
        matches!(self.stage, Stage::Enrolling { .. } | Stage::Saving { .. })
    }

    fn open(&mut self, now: u32, identity: Option<&Keypair>, record: &mut impl FnMut(Record)) {
        if self.admitted() {
            // A proven offer is still being written; it finishes first.
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
        if !matches!(self.stage, Stage::Handshake | Stage::Proving { .. }) {
            // The client speaks only in the handshake and the proof;
            // anything more ends its link. An offer waiting for its code
            // goes with it, a proven one does not.
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
                record(Record::Asked);
                // There is one link, so a setup offer still pending belongs
                // to a client that is gone, even if its disconnect was never
                // seen (physical, Oct 24, `fw45-code-09`): it would otherwise
                // refuse this client and take the owner's presses for nobody.
                pairing.cancel(Path::Setup);
                if let pairing::Event::Confirming { .. } = pairing.setup(now) {
                    let seconds = u16::try_from(pairing::Config::DEFAULT.confirm_ms / 1000)
                        .unwrap_or(u16::MAX);
                    self.offer = Some(offer);
                    self.stage = Stage::Asked;
                    self.reply(Reply::EnterCode { seconds }, record);
                } else {
                    self.finish(Reply::Refused(Refusal::Busy), record);
                }
            }
            Ok(Received::Proven { wifi }) => {
                record(Record::Proven {
                    wifi: wifi.is_some(),
                });
                self.wifi = wifi;
                self.stage = Stage::Enrolling { ticket: 0 };
            }
            Err(session::Error::WrongCode) => {
                self.finish(Reply::Refused(Refusal::WrongCode), record);
            }
            Err(session::Error::Invalid) => self.finish(Reply::Refused(Refusal::Invalid), record),
            Err(_) if self.stage == Stage::Handshake => self.fail(Fault::Handshake, record),
            Err(_) => self.fail(Fault::Proof, record),
        }
    }

    /// The client left. A proven offer still completes: the client holds
    /// the owner's code, and it can find the feeder on Wi-Fi.
    fn closed(&mut self, pairing: &mut Pairing) {
        self.close_at = None;
        match self.stage {
            Stage::Handshake | Stage::Proving { .. } => self.end(),
            Stage::Asked => {
                pairing.cancel(Path::Setup);
                self.end();
            }
            _ => {}
        }
    }

    /// Pairing decided the pending offer: its code is in, or it timed out.
    fn decided(&mut self, event: pairing::Event, record: &mut impl FnMut(Record)) {
        if self.stage != Stage::Asked {
            return;
        }
        match event {
            pairing::Event::Entered {
                path: Path::Setup,
                code,
            } => self.prove(code, record),
            pairing::Event::Refused {
                path: Path::Setup, ..
            } => self.finish(Reply::Refused(Refusal::Unconfirmed), record),
            _ => {}
        }
    }

    /// Start the code proof with the client.
    fn prove(&mut self, code: Code, record: &mut impl FnMut(Record)) {
        let mut seed = [0u8; 32];
        if !rng::fill(&mut seed) {
            return self.fail(Fault::Generator, record);
        }
        let Ok(len) = self.responder.prove(code, seed, &mut self.out) else {
            return self.fail(Fault::Proof, record);
        };
        if self.ble.send(&self.out[..len]).is_err() {
            return self.fail(Fault::Reply, record);
        }
        record(Record::Proving);
        self.stage = Stage::Proving {
            until: self.now.wrapping_add(PROVE_MS),
        };
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
                    self.enrolled(now, result, record);
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

    fn enrolled(&mut self, now: u32, result: KeyResult, record: &mut impl FnMut(Record)) {
        let enrolled = match result {
            KeyResult::Done => Enrolled::New,
            KeyResult::Duplicate => Enrolled::Already,
            KeyResult::Full => return self.finish(Reply::Refused(Refusal::Full), record),
            _ => return self.finish(Reply::Refused(Refusal::NotWritable), record),
        };

        let Some(credentials) = self.wifi else {
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
        self.outcome = Some(Tune::Coin);
        self.reply(reply, record);
        self.end();
        record(Record::Reboot { ended: Ended::Done });
        self.reboot_at = Some(now.wrapping_add(REBOOT_MS));
    }

    /// A last reply, then the link closes after [`LINGER_MS`]; setup mode
    /// stays for another try.
    fn finish(&mut self, reply: Reply, record: &mut impl FnMut(Record)) {
        // The owner at the feeder hears that the code did not take.
        if matches!(
            reply,
            Reply::Refused(Refusal::WrongCode | Refusal::Unconfirmed)
        ) {
            self.outcome = Some(Tune::Alert);
        }
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
        self.wifi = None;
    }
}

/// The Wi-Fi store's generation, once the writer has loaded it.
fn generation() -> Option<u32> {
    match network::current() {
        WifiResult::Current { view, .. } => Some(view.generation),
        _ => None,
    }
}
