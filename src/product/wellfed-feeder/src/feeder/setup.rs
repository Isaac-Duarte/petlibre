//! BLE setup mode: a new owner's phone (or the `wellfed` CLI) gives
//! the feeder Wi-Fi and its key, and learns the feeder's.
//!
//! Lock and Mic held on an unlocked panel start it (`pairing`), and so does
//! a boot that has no network to join: no build Wi-Fi and none stored
//! ([`Cause::Unprovisioned`], the owner's step). The loop then brings up
//! the BLE stack, which a provisioned boot never starts, and advertises
//! while the feeder is on mains, as stock does. Each connection
//! runs [`wellfed_device_api::setup`]'s responder under the feeder's identity:
//! 1. The client's ask arrives in the handshake's third message; pairing
//!    turns it into a pending offer, and the client is told to have the
//!    owner key its code in.
//! 2. With the code in, the responder and the client prove it to each
//!    other (`wellfed_session::setup_code`), within [`PROVE_MS`]. A client
//!    that proves it is the one the owner set up; one that does not, a
//!    relay among them, is refused.
//! 3. The proven client's last step carries its Wi-Fi. The settings writer
//!    enrolls the key as an owner (`Platform::keys_request`), then saves any
//!    Wi-Fi for the next join (`Platform::wifi_change`).
//! 4. The client gets `Done`, and the feeder reboots without BLE after
//!    [`REBOOT_MS`]. With no session for [`IDLE_MS`] it reboots too.
//!
//! The speaker answers the owner at the feeder: a `key` tick for each press
//! of the code, then `coin` when the client is enrolled, or `alert` when the
//! code was wrong or never finished. The feeder's two firmware tasks are
//! taken, so this loop steps the speaker itself, and only in setup mode;
//! the `sound` setting mutes it as it mutes every tune. The link and the
//! speaker are the platform's drivers, which make their own records.
//!
//! No record carries a key, a label, a credential or the code.

use core::fmt;

use wellfed_core::named_enum;
use wellfed_core::tone::Tune;
use wellfed_proto::setup::{Enrolled, Refusal, Reply, WifiSaved};
use wellfed_session::noise::Keypair;
use wellfed_session::setup_code::Code;

use super::platform::{BleDrop, BleLink, Platform};
use wellfed_device_api::clients::Role;
use wellfed_device_api::mailbox::{ClientChange, KeyResult, PlayResult, Requested, WifiResult};
use wellfed_device_api::pairing::{self, Pairing, Path};
use wellfed_device_api::setup::{self as session, MAX_MESSAGE, Offer, Received, Responder};
use wellfed_device_api::wifi::Credentials;

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
    /// What started setup mode.
    pub enum Cause {
        /// Lock and Mic held on an unlocked panel.
        Hold = "hold",
        /// The boot found no network to join: no build Wi-Fi and none
        /// stored.
        Unprovisioned = "unprovisioned",
    }
}

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
pub enum Record<D> {
    Start {
        cause: Cause,
    },
    /// The link's or the speaker's own record, under its own tag.
    Driver(D),
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

/// One record's body; `PLSET1` for setup mode's own, the driver's tag for
/// its records.
pub fn describe<D: fmt::Display>(record: Record<D>, f: &mut impl fmt::Write) -> fmt::Result {
    match record {
        Record::Start { cause } => write!(f, "start cause={cause}"),
        Record::Driver(record) => write!(f, "{record}"),
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
    /// What started setup mode; `None` while it is off.
    cause: Option<Cause>,
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
            cause: None,
            stage: Stage::None,
            responder: Responder::idle(),
            offer: None,
            wifi: None,
            active_ms: 0,
            reboot_at: None,
            close_at: None,
            ticks: 0,
            outcome: None,
            now: 0,
            out: [0; MAX_MESSAGE],
        }
    }

    /// Setup mode, started by a hold at `now_ms` (host tests).
    pub fn new(platform: &mut impl Platform, now_ms: u32) -> Self {
        let mut setup = Self::off();
        setup.start(platform, now_ms, Cause::Hold);
        setup
    }

    /// Start setup mode in place; it runs until the reboot that ends it.
    pub fn start(&mut self, platform: &mut impl Platform, now_ms: u32, cause: Cause) {
        self.cause = Some(cause);
        platform.ble_restart();
        self.stage = Stage::None;
        self.responder.stop();
        self.offer = None;
        self.wifi = None;
        self.active_ms = now_ms;
        self.reboot_at = None;
        self.close_at = None;
        platform.speaker_restart();
        self.ticks = 0;
        self.outcome = None;
        self.now = now_ms;
    }

