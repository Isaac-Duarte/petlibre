//! The feeder's network-facing state machines on the host, driven with
//! whatever bytes a test or a fuzzer supplies: the device API's [`Server`]
//! behind a fake device, and BLE setup's [`Responder`].
//!
//! The property tests (`tests/properties.rs`) and the cargo-fuzz targets
//! (`src/fuzz/`, which includes this file by path) share these drivers, so both
//! check the same invariants: nothing panics, every frame the feeder writes
//! fits its limit, and every message it seals decodes as the protocol says.
//! This file therefore uses only this crate's own dependencies.

#![allow(dead_code, reason = "each consumer drives a subset")]

use wellfed_core::clock::wall::WallClock;
use wellfed_core::log_tail::{Batch as LogBatch, LogTail};
use wellfed_core::tone::Tune;
use wellfed_device_api::clients::{CLIENTS, Client, Label, Role};
use wellfed_device_api::mailbox::{
    Calibration, CalibrationResult, CalibrationView, LockResult, Origin, PlayResult,
    SettingsFields, SettingsResult, SettingsView, TimeFields, TimeResult, TimeSettings,
    TimeSettingsResult, TimeSettingsView, TimeState, WifiResult, WifiSource, WifiView,
};
use wellfed_device_api::pairing::{Config, Pairing};
use wellfed_device_api::setup::{Received, Responder};
use wellfed_device_api::wifi::Credentials;
use wellfed_device_api::{
    Device, EventRead, KeyChange, KeyOutcome, Pass, Record, SESSIONS, Server,
};
use wellfed_proto::api::{
    self as proto, Capabilities, EventBody, FRAME_MAX, FeederMessage, FoodLevel, Hello,
    MESSAGE_MAX, Open, PROLOGUE, PROTOCOL_MAJOR, Refusal, Stats, StatsSection, Status, Welcome,
};
use wellfed_proto::setup::{self, Answer, Reply};
use wellfed_session::frame;
use wellfed_session::noise::{Handshake, Keypair, Pattern, Role as NoiseRole, Transport};
use wellfed_session::setup_code::{Code, Exchange, Round, Side};

pub const FEEDER: [u8; 32] = [0x10; 32];
pub const OWNER: [u8; 32] = [0x20; 32];
/// Passes a driver runs after its input is spent, for answers to leave.
const SETTLE: usize = 8;

/// The feeder's mailboxes, answering at once: every write is accepted and
/// lands by the next pass, so a session reaches every answer it can.
#[derive(Default)]
struct Fake {
    counter: u8,
    settings: SettingsView,
    wifi_generation: u32,
    ticket: u32,
    feeds: u32,
    events: Vec<proto::Event>,
    time: TimeSettingsView,
    calibration: CalibrationView,
    clock: WallClock,
    /// A line every pass, so a tailing session always has more to read.
    log: LogTail<1024>,
}

/// Every section at its widest, so a frame that holds these holds any.
fn widest(section: StatsSection) -> Stats<'static> {
    let counts = proto::Counts {
        uptime_s: u32::MAX,
        feeds: u32::MAX,
        feeds_failed: u32::MAX,
        portions: u32::MAX,
        presses: u32::MAX,
        sessions: u32::MAX,
        reconnects: u32::MAX,
        dhcp_renewals: u32::MAX,
        outages: u32::MAX,
        battery_s: u32::MAX,
    };
    let erases = proto::Erases {
        storage: [u32::MAX; proto::FLASH_SECTORS],
        update: u32::MAX,
    };
    match section {
        StatsSection::Counters => Stats::Counters(proto::Counters {
            boot: counts,
            reset: proto::ResetCause::Brownout,
            lifetime: Some(counts),
            resets: Some(proto::Resets {
                power_on: u32::MAX,
                watchdog: u32::MAX,
                software: u32::MAX,
                brownout: u32::MAX,
                other: u32::MAX,
            }),
            handshake_max_ms: u32::MAX,
            handshake_mean_ms: u32::MAX,
        }),
        StatsSection::Memory => Stats::Memory(proto::Memory {
            heap_free: u32::MAX,
            heap_min: u32::MAX,
            tasks: [Some(proto::TaskStack {
                name: "0123456789abcdef",
                free_min: u32::MAX,
            }); proto::TASKS_MAX],
            tasks_total: u32::MAX,
        }),
        StatsSection::Flash => Stats::Flash(proto::Flash {
            boot: erases,
            lifetime: Some(erases),
            events_kept: u32::MAX,
            events_capacity: u32::MAX,
        }),
        StatsSection::Power => Stats::Power(proto::PowerStats {
            adapter: Some(false),
            adapter_changes: u32::MAX,
            outages: u32::MAX,
            battery_s: u32::MAX,
            battery_min_mv: Some(u32::MAX),
            battery_max_mv: Some(u32::MAX),
            battery_mv: [Some(u16::MAX); proto::POWER_HISTORY],
            period_s: u32::MAX,
        }),
        StatsSection::Food => Stats::Food(proto::Food {
            present: Some(true),
            emptied: u32::MAX,
            refilled: u32::MAX,
            feeds_since_refill: Some(u32::MAX),
        }),
        StatsSection::Feeds => Stats::Feeds(proto::FeedStats {
            traced: u32::MAX,
            drive_ms: u32::MAX,
            samples: u32::MAX,
            unit: proto::CurrentUnit::MilliAmps,
            current_peak: u32::MAX,
            current_mean: Some(u32::MAX),
            rising: u32::MAX,
            falling: u32::MAX,
            beam_breaks: u32::MAX,
            reversals: u32::MAX,
            kept: u32::MAX,
            capacity: u32::MAX,
            unsaved: u32::MAX,
            last: Some(widest_summary()),
        }),
        StatsSection::Portions => Stats::Portions(proto::Portions {
            portions: u32::MAX,
            homed: u32::MAX,
            outlet_breaks: u32::MAX,
            portion_mg: Some(u32::MAX),
            delivered_mg: Some(u64::MAX),
        }),
    }
}

/// A trace's summary at its widest.
fn widest_summary() -> proto::TraceSummary {
    proto::TraceSummary {
        feed: u32::MAX,
        requested: u8::MAX,
        completed: u8::MAX,
        outcome: proto::FeedOutcome::Uncertain,
        reason: proto::FeedReason::Output,
        drive_ms: u32::MAX,
        samples: u32::MAX,
        unit: proto::CurrentUnit::MilliAmps,
        current_peak: u32::MAX,
        current_mean: Some(u32::MAX),
        rising: u16::MAX,
        falling: u16::MAX,
        first_edge_ms: Some(u32::MAX),
        beam_breaks: u16::MAX,
        reversals: u16::MAX,
        battery_rest_mv: Some(u16::MAX),
        battery_min_mv: Some(u16::MAX),
        stride: u16::MAX,
        truncated: true,
    }
}

/// The widest trace page: a whole page of body, at the largest offset.
static TRACE_PAGE: [u8; wellfed_proto::trace::PAGE_MAX] = [0xff; wellfed_proto::trace::PAGE_MAX];

impl Device for Fake {
    fn random(&mut self, out: &mut [u8; 32]) -> bool {
        self.counter = self.counter.wrapping_add(1);
        *out = [self.counter; 32];
        true
    }