    pub const fn is_on(&self) -> bool {
        self.cause.is_some()
    }

    /// Whether setup mode refuses feeds now. One the owner asked for with
    /// the hold always does. One a boot with no network started may run
    /// for every boot of a feeder that never gets Wi-Fi, so it lets the
    /// panel's Dispense feed while no session runs; a session's code is
    /// keyed in on the same buttons, and a feed must not hold its reboot.
    pub fn holds_feeds(&self) -> bool {
        match self.cause {
            None => false,
            Some(Cause::Hold) => true,
            Some(Cause::Unprovisioned) => self.stage != Stage::None || self.reboot_at.is_some(),
        }
    }

    /// A press counted towards the code: tick for it.
    pub fn pressed(&mut self) {
        self.ticks = self.ticks.saturating_add(1);
    }

    /// Setup has ended and its reply has had time to go out: the loop
    /// should reboot now (`Platform::reset`). The restart is a deliberate
    /// one, which ends KM0's reset-storm count, so a setup that finishes
    /// within minutes of its boot is never counted as a fault. With no
    /// session for [`IDLE_MS`] it reboots the same way; an unprovisioned
    /// feeder then enters setup again on the next boot.
    pub fn reboot_due(&self, now: u32) -> bool {
        self.is_on()
            && self
                .reboot_at
                .is_some_and(|at| wellfed_core::deadline_passed(now, at))
    }

    /// One step: the link, then the session. `event` is a setup-path event
    /// pairing produced this step (a press admitted the offer, or it timed
    /// out). Returns the delay to the next step.
    pub fn step<P: Platform>(
        &mut self,
        platform: &mut P,
        now: u32,
        pairing: &mut Pairing,
        event: Option<pairing::Event>,
        identity: Option<&Keypair>,
        record: &mut impl FnMut(Record<P::Record>),
    ) -> u32 {
        self.now = now;
        if let Some(at) = self.close_at
            && wellfed_core::deadline_passed(now, at)
        {
            self.close_at = None;
            if let Some(r) = platform.ble_close(BleDrop::Done) {
                record(Record::Driver(r));
            }
        }
        let mains = platform.power(now).is_some_and(|power| power.adapter);
        let step = platform.ble_step(mains && self.reboot_at.is_none());
        if let Some(r) = step.record {
            record(Record::Driver(r));
        }
        match step.link {
            Some(BleLink::Opened) => self.open(platform, now, identity, record),
            Some(BleLink::Message) => self.message(platform, now, pairing, record),
            Some(BleLink::Closed) => self.closed(pairing),
            None => {}
        }
        if let Some(event) = event {
            self.decided(platform, event, record);
        }
        if let Stage::Proving { until } = self.stage
            && wellfed_core::deadline_passed(now, until)
        {
            self.finish(platform, Reply::Refused(Refusal::Unconfirmed), record);
        }
        self.advance(platform, now, record);

        if self.reboot_at.is_none()
            && self.stage == Stage::None
            && wellfed_core::deadline_passed(now, self.active_ms.wrapping_add(IDLE_MS))
        {
            record(Record::Reboot { ended: Ended::Idle });
            self.reboot_at = Some(now);
        }
        let sound = self.sound(platform, record);
        sound.map_or(step.delay_ms, |delay| step.delay_ms.min(delay))
    }