    fn build(&self) -> &'static str {
        "fuzz-build"
    }

    fn status(&mut self, _now_ms: u32) -> Status<'static> {
        Status {
            build: "fuzz-build",
            bank: 1,
            uptime_ms: 1,
            boot_count: 1,
            reset_flags: 0,
            faults: 0,
            rssi: -50,
            wifi: proto::WifiSource::Stored,
            link_losses: 0,
            dns_losses: 0,
            clock_ms: None,
            power: None,
            locked: Some(false),
            settings_generation: self.settings.generation,
            wifi_generation: self.wifi_generation,
            last_event: self.events.last().map(|e| e.seq),
            sessions: 0,
            // The widest count, so a frame that holds it holds any.
            desiccant: proto::Desiccant::Remaining { days: i32::MIN },
            food: FoodLevel::Unread,
        }
    }

    fn today(&mut self, _now_ms: u32) -> Option<i32> {
        Some(20_389)
    }

    fn settings(&mut self, change: SettingsFields) -> SettingsResult {
        if change.changes() {
            self.settings.generation += 1;
        }
        SettingsResult::Current {
            view: self.settings,
            pending: false,
        }
    }

    fn wifi(&mut self, change: Option<Option<Credentials>>) -> WifiResult {
        if change.is_some() {
            self.wifi_generation += 1;
        }
        WifiResult::Current {
            view: WifiView {
                stored_ssid_crc: None,
                generation: self.wifi_generation,
                writable: true,
                source: WifiSource::Stored,
            },
            pending: false,
        }
    }

    fn lock(&mut self, locked: bool) -> LockResult {
        LockResult::Current {
            locked,
            pending: false,
        }
    }

    fn play(&mut self, _tune: Tune) -> Option<PlayResult> {
        Some(PlayResult::Started)
    }

    fn time_sync(&mut self, fields: TimeFields, origin: Origin) -> TimeResult {
        TimeResult::from_sync(self.clock.sync(1, fields.sync(), origin))
    }

    fn time_settings(&mut self, change: Option<TimeSettings>) -> TimeSettingsResult {
        if let Some(settings) = change {
            self.time.settings = settings;
            self.time.generation += 1;
        }
        self.time.writable = true;
        TimeSettingsResult::Current {
            view: self.time,
            pending: false,
        }
    }

    fn calibration(&mut self, change: Option<Calibration>) -> CalibrationResult {
        if let Some(calibration) = change {
            self.calibration.calibration = calibration;
            self.calibration.generation += 1;
        }
        self.calibration.writable = true;
        CalibrationResult::Current {
            view: self.calibration,
            pending: false,
        }
    }

    fn phase(&self) -> Option<proto::Phase> {
        Some(proto::Phase::Unknown)
    }

    fn time_state(&mut self, now_ms: u32) -> Option<TimeState> {
        Some(TimeState {
            clock: self.clock.state(now_ms),
            now_ms,
            ntp_last: None,
        })
    }

    fn keys(&mut self, _change: KeyChange) -> Option<u32> {
        self.ticket += 1;
        Some(self.ticket)
    }

    fn keys_outcome(&mut self, _ticket: u32) -> Option<KeyOutcome> {
        Some(KeyOutcome::Done)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::FEED
            .union(Capabilities::TIME)
            .union(Capabilities::STATS)
            .union(Capabilities::LOG)
            .union(Capabilities::TRACES)
            .union(Capabilities::CALIBRATION)
    }

    fn stats(&mut self, section: StatsSection, _now_ms: u32) -> Stats<'_> {
        widest(section)
    }

    fn feed_trace(&mut self, _feed: u32, offset: u16) -> Result<proto::TracePage<'_>, Refusal> {
        Ok(proto::TracePage {
            summary: widest_summary(),
            stored: true,
            offset,
            total: u16::MAX,
            bytes: &TRACE_PAGE,
        })
    }

    fn feed(&mut self, _portions: u8, _by: proto::Source) -> Result<u32, Refusal> {
        self.feeds += 1;
        Ok(self.feeds)
    }

    fn feed_reserved(&mut self, _feed: u32) -> Option<Result<(), Refusal>> {
        Some(Ok(()))
    }

    fn cancel_feed(&mut self, feed: u32) -> bool {
        feed != 0 && feed <= self.feeds
    }

    fn record(&mut self, _now_ms: u32, body: EventBody) {
        let seq = self.events.last().map_or(1, |e| e.seq + 1);
        self.events.push(proto::Event {
            seq,
            at: None,
            body,
        });
    }

    fn event_after(&mut self, after: Option<u64>) -> EventRead {
        let after = after.unwrap_or(0);
        self.events
            .iter()
            .find(|e| e.seq > after)
            .map_or(EventRead::None, |e| EventRead::Event(*e))
    }

    fn epoch(&self) -> Option<u32> {
        Some(1)
    }

    fn reboot(&mut self) {}

    fn log_after(&mut self, after: Option<u32>, text: &mut [u8]) -> Option<LogBatch> {
        self.log.read(after, text)
    }
}