    /// Start the next feedback tune once the speaker is free, and step it
    /// while it plays; the speaker's delay then, `None` when it is idle.
    fn sound<P: Platform>(
        &mut self,
        platform: &mut P,
        record: &mut impl FnMut(Record<P::Record>),
    ) -> Option<u32> {
        let next = if self.ticks > 0 {
            Some(Tune::Key)
        } else {
            self.outcome
        };
        if let Some(tune) = next
            && platform.speaker_idle()
        {
            match platform.play(tune) {
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
        if platform.speaker_idle() {
            return None;
        }

        let step = platform.speaker_step();
        if let Some(r) = step.record {
            record(Record::Driver(r));
        }
        Some(step.delay_ms)
    }

    /// The offer is proven and with the settings writer.
    fn admitted(&self) -> bool {
        matches!(self.stage, Stage::Enrolling { .. } | Stage::Saving { .. })
    }

    fn open<P: Platform>(
        &mut self,
        platform: &mut P,
        now: u32,
        identity: Option<&Keypair>,
        record: &mut impl FnMut(Record<P::Record>),
    ) {
        if self.admitted() {
            // A proven offer is still being written; it finishes first.
            return self.drop_link(platform, Fault::Busy, record);
        }
        self.active_ms = now;
        let Some(identity) = identity else {
            return self.fail(platform, Fault::Identity, record);
        };
        let mut ephemeral = [0u8; 32];
        if !platform.random(&mut ephemeral) {
            return self.fail(platform, Fault::Generator, record);
        }
        if self.responder.restart(identity, ephemeral).is_err() {
            return self.fail(platform, Fault::Handshake, record);
        }
        self.stage = Stage::Handshake;
        record(Record::Session);
    }

    fn message<P: Platform>(
        &mut self,
        platform: &mut P,
        now: u32,
        pairing: &mut Pairing,
        record: &mut impl FnMut(Record<P::Record>),
    ) {
        if !matches!(self.stage, Stage::Handshake | Stage::Proving { .. }) {
            // The client speaks only in the handshake and the proof;
            // anything more ends its link. An offer waiting for its code
            // goes with it, a proven one does not.
            platform.ble_consume();
            if self.admitted() {
                return self.drop_link(platform, Fault::Unexpected, record);
            }
            if self.stage == Stage::Asked {
                pairing.cancel(Path::Setup);
            }
            return self.fail(platform, Fault::Unexpected, record);
        }
        let Some(message) = platform.ble_message() else {
            return;
        };
        let received = self.responder.receive(message, &mut self.out);
        platform.ble_consume();

        match received {
            Ok(Received::Send(len)) => {
                if !platform.ble_send(&self.out[..len]) {
                    self.fail(platform, Fault::Reply, record);
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
                    self.reply(platform, Reply::EnterCode { seconds }, record);
                } else {
                    self.finish(platform, Reply::Refused(Refusal::Busy), record);
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
                self.finish(platform, Reply::Refused(Refusal::WrongCode), record);
            }
            Err(session::Error::Invalid) => {
                self.finish(platform, Reply::Refused(Refusal::Invalid), record);
            }
            Err(_) if self.stage == Stage::Handshake => {
                self.fail(platform, Fault::Handshake, record);
            }
            Err(_) => self.fail(platform, Fault::Proof, record),
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
    fn decided<P: Platform>(
        &mut self,
        platform: &mut P,
        event: pairing::Event,
        record: &mut impl FnMut(Record<P::Record>),
    ) {
        if self.stage != Stage::Asked {
            return;
        }
        match event {
            pairing::Event::Entered {
                path: Path::Setup,
                code,
            } => self.prove(platform, code, record),
            pairing::Event::Refused {
                path: Path::Setup, ..
            } => self.finish(platform, Reply::Refused(Refusal::Unconfirmed), record),
            _ => {}
        }
    }

    /// Start the code proof with the client.
    fn prove<P: Platform>(
        &mut self,
        platform: &mut P,
        code: Code,
        record: &mut impl FnMut(Record<P::Record>),
    ) {
        let mut seed = [0u8; 32];
        if !platform.random(&mut seed) {
            return self.fail(platform, Fault::Generator, record);
        }
        let Ok(len) = self.responder.prove(code, seed, &mut self.out) else {
            return self.fail(platform, Fault::Proof, record);
        };
        if !platform.ble_send(&self.out[..len]) {
            return self.fail(platform, Fault::Reply, record);
        }
        record(Record::Proving);
        self.stage = Stage::Proving {
            until: self.now.wrapping_add(PROVE_MS),
        };
    }

    /// Move an admitted offer through the writer: enroll, then Wi-Fi.
    fn advance<P: Platform>(
        &mut self,
        platform: &mut P,
        now: u32,
        record: &mut impl FnMut(Record<P::Record>),
    ) {
        let Some(offer) = self.offer else {
            return;
        };
        match self.stage {
            Stage::Enrolling { ticket: 0 } => {
                let enroll = ClientChange::Enroll {
                    key: offer.key,
                    label: offer.label,
                    role: Role::Owner,
                };
                // A busy mailbox (another change waits): ask again next step.
                if platform.keys_request(enroll) == Requested::Queued {
                    let ticket = platform.keys_ticket().max(1);
                    self.stage = Stage::Enrolling { ticket };
                }
            }
            Stage::Enrolling { ticket } => {
                if let Some(result) = platform.keys_outcome(ticket) {
                    record(Record::Enroll { result });
                    self.enrolled(platform, now, result, record);
                }
            }
            Stage::Saving { enrolled, before } if !platform.wifi_saving() => {
                let saved = generation(platform) != Some(before);
                record(Record::Wifi { saved });
                if saved {
                    let wifi = WifiSaved::Saved;
                    self.done(platform, now, Reply::Done { enrolled, wifi }, record);
                } else {
                    self.finish(platform, Reply::Refused(Refusal::NotWritable), record);
                }
            }
            _ => {}
        }
    }

    fn enrolled<P: Platform>(
        &mut self,
        platform: &mut P,
        now: u32,
        result: KeyResult,
        record: &mut impl FnMut(Record<P::Record>),
    ) {
        let enrolled = match result {
            KeyResult::Done => Enrolled::New,
            KeyResult::Duplicate => Enrolled::Already,
            KeyResult::Full => return self.finish(platform, Reply::Refused(Refusal::Full), record),
            _ => return self.finish(platform, Reply::Refused(Refusal::NotWritable), record),
        };

        let Some(credentials) = self.wifi else {
            let wifi = WifiSaved::Unchanged;
            return self.done(platform, now, Reply::Done { enrolled, wifi }, record);
        };
        let before = generation(platform).unwrap_or(0);
        match platform.wifi_change(Some(credentials)) {
            WifiResult::Current { .. } => self.stage = Stage::Saving { enrolled, before },
            WifiResult::Invalid => self.finish(platform, Reply::Refused(Refusal::Invalid), record),
            WifiResult::Busy | WifiResult::ReadOnly => {
                self.finish(platform, Reply::Refused(Refusal::NotWritable), record);
            }
        }
    }

    /// The client is an owner: answer, then reboot without BLE.
    fn done<P: Platform>(
        &mut self,
        platform: &mut P,
        now: u32,
        reply: Reply,
        record: &mut impl FnMut(Record<P::Record>),
    ) {
        self.outcome = Some(Tune::Coin);
        self.reply(platform, reply, record);
        self.end();
        record(Record::Reboot { ended: Ended::Done });
        self.reboot_at = Some(now.wrapping_add(REBOOT_MS));
    }

    /// A last reply, then the link closes after [`LINGER_MS`]; setup mode
    /// stays for another try.
    fn finish<P: Platform>(
        &mut self,
        platform: &mut P,
        reply: Reply,
        record: &mut impl FnMut(Record<P::Record>),
    ) {
        // The owner at the feeder hears that the code did not take.
        if matches!(
            reply,
            Reply::Refused(Refusal::WrongCode | Refusal::Unconfirmed)
        ) {
            self.outcome = Some(Tune::Alert);
        }
        self.reply(platform, reply, record);
        self.close_at = Some(self.now.wrapping_add(LINGER_MS));
        self.end();
    }

    fn reply<P: Platform>(
        &mut self,
        platform: &mut P,
        reply: Reply,
        record: &mut impl FnMut(Record<P::Record>),
    ) {
        record(Record::Replied { reply });
        let sent = self
            .responder
            .reply(&reply, &mut self.out)
            .ok()
            .is_some_and(|len| platform.ble_send(&self.out[..len]));
        if !sent {
            record(Record::Fault {
                fault: Fault::Reply,
            });
        }
    }

    /// End the session and drop its link.
    fn fail<P: Platform>(
        &mut self,
        platform: &mut P,
        fault: Fault,
        record: &mut impl FnMut(Record<P::Record>),
    ) {
        self.end();
        self.drop_link(platform, fault, record);
    }

    /// Drop the link but keep the session (a pressed offer completes).
    fn drop_link<P: Platform>(
        &mut self,
        platform: &mut P,
        fault: Fault,
        record: &mut impl FnMut(Record<P::Record>),
    ) {
        record(Record::Fault { fault });
        if let Some(r) = platform.ble_close(BleDrop::Protocol) {
            record(Record::Driver(r));
        }
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
fn generation(platform: &mut impl Platform) -> Option<u32> {
    match platform.wifi() {
        WifiResult::Current { view, .. } => Some(view.generation),
        _ => None,
    }
}