/// The server with one enrolled owner and the recovery window open.
pub struct Rig {
    server: Box<Server>,
    device: Fake,
    pairing: Pairing,
    identity: Keypair,
    now: u32,
    records: Vec<Record>,
}

impl Default for Rig {
    fn default() -> Self {
        Self::new()
    }
}

impl Rig {
    pub fn new() -> Self {
        let mut rig = Self {
            server: Box::new(Server::new()),
            device: Fake::default(),
            pairing: Pairing::new(Config::DEFAULT),
            identity: Keypair::from_secret(FEEDER),
            now: 1_000,
            records: Vec::new(),
        };
        rig.pairing.recovery(rig.now);

        let mut table = [None; CLIENTS];
        table[0] = Some(Client {
            key: *Keypair::from_secret(OWNER).public(),
            label: Label::new(b"owner").expect("a valid label"),
            role: Role::Owner,
            enrolled: 1,
        });
        *rig.server.clients_table().0 = table;
        let records = &mut rig.records;
        rig.server.clients_changed(1, |r| records.push(r));
        rig
    }

    pub fn accept(&mut self) -> Option<usize> {
        let records = &mut self.records;
        self.server.accept(self.now, |r| records.push(r))
    }

    pub fn pass(&mut self) {
        self.now += 20;
        let line = format!(
            "PLTST1 pass now={} {}\r\n",
            self.now,
            "x".repeat(self.now as usize % 120)
        );
        self.device.log.push(line.as_bytes());
        let records = &mut self.records;
        self.server.step(
            Pass {
                now_ms: self.now,
                device: &mut self.device,
                pairing: &mut self.pairing,
                identity: Some(&self.identity),
                feeding: false,
                recovery: None,
                signer: None,
            },
            |r| records.push(r),
        );
    }

    /// As much of `bytes` as the slot has room for; how much that was.
    pub fn deliver(&mut self, slot: usize, bytes: &[u8]) -> usize {
        let room = self.server.rx(slot);
        let n = room.len().min(bytes.len());
        room[..n].copy_from_slice(&bytes[..n]);
        self.server.received(slot, n);
        n
    }

    /// The frames `slot` sends, each checked against the frame limit.
    pub fn collect(&mut self, slot: usize) -> Vec<Vec<u8>> {
        let out = self.server.tx(slot).to_vec();
        self.server.sent(slot, out.len());

        let mut frames = Vec::new();
        let mut rest = out.as_slice();
        while !rest.is_empty() {
            let prefix = [rest[0], *rest.get(1).expect("a whole length prefix")];
            let len = frame::parse_prefix(prefix, FRAME_MAX).expect("a frame within the limit");
            let body = rest
                .get(frame::PREFIX..frame::PREFIX + len)
                .expect("the whole frame the prefix announces");
            frames.push(body.to_vec());
            rest = &rest[frame::PREFIX + len..];
        }
        frames
    }

    /// Whether the slot ended; it is freed once its last bytes are out.
    pub fn ended(&mut self, slot: usize) -> bool {
        if !self.server.closing(slot) {
            return false;
        }
        let records = &mut self.records;
        self.server.closed(slot, |r| records.push(r));
        true
    }

    /// Open the owner's IK session on a new slot: the slot, and the
    /// client's transport.
    pub fn open(&mut self) -> (usize, Transport) {
        let slot = self.accept().expect("a free slot");
        let mut handshake = Handshake::new(
            Pattern::Ik,
            NoiseRole::Initiator,
            PROLOGUE,
            &Keypair::from_secret(OWNER),
            Some(*self.identity.public()),
            [0x77; 32],
        )
        .expect("IK with the feeder's key");
        let hello = Hello {
            major: PROTOCOL_MAJOR,
            capabilities: Capabilities::KNOWN,
        };
        let mut payload = [0; 16];
        let len = wellfed_proto::encode(&hello, &mut payload).expect("a hello fits");
        let mut message = [0; FRAME_MAX];
        let written = handshake
            .write_message(&payload[..len], &mut message)
            .expect("the first message");

        let mut open = [0; 8];
        let len = wellfed_proto::encode(&Open::Session, &mut open).expect("an open fits");
        let mut bytes = framed(&open[..len]);
        bytes.extend(framed(&message[..written]));
        let delivered = self.deliver(slot, &bytes);
        assert_eq!(delivered, bytes.len());

        for _ in 0..SETTLE {
            self.pass();
            if let Some(reply) = self.collect(slot).first() {
                let mut plain = [0; FRAME_MAX];
                let read = handshake
                    .read_message(reply, &mut plain)
                    .expect("the feeder's answer");
                let welcome: Welcome<'_> =
                    wellfed_proto::decode(&plain[..read]).expect("a welcome");
                assert!(matches!(welcome, Welcome::Open { .. }), "{welcome:?}");
                return (slot, handshake.into_transport().expect("a finished IK"));
            }
        }
        panic!("the feeder never answered the handshake");
    }
}

pub fn framed(bytes: &[u8]) -> Vec<u8> {
    let mut out = frame::prefix(bytes.len(), usize::from(u16::MAX))
        .expect("a frame under 64 KiB")
        .to_vec();
    out.extend_from_slice(bytes);
    out
}

/// Split fuzzer input into messages: each starts with its length byte.
pub fn split(mut data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    while let Some((&len, rest)) = data.split_first() {
        let len = usize::from(len).min(rest.len());
        out.push(&rest[..len]);
        data = &rest[len..];
    }
    out
}

/// A peer that has not authenticated: any bytes on a new connection, as
/// they arrive, until the feeder closes it.
pub fn connection(bytes: &[u8]) {
    let mut rig = Rig::new();
    let slot = rig.accept().expect("a free slot");
    let mut rest = bytes;
    let mut idle = 0;

    while idle < SETTLE {
        let n = rig.deliver(slot, rest);
        rest = &rest[n..];
        rig.pass();
        let _ = rig.collect(slot);
        if rig.ended(slot) {
            return;
        }
        if rest.is_empty() {
            idle += 1;
        }
    }
}

/// The owner's session, then one sealed frame per message: every answer
/// opens under the session and decodes as a feeder message. The number of
/// answers.
pub fn session(messages: &[&[u8]]) -> usize {
    let mut rig = Rig::new();
    let (slot, mut transport) = rig.open();
    let mut sealed = [0; FRAME_MAX];
    let mut plain = [0; FRAME_MAX];
    let mut answers = 0;

    for message in messages {
        let message = &message[..message.len().min(MESSAGE_MAX)];
        let len = transport
            .seal(message, &mut sealed)
            .expect("a message within the limit");
        let bytes = framed(&sealed[..len]);
        let mut rest = bytes.as_slice();

        for _ in 0..SETTLE {
            let n = rig.deliver(slot, rest);
            rest = &rest[n..];
            rig.pass();
            for reply in rig.collect(slot) {
                let read = transport
                    .open(&reply, &mut plain)
                    .expect("the feeder seals under the session");
                let decoded = wellfed_proto::decode::<FeederMessage<'_>>(&plain[..read]);
                assert!(decoded.is_ok(), "{:?}", &plain[..read]);
                answers += 1;
            }
            if rig.ended(slot) {
                return answers;
            }
        }
    }
    answers
}

/// Every slot taken by a session that sends `bytes`, then one more
/// connection: the table stays bounded.
pub fn crowd(bytes: &[u8]) {
    let mut rig = Rig::new();
    let slots: Vec<usize> = (0..SESSIONS).filter_map(|_| rig.accept()).collect();
    assert_eq!(slots.len(), SESSIONS);
    assert_eq!(rig.accept(), None);
    for _ in 0..SETTLE {
        for &slot in &slots {
            rig.deliver(slot, bytes);
        }
        rig.pass();
        for &slot in &slots {
            let _ = rig.collect(slot);
            rig.ended(slot);
        }
    }
}

/// BLE setup from a peer that has not authenticated: any messages, in
/// order. Once one fails, every later one is refused.
pub fn setup_messages(messages: &[&[u8]]) {
    let identity = Keypair::from_secret(FEEDER);
    let mut responder = Responder::new(&identity, [0x55; 32]).expect("a responder");
    let mut out = [0; setup::REPLY_MAX + 64];
    let mut failed = false;

    for message in messages {
        let result = responder.receive(message, &mut out);
        if failed {
            assert!(result.is_err(), "{result:?} after a failure");
        }
        failed |= result.is_err();
    }
}

/// How a genuine client's BLE setup ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Setup {
    /// The feeder refused the request (its label) or a step of the proof.
    Refused,
    /// The proof completed and the feeder took this Wi-Fi.
    Proven(Option<Credentials>),
}

/// A genuine client: an XX handshake whose third message carries `request`
/// (any bytes), the owner's `code` keyed in, then the code proof with the
/// client's own `code`, its last step carrying `wifi`. The client's buffers
/// grow with its inputs, so any request or Wi-Fi is sent and the feeder,
/// not this driver, decides what it takes.
pub fn setup_client(
    request: &[u8],
    feeder_code: u16,
    client_code: u16,
    wifi: Option<setup::Wifi<'_>>,
) -> Setup {
    let identity = Keypair::from_secret(FEEDER);
    let mut responder = Responder::new(&identity, [0x55; 32]).expect("a responder");
    let mut client = Handshake::new(
        Pattern::Xx,
        NoiseRole::Initiator,
        setup::PROLOGUE,
        &Keypair::from_secret(OWNER),
        None,
        [0x66; 32],
    )
    .expect("XX needs no remote key");
    // The handshake's static key, the answer's fixed fields and the
    // length prefixes fit in the slack.
    let room = 256 + request.len() + wifi.map_or(0, |w| w.ssid.len() + w.psk.len());
    let (mut message, mut plain) = (vec![0; room], vec![0; room]);
    let mut reply = [0; 512];

    let len = client
        .write_message(&[], &mut message)
        .expect("message one");
    let Ok(Received::Send(n)) = responder.receive(&message[..len], &mut reply) else {
        panic!("the feeder answers the first message");
    };
    client
        .read_message(&reply[..n], &mut plain)
        .expect("message two");
    let Ok(len) = client.write_message(request, &mut message) else {
        return Setup::Refused;
    };
    let offer = match responder.receive(&message[..len], &mut reply) {
        Ok(Received::Offer(offer)) => offer,
        Ok(other) => panic!("{other:?} for the third message"),
        Err(_) => return Setup::Refused,
    };
    assert_eq!(offer.key, *Keypair::from_secret(OWNER).public());

    let hash = *client.handshake_hash();
    let mut transport = client.into_transport().expect("a finished XX");
    let mut exchange = Exchange::new(Side::Client, hash, Code::from_bits(client_code), [0x99; 32]);
    let mut n = responder
        .prove(Code::from_bits(feeder_code), [0x88; 32], &mut reply)
        .expect("the first step of the proof");

    loop {
        let read = transport
            .open(&reply[..n], &mut plain)
            .expect("the feeder seals under the session");
        let Ok(Reply::Round { open, commit }) = wellfed_proto::decode(&plain[..read]) else {
            panic!("a round of the proof");
        };
        if exchange.read(Round { open, commit }).is_err() {
            return Setup::Refused;
        }
        let round = exchange.write().expect("our turn");
        let answer = if exchange.peer_proven() {
            Answer::Last {
                open: round.open.expect("the last step opens"),
                wifi,
            }
        } else {
            Answer::Round {
                open: round.open,
                commit: round.commit,
            }
        };
        let len = wellfed_proto::encode(&answer, &mut plain).expect("an answer fits");
        let len = transport
            .seal(&plain[..len], &mut message)
            .expect("a sealed answer");
        match responder.receive(&message[..len], &mut reply) {
            Ok(Received::Send(next)) => n = next,
            Ok(Received::Proven { wifi }) => return Setup::Proven(wifi),
            Ok(other) => panic!("{other:?} during the proof"),
            Err(_) => return Setup::Refused,
        }
    }
}
