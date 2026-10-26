//! The device API end to end on the host: `wellfed-client`'s sessions
//! and connections against the feeder's `Server` (`wellfed-device-api`),
//! with a fake device standing in for the mailboxes and the settings
//! writer. Each test moves bytes between the two by hand, so it controls
//! every pass and, for a connection, the client's clock.

use std::time::{Duration, Instant};

use wellfed_client::api::{Error, Incoming, Invite, Join, JoinStep, Session};
use wellfed_client::connection::{
    Connection, Cursor, Event, KEEPALIVE_ID, Recovery, Resume, Timing,
};
use wellfed_core::buttons::{Button, Buttons, Gesture};
use wellfed_core::clock::wall::WallClock;
use wellfed_core::log_tail::{Batch as LogBatch, LogTail};
use wellfed_core::tone::Tune;
use wellfed_device_api::clients::{CLIENTS, Client, Label, Role};
use wellfed_device_api::mailbox::Lock;
use wellfed_device_api::mailbox::{
    LockResult, Origin, PlayResult, SettingsFields, SettingsResult, SettingsView, TimeFields,
    TimeResult, TimeSettings, TimeSettingsResult, TimeSettingsView, TimeState, WifiResult,
    WifiSource, WifiView,
};
use wellfed_device_api::pairing::{Config, Event as PairingEvent, Pairing};
use wellfed_device_api::wifi::Credentials;
use wellfed_device_api::{
    Device, EventRead, KeyChange, KeyOutcome, Pass, Reason, Record, SESSIONS, Server, UpdateAnswer,
    UpdateKeyChange, UpdateRequest,
};
use wellfed_model as model;
use wellfed_model::{Answer, Generation, Hello, Joined, Op, OpenRefusal, SettingsPatch};
use wellfed_proto::api::{
    self as proto, Capabilities, EventBody, Refusal, Status, UpdatePhase, UpdateRefusal,
    UpdateView, WANTED_MAX,
};
use wellfed_session::frame;
use wellfed_session::noise::Keypair;
use wellfed_session::setup_code::Code;

const FEEDER: [u8; 32] = [0x10; 32];
const OWNER: [u8; 32] = [0x20; 32];
const MEMBER: [u8; 32] = [0x30; 32];
const STRANGER: [u8; 32] = [0x40; 32];
/// The fake ring's epoch.
const EPOCH: u32 = 0x0e90_c401;

fn public(secret: [u8; 32]) -> [u8; 32] {
    *Keypair::from_secret(secret).public()
}

/// The feeder's mailboxes and stores, with a writer the test runs by hand.
#[derive(Default)]
struct Fake {
    counter: u8,
    settings: SettingsView,
    settings_pending: Option<SettingsFields>,
    wifi_generation: u32,
    wifi_pending: Option<Option<Credentials>>,
    locked: Option<bool>,
    clients: Vec<Client>,
    keys_pending: Option<(u32, KeyChange)>,
    keys_done: Option<(u32, KeyOutcome)>,
    ticket: u32,
    events: Vec<proto::Event>,
    /// Events before this sequence number left the ring.
    oldest: u64,
    /// `None` until the writer has opened the ring.
    epoch: Option<u32>,
    rebooted: bool,
    syncs: usize,
    /// Feeds queued, in order: feed `n` is `feeds[n - 1]`.
    feeds: Vec<(u8, proto::Source)>,
    /// Feeds up to this number are reserved (the owner's turn, by hand).
    reserved: u32,
    cancelled: Vec<u32>,
    /// Serves `Capabilities::UPDATE`.
    update: bool,
    /// An install holds the flash.
    installing: bool,
    /// Update requests queued, in order, and the next one's answer.
    updates: Vec<String>,
    update_done: Option<(u32, Result<UpdateAnswer, Refusal>)>,
    /// Keep the next request queued (its lease held) until `answer_held`.
    hold: bool,
    held: Option<(u32, Result<UpdateAnswer, Refusal>)>,
    /// Chunk offsets handed over.
    chunks: Vec<u32>,
    time: TimeSettingsView,
    time_pending: Option<TimeSettings>,
    /// The writer's next time save fails, leaving the store as it was.
    time_fails: bool,
    /// Serves `Capabilities::LOG` from `tail`.
    log: bool,
    tail: LogTail<4096>,
}

/// An install receiving its first sector.
fn receiving() -> UpdateView {
    UpdateView {
        phase: UpdatePhase::Receiving,
        bank: 0,
        min_version: 4,
        digest: Some([9; 32]),
        version: 5,
        image_len: 8192,
        written: 0,
        wanted: [Some(0); WANTED_MAX],
        stopped: None,
    }
}

impl Fake {
    fn new() -> Self {
        let mut fake = Self {
            settings: SettingsView {
                lights: true,
                sound: true,
                volume: 40,
                generation: 7,
                writable: true,
            },
            wifi_generation: 3,
            locked: Some(false),
            oldest: 1,
            epoch: Some(EPOCH),
            ..Self::default()
        };
        fake.enroll(public(OWNER), Role::Owner);
        fake.enroll(public(MEMBER), Role::Member);
        fake
    }

    fn enroll(&mut self, key: [u8; 32], role: Role) {
        self.clients.push(Client {
            key,
            label: Label::new(b"test").unwrap(),
            role,
            enrolled: 1,
        });
    }

    /// The update stage answers the request it held.
    fn answer_held(&mut self) {
        self.update_done = self.held.take();
    }

    /// The settings writer's turn: apply what is queued.
    fn write(&mut self) {
        if let Some(fields) = self.settings_pending.take() {
            self.settings.lights = fields.lights.unwrap_or(self.settings.lights);
            self.settings.sound = fields.sound.unwrap_or(self.settings.sound);
            self.settings.volume = fields.volume.unwrap_or(self.settings.volume);
            self.settings.generation += 1;
        }
        if self.wifi_pending.take().is_some() {
            self.wifi_generation += 1;
        }
        if let Some(settings) = self.time_pending.take()
            && !std::mem::take(&mut self.time_fails)
            && settings != self.time.settings
        {
            self.time.settings = settings;
            self.time.generation += 1;
        }
        if let Some((ticket, change)) = self.keys_pending.take() {
            let outcome = match change {
                KeyChange::Enroll { key, role, .. } => {
                    if self.clients.iter().any(|c| c.key == key) {
                        KeyOutcome::Duplicate
                    } else {
                        self.enroll(key, role);
                        KeyOutcome::Done
                    }
                }
                KeyChange::Revoke { key } => {
                    let before = self.clients.len();
                    self.clients.retain(|c| c.key != key);
                    if self.clients.len() < before {
                        KeyOutcome::Done
                    } else {
                        KeyOutcome::NotFound
                    }
                }
            };
            self.keys_done = Some((ticket, outcome));
        }
        self.reserved = u32::try_from(self.feeds.len()).unwrap();
    }

    fn table(&self) -> [Option<Client>; CLIENTS] {
        let mut table = [None; CLIENTS];
        for (slot, client) in table.iter_mut().zip(&self.clients) {
            *slot = Some(*client);
        }
        table
    }
}

impl Device for Fake {
    fn random(&mut self, out: &mut [u8; 32]) -> bool {
        self.counter = self.counter.wrapping_add(1);
        *out = [self.counter; 32];
        true
    }

    fn build(&self) -> &'static str {
        "test-build"
    }

    fn status(&mut self, _now_ms: u32) -> Status<'static> {
        Status {
            build: "test-build",
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
            locked: self.locked,
            settings_generation: self.settings.generation,
            wifi_generation: self.wifi_generation,
            last_event: self.events.last().map(|e| e.seq),
            sessions: 0,
        }
    }

    fn settings(&mut self, change: SettingsFields) -> SettingsResult {
        if !change.changes() {
            return SettingsResult::Current {
                view: self.settings,
                pending: self.settings_pending.is_some(),
            };
        }
        if self.settings_pending.is_some() {
            return SettingsResult::Busy;
        }
        self.settings_pending = Some(change);
        SettingsResult::Current {
            view: self.settings,
            pending: true,
        }
    }

    fn wifi(&mut self, change: Option<Option<Credentials>>) -> WifiResult {
        if let Some(change) = change {
            if self.wifi_pending.is_some() {
                return WifiResult::Busy;
            }
            self.wifi_pending = Some(change);
        }
        WifiResult::Current {
            view: WifiView {
                stored_ssid_crc: None,
                generation: self.wifi_generation,
                writable: true,
                source: WifiSource::Stored,
            },
            pending: self.wifi_pending.is_some(),
        }
    }

    fn lock(&mut self, locked: bool) -> LockResult {
        self.locked = Some(locked);
        LockResult::Current {
            locked,
            pending: true,
        }
    }

    fn play(&mut self, _tune: Tune) -> Option<PlayResult> {
        Some(PlayResult::Started)
    }

    fn time_sync(&mut self, _fields: TimeFields, _origin: Origin) -> TimeResult {
        self.syncs += 1;
        TimeResult::Synced { jump_ms: Some(5) }
    }

    fn keys(&mut self, change: KeyChange) -> Option<u32> {
        if self.keys_pending.is_some() {
            return None;
        }
        self.ticket += 1;
        self.keys_pending = Some((self.ticket, change));
        Some(self.ticket)
    }

    fn keys_outcome(&mut self, ticket: u32) -> Option<KeyOutcome> {
        self.keys_done
            .filter(|&(done, _)| done == ticket)
            .map(|(_, outcome)| outcome)
    }

    fn capabilities(&self) -> Capabilities {
        let mut capabilities = Capabilities::FEED.union(Capabilities::TIME);
        if self.update {
            capabilities = capabilities.union(Capabilities::UPDATE);
        }
        if self.log {
            capabilities = capabilities.union(Capabilities::LOG);
        }
        capabilities
    }

    fn log_after(&mut self, after: Option<u32>, text: &mut [u8]) -> Option<LogBatch> {
        self.tail.read(after, text)
    }

    fn time_settings(&mut self, change: Option<TimeSettings>) -> TimeSettingsResult {
        if let Some(settings) = change {
            if self.time_pending.is_some() {
                return TimeSettingsResult::Busy;
            }
            self.time_pending = Some(settings);
        }
        TimeSettingsResult::Current {
            view: TimeSettingsView {
                writable: true,
                ..self.time
            },
            pending: self.time_pending.is_some(),
        }
    }

    fn time_state(&mut self, now_ms: u32) -> Option<TimeState> {
        Some(TimeState {
            clock: WallClock::new().state(now_ms),
            now_ms,
            ntp_last: None,
        })
    }

    fn update(&mut self, request: UpdateRequest<'_>) -> Result<u32, Refusal> {
        self.ticket += 1;
        let answer = match request {
            UpdateRequest::Offer { device, .. } => {
                assert_eq!(device, public(FEEDER), "an offer names this feeder");
                self.installing = true;
                Ok(UpdateAnswer::View(receiving()))
            }
            UpdateRequest::Statement { statement } => Ok(UpdateAnswer::Key {
                change: UpdateKeyChange::Retired,
                id: statement[..8].try_into().unwrap(),
            }),
            UpdateRequest::Cancel => {
                self.installing = false;
                Ok(UpdateAnswer::View(UpdateView {
                    phase: UpdatePhase::Idle,
                    ..receiving()
                }))
            }
        };
        self.updates.push(format!("{request:?}"));
        if self.hold {
            self.held = Some((self.ticket, answer));
        } else {
            self.update_done = Some((self.ticket, answer));
        }
        Ok(self.ticket)
    }

    fn update_outcome(&mut self, ticket: u32) -> Option<Result<UpdateAnswer, Refusal>> {
        self.update_done
            .filter(|&(done, _)| done == ticket)
            .map(|(_, answer)| answer)
    }

    fn update_chunk(&mut self, offset: u32, _data: &[u8]) -> Result<UpdateView, Refusal> {
        if !self.installing {
            return Err(Refusal::Update(UpdateRefusal::NotInstalling));
        }
        self.chunks.push(offset);
        Ok(receiving())
    }

    fn update_view(&mut self) -> Option<UpdateView> {
        self.update.then(receiving)
    }

    fn installing(&self) -> bool {
        // A queued request holds the lease too.
        self.installing || self.held.is_some()
    }

    fn feed_active(&self) -> bool {
        self.reserved < u32::try_from(self.feeds.len()).unwrap()
    }

    fn feed(&mut self, portions: u8, by: proto::Source) -> Result<u32, Refusal> {
        if self.reserved < u32::try_from(self.feeds.len()).unwrap() {
            return Err(Refusal::Busy);
        }
        self.feeds.push((portions, by));
        Ok(u32::try_from(self.feeds.len()).unwrap())
    }

    fn feed_reserved(&mut self, feed: u32) -> Option<Result<(), Refusal>> {
        (feed <= self.reserved).then_some(Ok(()))
    }

    fn cancel_feed(&mut self, feed: u32) -> bool {
        let running = feed != 0 && feed <= self.reserved && !self.cancelled.contains(&feed);
        if running {
            self.cancelled.push(feed);
        }
        running
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
        if after + 1 < self.oldest {
            return EventRead::Gap {
                from: after + 1,
                to: self.oldest - 1,
            };
        }
        self.events
            .iter()
            .find(|e| e.seq > after && e.seq >= self.oldest)
            .map_or(EventRead::None, |e| EventRead::Event(*e))
    }

    fn epoch(&self) -> Option<u32> {
        self.epoch
    }

    fn reboot(&mut self) {
        self.rebooted = true;
    }
}

/// The server, its device and pairing, stepped one pass at a time.
struct Rig {
    server: Box<Server>,
    device: Fake,
    pairing: Pairing,
    identity: Keypair,
    now: u32,
    feeding: bool,
    /// Pairing's decision on a recovery offer, for the next pass.
    recovery: Option<wellfed_device_api::pairing::Event>,
    /// Pairing's decision on an update-key offer, for the next pass.
    signer: Option<wellfed_device_api::pairing::Event>,
    records: Vec<Record>,
    clients_generation: u32,
}

impl Rig {
    fn new() -> Self {
        let mut rig = Self {
            server: Box::new(Server::new()),
            device: Fake::new(),
            pairing: Pairing::new(Config::DEFAULT),
            identity: Keypair::from_secret(FEEDER),
            now: 1_000,
            feeding: false,
            recovery: None,
            signer: None,
            records: Vec::new(),
            clients_generation: 0,
        };
        rig.publish();
        rig
    }

    /// The writer publishes the client table, as `keys::clients_since`.
    fn publish(&mut self) {
        self.clients_generation += 1;
        let table = self.device.table();
        *self.server.clients_table().0 = table;
        let records = &mut self.records;
        self.server
            .clients_changed(self.clients_generation, |r| records.push(r));
    }

    /// One pass; whether the server has work waiting.
    fn pass(&mut self) -> bool {
        self.now += 20;
        let records = &mut self.records;
        self.server.step(
            Pass {
                now_ms: self.now,
                device: &mut self.device,
                pairing: &mut self.pairing,
                identity: Some(&self.identity),
                feeding: self.feeding,
                recovery: self.recovery.take(),
                signer: self.signer.take(),
            },
            |r| records.push(r),
        )
    }

    fn accept(&mut self) -> Option<usize> {
        let records = &mut self.records;
        self.server.accept(self.now, |r| records.push(r))
    }

    /// Client bytes into `slot`, as far as its buffer has room.
    fn deliver(&mut self, slot: usize, bytes: &mut Vec<u8>) {
        let room = self.server.rx(slot);
        let n = room.len().min(bytes.len());
        room[..n].copy_from_slice(&bytes[..n]);
        self.server.received(slot, n);
        bytes.drain(..n);
    }

    /// Everything `slot` has to send.
    fn collect(&mut self, slot: usize) -> Vec<u8> {
        let out = self.server.tx(slot).to_vec();
        self.server.sent(slot, out.len());
        out
    }

    fn ended(&self, reason: Reason) -> bool {
        self.records
            .iter()
            .any(|r| matches!(r, Record::Ended { reason: got, .. } if *got == reason))
    }
}

/// One client on one slot.
struct Client_ {
    slot: usize,
    session: Session,
    outbox: Vec<u8>,
}

/// Run passes, moving bytes both ways, until `done` or a bound.
fn run(rig: &mut Rig, client: &mut Client_, mut done: impl FnMut(&mut Session) -> bool) {
    for _ in 0..200 {
        rig.deliver(client.slot, &mut client.outbox);
        rig.pass();
        let bytes = rig.collect(client.slot);
        if !bytes.is_empty() {
            client.session.receive(&bytes).unwrap();
        }
        rig.device.write();
        if done(&mut client.session) {
            return;
        }
    }
    panic!("no progress");
}

fn connect(rig: &mut Rig, secret: [u8; 32]) -> (Client_, Option<Incoming>) {
    let slot = rig.accept().unwrap();
    let (session, outbox) =
        Session::connect(&Keypair::from_secret(secret), public(FEEDER), [0x77; 32]).unwrap();
    let mut client = Client_ {
        slot,
        session,
        outbox,
    };
    let mut welcome = None;
    run(rig, &mut client, |session| {
        if let Some(incoming @ (Incoming::Opened(_) | Incoming::Refused(_))) =
            session.next_incoming().unwrap()
        {
            welcome = Some(incoming);
            return true;
        }
        false
    });
    (client, welcome)
}

fn open(rig: &mut Rig, secret: [u8; 32]) -> Client_ {
    let (client, welcome) = connect(rig, secret);
    assert!(matches!(welcome, Some(Incoming::Opened(_))), "{welcome:?}");
    client
}

/// Send one request and run until its answer.
fn ask(rig: &mut Rig, client: &mut Client_, id: u8, op: Op) -> Answer {
    let bytes = client.session.request([id; 16], &op).unwrap();
    client.outbox.extend(bytes);
    let mut answer = None;
    run(rig, client, |session| {
        while let Some(incoming) = session.next_incoming().unwrap() {
            if let Incoming::Answer {
                id: got,
                answer: got_answer,
            } = incoming
                && got == [id; 16]
            {
                answer = Some(got_answer);
                return true;
            }
        }
        false
    });
    answer.unwrap()
}

#[test]
fn an_owner_opens_a_session_and_reads_status_and_settings() {
    let mut rig = Rig::new();
    let mut owner = open(&mut rig, OWNER);
    assert!(rig.records.iter().any(|r| matches!(
        r,
        Record::Opened {
            role: Role::Owner,
            ..
        }
    )));

    let status = ask(&mut rig, &mut owner, 1, Op::Status);
    let Answer::Status(status) = status else {
        panic!("{status:?}");
    };
    assert_eq!(status.settings_generation, Generation(7));
    assert_eq!(status.sessions, 1);

    let settings = ask(&mut rig, &mut owner, 2, Op::GetSettings);
    assert!(matches!(
        settings,
        Answer::Settings(model::Settings {
            generation: Generation(7),
            volume: 40,
            ..
        })
    ));
}

#[test]
fn an_unknown_key_and_the_wrong_feeder_are_refused() {
    let mut rig = Rig::new();
    let (_, welcome) = connect(&mut rig, STRANGER);
    assert_eq!(welcome, Some(Incoming::Refused(OpenRefusal::Unknown)));
    assert!(rig.ended(Reason::Unknown));

    // A client holding another feeder's key: the feeder cannot read its
    // first message and drops it.
    let mut rig = Rig::new();
    let slot = rig.accept().unwrap();
    let (_, mut outbox) =
        Session::connect(&Keypair::from_secret(OWNER), public(STRANGER), [1; 32]).unwrap();
    for _ in 0..5 {
        rig.deliver(slot, &mut outbox);
        rig.pass();
    }
    assert!(rig.ended(Reason::Noise));
    assert!(rig.server.closing(slot));
}

#[test]
fn a_stale_settings_write_is_refused_and_a_current_one_applies() {
    let mut rig = Rig::new();
    let mut owner = open(&mut rig, OWNER);
    let changes = SettingsPatch {
        volume: Some(60),
        ..SettingsPatch::default()
    };

    let done = ask(
        &mut rig,
        &mut owner,
        1,
        Op::PutSettings {
            base: Generation(7),
            changes,
        },
    );
    assert_eq!(
        done,
        Answer::Done {
            generation: Some(Generation(8))
        }
    );
    assert_eq!(rig.device.settings.volume, 60);

    let stale = ask(
        &mut rig,
        &mut owner,
        2,
        Op::PutSettings {
            base: Generation(7),
            changes,
        },
    );
    assert_eq!(
        stale,
        Answer::Refused(model::Refusal::Stale {
            current: Generation(8)
        })
    );
    assert!(
        rig.device
            .events
            .iter()
            .any(|e| matches!(e.body, EventBody::Settings { generation: 8, .. }))
    );
}

#[test]
fn a_retried_write_gets_its_first_answer_even_after_a_reconnect() {
    let mut rig = Rig::new();
    let mut owner = open(&mut rig, OWNER);
    let changes = SettingsPatch {
        lights: Some(false),
        ..SettingsPatch::default()
    };
    let first = ask(
        &mut rig,
        &mut owner,
        9,
        Op::PutSettings {
            base: Generation(7),
            changes,
        },
    );
    assert_eq!(
        first,
        Answer::Done {
            generation: Some(Generation(8))
        }
    );

    // The connection drops before the client sees the answer; it
    // reconnects and retries with the same id.
    let records = &mut rig.records;
    rig.server.closed(owner.slot, |r| records.push(r));
    let mut owner = open(&mut rig, OWNER);
    let retry = ask(
        &mut rig,
        &mut owner,
        9,
        Op::PutSettings {
            base: Generation(7),
            changes,
        },
    );
    assert_eq!(retry, first);
    assert_eq!(rig.device.settings.generation, 8, "written once");
    assert!(
        rig.records
            .iter()
            .any(|r| matches!(r, Record::Retried { .. }))
    );
}

/// A time settings write answers `Done` with the new generation once the
/// store holds it, `Done` too when it changed nothing, and is refused when
/// the writer could not make it, rather than answered with the old
/// generation as if it had.
#[test]
fn a_time_settings_write_answers_what_the_writer_did() {
    let mut rig = Rig::new();
    let mut owner = open(&mut rig, OWNER);
    let put = |base, port| Op::PutTime {
        base: Generation(base),
        changes: model::TimeChanges {
            zone: Some("MST7MDT,M3.2.0,M11.1.0".to_owned()),
            ntp: Some(model::NtpServer::Gateway { port }),
        },
    };

    let done = ask(&mut rig, &mut owner, 1, put(0, 1123));
    assert_eq!(
        done,
        Answer::Done {
            generation: Some(Generation(1))
        }
    );
    let unchanged = ask(&mut rig, &mut owner, 2, put(1, 1123));
    assert_eq!(
        unchanged,
        Answer::Done {
            generation: Some(Generation(1))
        }
    );

    rig.device.time_fails = true;
    let failed = ask(&mut rig, &mut owner, 3, put(1, 123));
    assert_eq!(failed, Answer::Refused(model::Refusal::NotWritable));
    let Answer::Time(view) = ask(&mut rig, &mut owner, 4, Op::GetTime) else {
        panic!("the time view");
    };
    assert_eq!(view.generation, Generation(1));
    assert_eq!(view.ntp, model::NtpServer::Gateway { port: 1123 });
    assert_eq!(view.clock_ms, None, "the feeder needs the time");
}

#[test]
fn a_member_may_feed_and_change_settings_but_not_manage_clients() {
    let mut rig = Rig::new();
    let mut member = open(&mut rig, MEMBER);

    let wifi = ask(
        &mut rig,
        &mut member,
        1,
        Op::PutWifi {
            base: Generation(3),
            wifi: None,
        },
    );
    assert_eq!(wifi, Answer::Refused(model::Refusal::Forbidden));
    let revoke = ask(
        &mut rig,
        &mut member,
        2,
        Op::RevokeClient { key: public(OWNER) },
    );
    assert_eq!(revoke, Answer::Refused(model::Refusal::Forbidden));
    let reboot = ask(&mut rig, &mut member, 3, Op::Reboot);
    assert_eq!(reboot, Answer::Refused(model::Refusal::Forbidden));
    // The NTP server decides which fixes the clock hears: an owner's.
    let time = ask(
        &mut rig,
        &mut member,
        7,
        Op::PutTime {
            base: Generation(0),
            changes: model::TimeChanges::default(),
        },
    );
    assert_eq!(time, Answer::Refused(model::Refusal::Forbidden));

    let lock = ask(&mut rig, &mut member, 4, Op::SetLock { locked: true });
    assert_eq!(lock, Answer::Done { generation: None });
    let feed = ask(&mut rig, &mut member, 5, Op::Feed { portions: 1 });
    assert_eq!(feed, Answer::Feeding { feed: 1 });
    let cancel = ask(&mut rig, &mut member, 6, Op::CancelFeed { feed: 1 });
    assert_eq!(cancel, Answer::Done { generation: None });
}

#[test]
fn a_feed_answers_once_reserved_and_a_retry_never_feeds_twice() {
    let mut rig = Rig::new();
    let mut owner = open(&mut rig, OWNER);

    let zero = ask(&mut rig, &mut owner, 1, Op::Feed { portions: 0 });
    assert_eq!(zero, Answer::Refused(model::Refusal::Invalid));
    let feed = ask(&mut rig, &mut owner, 2, Op::Feed { portions: 3 });
    assert_eq!(feed, Answer::Feeding { feed: 1 });
    let by = proto::Source::Client(public(OWNER)[..4].try_into().unwrap());
    assert_eq!(rig.device.feeds, [(3, by)]);

    // The same id after a reconnect: the first answer, and no second feed.
    let mut owner = open(&mut rig, OWNER);
    let again = ask(&mut rig, &mut owner, 2, Op::Feed { portions: 3 });
    assert_eq!(again, Answer::Feeding { feed: 1 });
    assert_eq!(rig.device.feeds.len(), 1);

    let cancel = ask(&mut rig, &mut owner, 3, Op::CancelFeed { feed: 1 });
    assert_eq!(cancel, Answer::Done { generation: None });
    let ended = ask(&mut rig, &mut owner, 4, Op::CancelFeed { feed: 1 });
    assert_eq!(ended, Answer::Refused(model::Refusal::NotFound));
}

#[test]
fn a_client_without_the_feed_capability_neither_feeds_nor_sees_feeds() {
    let mut rig = Rig::new();
    let fed = EventBody::Fed {
        feed: 1,
        requested: 1,
        completed: 1,
        outcome: proto::FeedOutcome::Done,
        reason: proto::FeedReason::Done,
        by: proto::Source::Panel,
    };
    rig.device.record(0, fed);
    rig.device.record(
        0,
        EventBody::Lock {
            locked: true,
            by: proto::Source::Panel,
        },
    );

    let slot = rig.accept().unwrap();
    let keys = Keypair::from_secret(OWNER);
    let hello = Hello {
        major: 1,
        capabilities: model::Capabilities::V1,
    };
    let (session, outbox) =
        Session::connect_with(&keys, public(FEEDER), [0x77; 32], hello).unwrap();
    let mut old = Client_ {
        slot,
        session,
        outbox,
    };
    run(&mut rig, &mut old, |session| {
        matches!(session.next_incoming(), Ok(Some(Incoming::Opened(_))))
    });
    let feed = ask(&mut rig, &mut old, 1, Op::Feed { portions: 1 });
    assert_eq!(feed, Answer::Refused(model::Refusal::Unavailable));
    assert!(rig.device.feeds.is_empty());

    let bytes = old.session.subscribe(None, false).unwrap();
    old.outbox.extend(bytes);
    let mut seen = Vec::new();
    run(&mut rig, &mut old, |session| {
        while let Some(incoming) = session.next_incoming().unwrap() {
            if let Incoming::Pushed(model::Pushed::Event(event)) = incoming {
                seen.push(event.seq);
            }
        }
        !seen.is_empty()
    });
    assert_eq!(seen, [2]);
}

#[test]
fn revoking_a_client_closes_its_open_session() {
    let mut rig = Rig::new();
    let mut owner = open(&mut rig, OWNER);
    let member = open(&mut rig, MEMBER);

    let listed = ask(&mut rig, &mut owner, 1, Op::ListClients);
    let Answer::Clients(clients) = listed else {
        panic!("{listed:?}");
    };
    assert_eq!(clients.len(), 2);
    assert!(
        clients
            .iter()
            .any(|c| c.you && c.role == model::ClientRole::Owner)
    );

    let revoke = ask(
        &mut rig,
        &mut owner,
        2,
        Op::RevokeClient {
            key: public(MEMBER),
        },
    );
    assert_eq!(revoke, Answer::Done { generation: None });
    rig.publish();
    assert!(rig.ended(Reason::Revoked));
    assert!(rig.server.closing(member.slot));
}

#[test]
fn an_invite_enrolls_a_new_client_once() {
    let mut rig = Rig::new();
    let mut owner = open(&mut rig, OWNER);
    let invite = ask(
        &mut rig,
        &mut owner,
        1,
        Op::CreateInvite {
            role: model::ClientRole::Member,
        },
    );
    let Answer::Invite(invite) = invite else {
        panic!("{invite:?}");
    };
    let invite = Invite {
        feeder: public(FEEDER),
        id: invite.id,
        secret: invite.secret.0,
    };

    let joined = redeem(&mut rig, invite, STRANGER);
    assert_eq!(
        joined,
        Some(Joined::Enrolled {
            role: model::ClientRole::Member
        })
    );
    rig.publish();
    let _stranger = open(&mut rig, STRANGER);

    // The invite was single-use.
    assert_eq!(redeem(&mut rig, invite, [0x50; 32]), None);
    assert!(rig.ended(Reason::Invite));
}

/// Redeem `invite` with a new key; the feeder's answer, or `None` if the
/// connection was dropped.
fn redeem(rig: &mut Rig, invite: Invite, secret: [u8; 32]) -> Option<Joined> {
    let slot = rig.accept().unwrap();
    let (mut join, mut outbox) =
        Join::connect(&Keypair::from_secret(secret), invite, "phone", [0x55; 32]).unwrap();
    for _ in 0..200 {
        rig.deliver(slot, &mut outbox);
        rig.pass();
        let bytes = rig.collect(slot);
        if !bytes.is_empty() {
            match join.receive(&bytes).unwrap() {
                Some(JoinStep::Send(bytes)) => outbox.extend(bytes),
                Some(JoinStep::Done(joined)) => return Some(joined),
                None => {}
            }
        }
        rig.device.write();
        if rig.server.closing(slot) && rig.server.tx(slot).is_empty() {
            let records = &mut rig.records;
            rig.server.closed(slot, |r| records.push(r));
            return None;
        }
    }
    panic!("no progress");
}

#[test]
fn a_subscriber_reads_every_event_after_its_cursor_and_learns_of_a_gap() {
    let mut rig = Rig::new();
    for generation in 10..15 {
        let body = EventBody::Settings {
            generation,
            by: proto::Source::Panel,
        };
        rig.device.record(0, body);
    }
    // Events 1 and 2 left the ring.
    rig.device.oldest = 3;

    let mut owner = open(&mut rig, OWNER);
    let bytes = owner.session.subscribe(Some(0), false).unwrap();
    owner.outbox.extend(bytes);
    let mut seen = Vec::new();
    run(&mut rig, &mut owner, |session| {
        while let Some(incoming) = session.next_incoming().unwrap() {
            match incoming {
                Incoming::Pushed(model::Pushed::Gap(gap)) => seen.push((gap.from, gap.to)),
                Incoming::Pushed(model::Pushed::Event(event)) => seen.push((event.seq, 0)),
                _ => {}
            }
        }
        seen.len() == 4
    });
    assert_eq!(seen, [(1, 2), (3, 0), (4, 0), (5, 0)]);

    // A new event reaches the subscriber live.
    rig.device.record(0, EventBody::Lost { count: 1 });
    let mut live = None;
    run(&mut rig, &mut owner, |session| {
        if let Some(Incoming::Pushed(model::Pushed::Event(event))) =
            session.next_incoming().unwrap()
        {
            live = Some(event.seq);
        }
        live.is_some()
    });
    assert_eq!(live, Some(6));
}

#[test]
fn one_handshake_operation_per_pass_and_none_while_feeding() {
    let mut rig = Rig::new();
    let mut clients: Vec<(usize, Vec<u8>)> = (0..SESSIONS)
        .map(|_| {
            let slot = rig.accept().unwrap();
            let (_, outbox) =
                Session::connect(&Keypair::from_secret(OWNER), public(FEEDER), [9; 32]).unwrap();
            (slot, outbox)
        })
        .collect();
    assert_eq!(rig.accept(), None, "a fifth connection is refused");
    assert!(rig.ended(Reason::Full));

    for (slot, outbox) in &mut clients {
        rig.deliver(*slot, outbox);
    }
    // Each slot needs its open frame read, then three operations: the
    // handshake built, the first message read, the answer written.
    rig.pass();
    rig.feeding = true;
    for _ in 0..10 {
        rig.pass();
    }
    assert!(
        clients
            .iter()
            .all(|(slot, _)| rig.server.tx(*slot).is_empty())
    );

    rig.feeding = false;
    let mut answered = 0;
    for pass in 1..=(3 * SESSIONS) {
        rig.pass();
        let now = clients
            .iter()
            .filter(|(slot, _)| !rig.server.tx(*slot).is_empty())
            .count();
        assert!(now <= pass / 3 + 1);
        answered = now;
    }
    assert_eq!(answered, SESSIONS);
}

#[test]
fn a_handshake_waiting_on_its_client_is_not_busy() {
    let mut rig = Rig::new();
    let slot = rig.accept().unwrap();
    let (_, mut outbox) =
        Session::connect(&Keypair::from_secret(OWNER), public(FEEDER), [9; 32]).unwrap();
    let open = frame::PREFIX + frame::parse_prefix([outbox[0], outbox[1]], usize::MAX).unwrap();
    let mut first = outbox.drain(..open).collect();

    // The open frame read and the handshake built are work; then the
    // feeder waits for the client's first message, which is not.
    rig.deliver(slot, &mut first);
    assert!(rig.pass(), "the open frame");
    assert!(rig.pass(), "the handshake built");
    assert!(!rig.pass());

    rig.deliver(slot, &mut outbox);
    assert!(rig.pass(), "the client's message is work again");
}

#[test]
fn malformed_bytes_and_floods_cost_only_their_own_session() {
    let mut rig = Rig::new();
    let mut owner = open(&mut rig, OWNER);

    // A frame announcing more than the limit.
    let bad = rig.accept().unwrap();
    rig.deliver(bad, &mut vec![0xff, 0xff, 1, 2, 3]);
    rig.pass();
    assert!(rig.ended(Reason::Malformed));

    // Garbage after a valid open frame never finishes a handshake; another
    // client floods requests. The owner still gets answers.
    let flood = open(&mut rig, MEMBER);
    let mut flood = flood;
    for id in 0..50u8 {
        let bytes = flood.session.request([id; 16], &Op::Status).unwrap();
        flood.outbox.extend(bytes);
    }
    for _ in 0..3 {
        rig.deliver(flood.slot, &mut flood.outbox);
        rig.pass();
        let _ = rig.collect(flood.slot);
    }
    let status = ask(&mut rig, &mut owner, 1, Op::Status);
    assert!(matches!(status, Answer::Status(_)));
}

#[test]
fn an_idle_session_and_a_stalled_handshake_time_out() {
    let mut rig = Rig::new();
    let owner = open(&mut rig, OWNER);
    let stalled = rig.accept().unwrap();

    rig.now += wellfed_device_api::HANDSHAKE_MS;
    rig.pass();
    assert!(rig.server.closing(stalled));
    rig.now += wellfed_device_api::IDLE_MS;
    rig.pass();
    assert!(rig.server.closing(owner.slot));
}

#[test]
fn reboot_and_time_sync_reach_the_device() {
    let mut rig = Rig::new();
    let mut owner = open(&mut rig, OWNER);
    let sync = model::TimeSync {
        unix_ms: 1_700_000_000_000,
        offset_s: 0,
        next_change_s: 0,
        next_offset_s: 0,
    };
    let synced = ask(&mut rig, &mut owner, 1, Op::TimeSync(sync));
    assert_eq!(synced, Answer::Done { generation: None });
    assert_eq!(rig.device.syncs, 1);

    let reboot = ask(&mut rig, &mut owner, 2, Op::Reboot);
    assert_eq!(reboot, Answer::Done { generation: None });
    rig.now += wellfed_device_api::REBOOT_MS;
    rig.pass();
    assert!(rig.device.rebooted);
}

/// Run a LAN recovery session to its last reply: the setup exchange over
/// the device API, with the owner keying `keyed` in on the panel when the
/// feeder asks. `None` when the feeder dropped the connection.
fn recover(
    rig: &mut Rig,
    secret: [u8; 32],
    code: Code,
    keyed: Code,
) -> Option<wellfed_model::setup::Reply> {
    use wellfed_client::setup::{Ask, Session as Setup};
    use wellfed_model::setup::Reply;

    let slot = rig.accept().unwrap();
    let ask = Ask {
        label: "laptop".into(),
        wifi: None,
    };
    let keys = Keypair::from_secret(secret);
    let setup = Setup::new(&keys, [0x61; 32], ask, code, [0x62; 32]).unwrap();
    let mut recovery = Recovery::new(setup).unwrap();
    let mut outbox = Vec::new();

    for _ in 0..400 {
        while let Some(bytes) = recovery.poll_transmit() {
            outbox.extend(bytes);
        }
        rig.deliver(slot, &mut outbox);
        rig.pass();
        let received = rig.collect(slot);
        rig.device.write();
        match recovery.handle_input(&received) {
            Ok(()) => {}
            // A mismatched code fails at whichever side opens a press
            // first; with the feeder writing first that is the client.
            Err(Error::Setup(wellfed_client::setup::Error::WrongCode)) => {
                return Some(Reply::Refused(wellfed_model::setup::Refusal::WrongCode));
            }
            Err(error) => panic!("{error}"),
        }
        while let Some(reply) = recovery.poll_reply() {
            let Reply::EnterCode { .. } = reply else {
                return Some(reply);
            };
            for &button in keyed.presses() {
                let down = Gesture::Down {
                    chord: Buttons::of(button),
                };
                let event = rig.pairing.gesture(rig.now, down, Lock::Unlocked);
                if matches!(event, Some(PairingEvent::Entered { .. })) {
                    rig.recovery = event;
                }
            }
        }
        if rig.server.closing(slot) && rig.server.tx(slot).is_empty() {
            let records = &mut rig.records;
            rig.server.closed(slot, |r| records.push(r));
            return None;
        }
    }
    panic!("no progress");
}

fn open_recovery(rig: &mut Rig) {
    let hold = Gesture::Hold {
        chord: Buttons::of(Button::Lock).with(Button::Dispense),
        level: 1,
    };
    assert_eq!(
        rig.pairing.gesture(rig.now, hold, Lock::Unlocked),
        Some(PairingEvent::RecoveryOpened)
    );
}

#[test]
fn the_recovery_window_admits_one_owner_who_proves_the_code() {
    use wellfed_model::setup::{Enrolled, Reply, WifiSaved};

    let mut rig = Rig::new();
    // Closed until a Lock and Dispense hold opens it.
    assert_eq!(
        recover(&mut rig, STRANGER, Code::from_bits(7), Code::from_bits(7)),
        None
    );
    assert!(rig.ended(Reason::Recovery));

    open_recovery(&mut rig);
    let code = Code::from_bits(0x0abc);
    assert_eq!(
        recover(&mut rig, STRANGER, code, code),
        Some(Reply::Done {
            enrolled: Enrolled::New,
            wifi: WifiSaved::Unchanged
        })
    );
    assert!(
        rig.device
            .clients
            .iter()
            .any(|c| c.key == public(STRANGER) && c.role == Role::Owner)
    );
    assert!(rig.device.events.iter().any(|e| matches!(
        e.body,
        EventBody::Enrolled {
            path: proto::Path::Recovery,
            ..
        }
    )));

    // The offer closed the window: the next client is turned away.
    assert_eq!(
        recover(&mut rig, [0x70; 32], code, code),
        None,
        "one owner per window"
    );
    rig.publish();
    let _owner = open(&mut rig, STRANGER);
}

#[test]
fn a_wrong_code_is_refused_and_enrolls_nobody() {
    use wellfed_model::setup::{Refusal as SetupRefusal, Reply};

    let mut rig = Rig::new();
    open_recovery(&mut rig);
    let reply = recover(
        &mut rig,
        STRANGER,
        Code::from_bits(0x0abc),
        Code::from_bits(0x0abd),
    );
    assert_eq!(reply, Some(Reply::Refused(SetupRefusal::WrongCode)));
    assert!(!rig.device.clients.iter().any(|c| c.key == public(STRANGER)));
}

/// One sans-IO connection on one slot, with the client's clock tied to the
/// rig's passes.
struct Live {
    slot: usize,
    connection: Connection,
    base: Instant,
    outbox: Vec<u8>,
    events: Vec<Event>,
}

impl Live {
    fn now(&self, rig: &Rig) -> Instant {
        self.base + Duration::from_millis(u64::from(rig.now))
    }
}

fn live_with(rig: &mut Rig, secret: [u8; 32], hello: Hello) -> Live {
    let slot = rig.accept().unwrap();
    let base = Instant::now();
    let now = base + Duration::from_millis(u64::from(rig.now));
    let keys = Keypair::from_secret(secret);
    let connection = Connection::with(
        &keys,
        public(FEEDER),
        [0x77; 32],
        hello,
        Timing::DEFAULT,
        now,
    )
    .unwrap();
    Live {
        slot,
        connection,
        base,
        outbox: Vec::new(),
        events: Vec::new(),
    }
}

/// Passes, moving bytes both ways and collecting events, until `done` or
/// the connection fails.
fn drive(rig: &mut Rig, live: &mut Live, mut done: impl FnMut(&Live) -> bool) -> Result<(), Error> {
    for _ in 0..200 {
        while let Some(bytes) = live.connection.poll_transmit() {
            live.outbox.extend(bytes);
        }
        rig.deliver(live.slot, &mut live.outbox);
        rig.pass();
        let bytes = rig.collect(live.slot);
        rig.device.write();
        if !bytes.is_empty() {
            let now = live.now(rig);
            live.connection.handle_input(now, &bytes)?;
        }
        while let Some(event) = live.connection.poll_event() {
            live.events.push(event);
        }
        if done(live) {
            return Ok(());
        }
    }
    panic!("no progress");
}

fn live(rig: &mut Rig, secret: [u8; 32]) -> Live {
    let hello = Hello {
        major: 1,
        capabilities: model::Capabilities::KNOWN,
    };
    let mut live = live_with(rig, secret, hello);
    drive(rig, &mut live, |live| live.connection.opened().is_some()).unwrap();
    live
}

fn pushed(live: &Live) -> Vec<model::Event> {
    live.events
        .iter()
        .filter_map(|event| match event {
            Event::Pushed(model::Pushed::Event(event)) => Some(*event),
            _ => None,
        })
        .collect()
}

#[test]
fn a_connection_names_the_epoch_and_answers_each_request_once() {
    let mut rig = Rig::new();
    let mut owner = live(&mut rig, OWNER);
    let opened = owner.connection.opened().unwrap().clone();
    assert_eq!(
        (opened.role, opened.capabilities, opened.epoch),
        (
            model::ClientRole::Owner,
            model::Capabilities {
                feed: true,
                update: false,
                time: true,
                log: false,
            },
            EPOCH
        )
    );

    let now = owner.now(&rig);
    owner
        .connection
        .request(now, [1; 16], &model::Op::Status)
        .unwrap();
    owner
        .connection
        .request(now, [2; 16], &model::Op::GetSettings)
        .unwrap();
    drive(&mut rig, &mut owner, |live| live.events.len() == 2).unwrap();
    let answered: Vec<_> = owner
        .events
        .iter()
        .map(|event| match event {
            Event::Answer { id, answer } => (id[0], matches!(answer, model::Answer::Status(_))),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(answered, [(1, true), (2, false)]);
    assert!(matches!(
        owner
            .connection
            .request(now, KEEPALIVE_ID, &model::Op::Status),
        Err(Error::ReservedId)
    ));
}

#[test]
fn an_unanswered_request_is_reported_once_and_its_late_answer_dropped() {
    let mut rig = Rig::new();
    let mut owner = live(&mut rig, OWNER);
    let sent = owner.now(&rig);
    owner
        .connection
        .request(sent, [3; 16], &model::Op::Status)
        .unwrap();

    // The answer time passes before the bytes reach the feeder.
    let overdue = sent + Timing::DEFAULT.answer;
    assert!(owner.connection.poll_timeout().unwrap() <= overdue);
    owner.connection.handle_timeout(overdue).unwrap();
    assert_eq!(
        owner.connection.poll_event(),
        Some(Event::Unanswered { id: [3; 16] })
    );

    let mut passes = 0;
    drive(&mut rig, &mut owner, |_| {
        passes += 1;
        passes == 20
    })
    .unwrap();
    assert!(owner.events.is_empty(), "{:?}", owner.events);
}

#[test]
fn the_keepalive_goes_out_when_quiet_and_silence_ends_the_session() {
    let mut rig = Rig::new();
    let mut owner = live(&mut rig, OWNER);
    while owner.connection.poll_transmit().is_some() {}
    let heard = owner.now(&rig);

    let due = owner.connection.poll_timeout().unwrap();
    assert!(due <= heard + Timing::DEFAULT.keepalive, "{due:?}");
    owner.connection.handle_timeout(due).unwrap();
    let keepalive = owner.connection.poll_transmit().expect("a keepalive");
    owner.outbox.extend(keepalive);
    let mut passes = 0;
    drive(&mut rig, &mut owner, |_| {
        passes += 1;
        passes == 20
    })
    .unwrap();
    assert!(
        owner.events.is_empty(),
        "the keepalive's answer is swallowed"
    );

    // Nothing heard for the silence limit: the feeder is gone.
    let silent = owner.now(&rig) + Timing::DEFAULT.silence;
    assert!(matches!(
        owner.connection.handle_timeout(silent),
        Err(Error::Silent)
    ));
    assert!(matches!(
        owner
            .connection
            .request(silent, [4; 16], &model::Op::Status),
        Err(Error::Silent)
    ));
    assert_eq!(owner.connection.poll_timeout(), None);
}

#[test]
fn a_major_2_hello_gets_the_typed_version_refusal() {
    let mut rig = Rig::new();
    let hello = Hello {
        major: 2,
        capabilities: model::Capabilities::KNOWN,
    };
    let mut client = live_with(&mut rig, OWNER, hello);
    let ended = drive(&mut rig, &mut client, |_| false);
    assert!(
        matches!(
            ended,
            Err(Error::Refused(OpenRefusal::Version { supported: 1 }))
        ),
        "{ended:?}"
    );
}

#[test]
fn a_welcome_waits_for_the_event_ring() {
    let mut rig = Rig::new();
    rig.device.epoch = None;
    let hello = Hello {
        major: 1,
        capabilities: model::Capabilities::KNOWN,
    };
    let mut owner = live_with(&mut rig, OWNER, hello);
    let mut passes = 0;
    drive(&mut rig, &mut owner, |_| {
        passes += 1;
        passes == 20
    })
    .unwrap();
    assert!(owner.connection.opened().is_none());

    rig.device.epoch = Some(EPOCH);
    drive(&mut rig, &mut owner, |live| {
        live.connection.opened().is_some()
    })
    .unwrap();
}

#[test]
fn a_cursor_resumes_in_its_epoch_and_restarts_from_another() {
    let mut rig = Rig::new();
    for generation in 1..=3 {
        let by = proto::Source::Panel;
        rig.device.record(0, EventBody::Settings { generation, by });
    }

    let mut owner = live(&mut rig, OWNER);
    let same = Cursor {
        epoch: EPOCH,
        seq: 2,
    };
    let now = owner.now(&rig);
    let resume = owner.connection.subscribe(now, Some(same), false).unwrap();
    assert_eq!(resume, Resume::Continued);
    drive(&mut rig, &mut owner, |live| pushed(live).len() == 1).unwrap();
    assert_eq!(pushed(&owner)[0].seq, 3);
    let newest = Cursor {
        epoch: EPOCH,
        seq: 3,
    };
    assert_eq!(owner.connection.cursor(), Some(newest));

    // A mirror of a ring since re-created: its numbers mean nothing here.
    let mut mirror = live(&mut rig, MEMBER);
    let stale = Cursor {
        epoch: EPOCH ^ 1,
        seq: 2,
    };
    let now = mirror.now(&rig);
    let resume = mirror
        .connection
        .subscribe(now, Some(stale), false)
        .unwrap();
    assert_eq!(resume, Resume::Restarted);
    drive(&mut rig, &mut mirror, |live| pushed(live).len() == 3).unwrap();
    let seqs: Vec<u64> = pushed(&mirror).iter().map(|e| e.seq).collect();
    assert_eq!(seqs, [1, 2, 3]);
    assert_eq!(mirror.connection.cursor(), Some(newest));
}

/// Every event body, one of each. `variant` has no wildcard, so a new
/// body fails to compile here until it joins the list.
fn every_body() -> Vec<EventBody> {
    fn variant(body: &EventBody) -> usize {
        match body {
            EventBody::Boot { .. } => 0,
            EventBody::Lock { .. } => 1,
            EventBody::Settings { .. } => 2,
            EventBody::Wifi { .. } => 3,
            EventBody::Enrolled { .. } => 4,
            EventBody::Revoked { .. } => 5,
            EventBody::ClockSync { .. } => 6,
            EventBody::Lost { .. } => 7,
            EventBody::Fed { .. } => 8,
        }
    }
    let by = proto::Source::Panel;
    let bodies = vec![
        EventBody::Boot {
            bank: 1,
            reset_flags: 0,
        },
        EventBody::Lock { locked: true, by },
        EventBody::Settings { generation: 1, by },
        EventBody::Wifi { generation: 1, by },
        EventBody::Enrolled {
            client: [1; 4],
            role: proto::Role::Member,
            path: proto::Path::Invite,
        },
        EventBody::Revoked { client: [1; 4], by },
        EventBody::ClockSync { by, jump_ms: None },
        EventBody::Lost { count: 1 },
        EventBody::Fed {
            feed: 1,
            requested: 1,
            completed: 1,
            outcome: proto::FeedOutcome::Done,
            reason: proto::FeedReason::Done,
            by,
        },
    ];
    let mut seen: Vec<usize> = bodies.iter().map(variant).collect();
    seen.dedup();
    assert_eq!(seen, (0..bodies.len()).collect::<Vec<_>>());
    bodies
}

#[test]
fn a_v1_only_session_never_receives_a_body_outside_v1() {
    let mut rig = Rig::new();
    let bodies = every_body();
    for body in &bodies {
        rig.device.record(0, *body);
    }
    // A last V1 event, so the session knows it has seen everything.
    rig.device.record(0, EventBody::Lost { count: 9 });
    let v1: Vec<EventBody> = bodies
        .iter()
        .copied()
        .filter(|body| body.capability() == Capabilities::V1)
        .chain([EventBody::Lost { count: 9 }])
        .collect();

    let hello = Hello {
        major: 1,
        capabilities: model::Capabilities::V1,
    };
    let mut old = live_with(&mut rig, OWNER, hello);
    drive(&mut rig, &mut old, |live| {
        live.connection.opened().is_some()
    })
    .unwrap();
    let now = old.now(&rig);
    old.connection.subscribe(now, None, false).unwrap();
    drive(&mut rig, &mut old, |live| {
        pushed(live).last().map(|e| EventBody::from(e.body)) == Some(EventBody::Lost { count: 9 })
    })
    .unwrap();

    let received: Vec<EventBody> = pushed(&old).iter().map(|e| e.body.into()).collect();
    assert_eq!(received, v1);
    assert!(v1.len() < bodies.len() + 1, "some body is outside V1");
}

#[test]
fn a_request_pending_when_the_session_ends_is_reported_unanswered() {
    let mut rig = Rig::new();
    let mut owner = live(&mut rig, OWNER);
    let sent = owner.now(&rig);
    owner
        .connection
        .request(sent, [5; 16], &model::Op::Status)
        .unwrap();

    // The feeder goes silent before the bytes reach it.
    let silent = sent + Timing::DEFAULT.silence;
    assert!(matches!(
        owner.connection.handle_timeout(silent),
        Err(Error::Silent)
    ));
    assert_eq!(
        owner.connection.poll_event(),
        Some(Event::Unanswered { id: [5; 16] })
    );
    assert_eq!(owner.connection.poll_event(), None);
}

/// A rig whose feeder serves updates, with a server key enrolled too.
fn update_rig() -> Rig {
    let mut rig = Rig::new();
    rig.device.update = true;
    rig.device.enroll(public(STRANGER), Role::Server);
    rig.publish();
    rig
}

const MANIFEST: [u8; 160] = [0xa5; 160];

fn offer() -> Op {
    Op::OfferUpdate {
        manifest: MANIFEST.to_vec(),
        downgrade: None,
    }
}

/// The feeder's view as a client reads it.
fn view(view: UpdateView) -> Answer {
    Answer::Update(view.into())
}

fn enroll_laptop() -> Op {
    Op::EnrollUpdateKey {
        key: [0x5e; 32],
        label: "laptop".into(),
    }
}

#[test]
fn only_an_owner_s_session_with_the_update_bit_may_update() {
    let mut rig = update_rig();
    let ops = [
        offer(),
        Op::UpdateChunk {
            offset: 0,
            data: vec![1; 512],
        },
        Op::UpdateState,
        Op::CancelUpdate,
        Op::EnrollUpdateKey {
            key: [3; 32],
            label: "laptop".into(),
        },
        Op::UpdateKeyStatement {
            statement: vec![4; 88],
        },
    ];
    for secret in [MEMBER, STRANGER] {
        let mut client = open(&mut rig, secret);
        for (n, op) in ops.iter().enumerate() {
            let answer = ask(
                &mut rig,
                &mut client,
                u8::try_from(n).unwrap() + 1,
                op.clone(),
            );
            assert_eq!(answer, Answer::Refused(model::Refusal::Forbidden), "{op:?}");
        }
    }
    assert!(rig.device.updates.is_empty() && rig.device.chunks.is_empty());

    // An owner whose hello lacks the bit, or a feeder that does not serve it.
    let mut rig = update_rig();
    let hello = Hello {
        major: 1,
        capabilities: model::Capabilities {
            feed: true,
            update: false,
            time: false,
            log: true,
        },
    };
    let slot = rig.accept().unwrap();
    let (session, outbox) =
        Session::connect_with(&Keypair::from_secret(OWNER), public(FEEDER), [7; 32], hello)
            .unwrap();
    let mut old = Client_ {
        slot,
        session,
        outbox,
    };
    run(&mut rig, &mut old, |session| {
        matches!(session.next_incoming(), Ok(Some(Incoming::Opened(_))))
    });
    let mut plain = Rig::new();
    let mut owner = open(&mut plain, OWNER);
    for (n, op) in ops.iter().enumerate() {
        let id = u8::try_from(n).unwrap() + 1;
        let unavailable = Answer::Refused(model::Refusal::Unavailable);
        assert_eq!(ask(&mut rig, &mut old, id, op.clone()), unavailable);
        assert_eq!(ask(&mut plain, &mut owner, id, op.clone()), unavailable);
    }
    assert!(rig.device.updates.is_empty());
}

#[test]
fn an_offer_waits_for_no_feed_and_a_feed_waits_for_the_install() {
    let mut rig = update_rig();
    let mut owner = open(&mut rig, OWNER);
    let busy_refusal = Answer::Refused(model::Refusal::Busy);

    // A feed is prepared or driven: the offer is refused, not queued.
    rig.feeding = true;
    assert_eq!(ask(&mut rig, &mut owner, 1, offer()), busy_refusal);
    rig.feeding = false;

    // A feed queued but not yet reserved holds the offer off too.
    rig.device.feeds.push((1, proto::Source::Panel));
    assert_eq!(ask(&mut rig, &mut owner, 2, offer()), busy_refusal);
    rig.device.reserved = 1;
    assert!(rig.device.updates.is_empty());

    assert_eq!(ask(&mut rig, &mut owner, 3, offer()), view(receiving()));
    assert_eq!(rig.device.updates.len(), 1);
    assert!(rig.device.installing);

    let feed = ask(&mut rig, &mut owner, 4, Op::Feed { portions: 1 });
    assert_eq!(feed, busy_refusal);
    assert_eq!(rig.device.feeds.len(), 1, "no feed during the install");

    // The same offer id again: the state now, not a second offer.
    assert_eq!(ask(&mut rig, &mut owner, 3, offer()), view(receiving()));
    assert_eq!(rig.device.updates.len(), 1);

    let cancel = ask(&mut rig, &mut owner, 5, Op::CancelUpdate);
    assert!(matches!(
        cancel,
        Answer::Update(model::UpdateView {
            phase: model::UpdatePhase::Idle,
            ..
        })
    ));
    let feed = ask(&mut rig, &mut owner, 6, Op::Feed { portions: 1 });
    assert_eq!(feed, Answer::Feeding { feed: 2 });
}

#[test]
fn a_statement_or_a_cancel_waits_for_no_feed_and_a_feed_waits_for_either() {
    let mut rig = update_rig();
    let mut owner = open(&mut rig, OWNER);
    let mut other = open(&mut rig, OWNER);
    let busy = Answer::Refused(model::Refusal::Busy);
    let statement = Op::UpdateKeyStatement {
        statement: vec![4; 88],
    };

    // Both may write the update store: never while a feed is driven or
    // queued.
    rig.feeding = true;
    assert_eq!(ask(&mut rig, &mut owner, 1, statement.clone()), busy);
    assert_eq!(ask(&mut rig, &mut owner, 2, Op::CancelUpdate), busy);
    rig.feeding = false;
    rig.device.feeds.push((1, proto::Source::Panel));
    assert_eq!(ask(&mut rig, &mut owner, 3, statement.clone()), busy);
    rig.device.feeds.push((1, proto::Source::Panel));
    assert_eq!(ask(&mut rig, &mut owner, 4, Op::CancelUpdate), busy);
    assert!(rig.device.updates.is_empty());

    // A statement waiting for the update stage holds the flash: no feed
    // until it is answered.
    rig.device.hold = true;
    assert_eq!(ask_within(&mut rig, &mut owner, 5, statement, 5), None);
    let feed = ask(&mut rig, &mut other, 6, Op::Feed { portions: 1 });
    assert_eq!(feed, busy);
    assert_eq!(rig.device.feeds.len(), 2, "no feed while it waits");

    rig.device.hold = false;
    rig.device.answer_held();
    let done = poll_answer(&mut rig, &mut owner, 5, 50).unwrap();
    assert_eq!(done, Answer::Done { generation: None });
    let feed = ask(&mut rig, &mut other, 7, Op::Feed { portions: 1 });
    assert_eq!(feed, Answer::Feeding { feed: 3 });
}

#[test]
fn a_session_key_is_never_enrolled_as_an_update_signer() {
    let mut rig = update_rig();
    let mut owner = open(&mut rig, OWNER);
    let enroll = Op::EnrollUpdateKey {
        key: public(MEMBER),
        label: "phone".into(),
    };
    assert_eq!(ask_within(&mut rig, &mut owner, 1, enroll, 50), None);

    rig.signer = press_dispense(&mut rig);
    let refused = poll_answer(&mut rig, &mut owner, 1, 50).unwrap();
    assert_eq!(refused, Answer::Refused(model::Refusal::Invalid));
    let member = rig.device.clients.iter().find(|c| c.key == public(MEMBER));
    assert_eq!(member.map(|c| c.role), Some(Role::Member));
}

#[test]
fn chunks_are_answered_with_the_state_and_never_push_out_a_remembered_write() {
    let mut rig = update_rig();
    let mut owner = open(&mut rig, OWNER);
    let put = Op::PutSettings {
        base: Generation(7),
        changes: SettingsPatch {
            volume: Some(50),
            ..SettingsPatch::default()
        },
    };
    let first = ask(&mut rig, &mut owner, 200, put.clone());
    assert_eq!(
        first,
        Answer::Done {
            generation: Some(Generation(8))
        }
    );

    let not_yet = ask(
        &mut rig,
        &mut owner,
        1,
        Op::UpdateChunk {
            offset: 0,
            data: vec![1; 512],
        },
    );
    assert_eq!(
        not_yet,
        Answer::Refused(model::Refusal::Update(model::UpdateRefusal::NotInstalling))
    );
    ask(&mut rig, &mut owner, 2, offer());

    // More chunks than the server remembers writes.
    for n in 0..40u8 {
        let offset = u32::from(n) * 512;
        let answer = ask(
            &mut rig,
            &mut owner,
            n + 10,
            Op::UpdateChunk {
                offset,
                data: vec![n; 512],
            },
        );
        assert_eq!(answer, view(receiving()));
    }
    assert_eq!(rig.device.chunks.len(), 40);
    let oversized = ask(
        &mut rig,
        &mut owner,
        60,
        Op::UpdateChunk {
            offset: 0,
            data: vec![0; 513],
        },
    );
    assert_eq!(oversized, Answer::Refused(model::Refusal::Invalid));

    let retry = ask(&mut rig, &mut owner, 200, put);
    assert_eq!(retry, first, "the settings write is still remembered");
    assert_eq!(rig.device.settings.generation, 8);
    let logged = rig
        .records
        .iter()
        .filter(|r| {
            matches!(
                r,
                Record::Request {
                    op: "update-chunk",
                    ..
                }
            )
        })
        .count();
    assert_eq!(
        logged, 1,
        "only the refused chunk is logged, not every one taken"
    );
}

#[test]
fn a_key_statement_is_answered_done_and_logged_by_its_key_id() {
    let mut rig = update_rig();
    let mut owner = open(&mut rig, OWNER);
    let mut statement = [0u8; 88];
    statement[..8].copy_from_slice(&[0xc1; 8]);
    let done = ask(
        &mut rig,
        &mut owner,
        1,
        Op::UpdateKeyStatement {
            statement: statement.to_vec(),
        },
    );
    assert_eq!(done, Answer::Done { generation: None });
    assert!(rig.records.iter().any(|r| matches!(
        r,
        Record::UpdateKey {
            change: UpdateKeyChange::Retired,
            key,
            ..
        } if *key == [0xc1; 8]
    )));
}

/// Send a request and run `passes` passes; its answer, if one came.
fn ask_within(
    rig: &mut Rig,
    client: &mut Client_,
    id: u8,
    op: Op,
    passes: usize,
) -> Option<Answer> {
    let bytes = client.session.request([id; 16], &op).unwrap();
    client.outbox.extend(bytes);
    poll_answer(rig, client, id, passes)
}

fn poll_answer(rig: &mut Rig, client: &mut Client_, id: u8, passes: usize) -> Option<Answer> {
    for _ in 0..passes {
        rig.deliver(client.slot, &mut client.outbox);
        rig.pass();
        let bytes = rig.collect(client.slot);
        if !bytes.is_empty() {
            client.session.receive(&bytes).unwrap();
        }
        rig.device.write();
        while let Some(incoming) = client.session.next_incoming().unwrap() {
            if let Incoming::Answer { id: got, answer } = incoming
                && got == [id; 16]
            {
                return Some(answer);
            }
        }
    }
    None
}

fn press_dispense(rig: &mut Rig) -> Option<PairingEvent> {
    let press = Gesture::Down {
        chord: Buttons::of(Button::Dispense),
    };
    rig.pairing.gesture(rig.now, press, Lock::Unlocked)
}

#[test]
fn an_update_key_enrolls_only_once_dispense_is_pressed() {
    let mut rig = update_rig();
    let mut owner = open(&mut rig, OWNER);
    assert_eq!(
        ask_within(&mut rig, &mut owner, 1, enroll_laptop(), 50),
        None
    );
    assert!(!rig.device.clients.iter().any(|c| c.key == [0x5e; 32]));

    // A second offer while the first waits for its press.
    let mut other = open(&mut rig, OWNER);
    let busy = ask(&mut rig, &mut other, 2, enroll_laptop());
    assert_eq!(busy, Answer::Refused(model::Refusal::Busy));

    rig.signer = press_dispense(&mut rig);
    assert!(matches!(rig.signer, Some(PairingEvent::Admit { .. })));
    let done = poll_answer(&mut rig, &mut owner, 1, 50).unwrap();
    assert_eq!(done, Answer::Done { generation: None });
    let enrolled = rig.device.clients.iter().find(|c| c.key == [0x5e; 32]);
    assert_eq!(enrolled.map(|c| c.role), Some(Role::UpdateSigner));
    assert!(rig.records.iter().any(|r| matches!(
        r,
        Record::UpdateKey {
            change: UpdateKeyChange::Enrolled,
            ..
        }
    )));
}

#[test]
fn an_unpressed_update_key_is_refused_and_a_closed_session_cancels_its_offer() {
    let mut rig = update_rig();
    let mut owner = open(&mut rig, OWNER);
    assert_eq!(
        ask_within(&mut rig, &mut owner, 1, enroll_laptop(), 5),
        None
    );
    rig.now += Config::DEFAULT.confirm_ms;
    rig.signer = rig.pairing.tick(rig.now);
    let refused = poll_answer(&mut rig, &mut owner, 1, 50).unwrap();
    assert_eq!(
        refused,
        Answer::Refused(model::Refusal::Update(model::UpdateRefusal::Unconfirmed))
    );

    // The client leaves while its offer waits: the next press feeds again,
    // and a leftover decision never admits a later offer.
    assert_eq!(
        ask_within(&mut rig, &mut owner, 2, enroll_laptop(), 5),
        None
    );
    let records = &mut rig.records;
    rig.server.closed(owner.slot, |r| records.push(r));
    rig.pass();
    assert_eq!(
        press_dispense(&mut rig),
        None,
        "no offer is pending any more"
    );
    assert!(!rig.device.clients.iter().any(|c| c.key == [0x5e; 32]));
}

fn log_rig() -> Rig {
    let mut rig = Rig::new();
    rig.device.log = true;
    rig
}

fn log_line(n: u32) -> String {
    format!("PLTST1 line={n:04} {}", "x".repeat(80))
}

/// Run until the session has read every log line up to `seq`: the lines
/// read and the dropped counts, each batch checked against the frame.
fn follow_log(rig: &mut Rig, client: &mut Client_, seq: u32) -> (Vec<String>, u32) {
    let (mut lines, mut dropped, mut last) = (Vec::new(), 0, 0);
    run(rig, client, |session| {
        while let Some(incoming) = session.next_incoming().unwrap() {
            if let Incoming::Pushed(model::Pushed::Log(batch)) = incoming {
                let text: usize = batch.lines.iter().map(|line| line.len() + 1).sum();
                assert!(text <= proto::LOG_TEXT_MAX, "{text} bytes");
                assert!(batch.seq > last, "batches go forward");
                last = batch.seq;
                dropped += batch.dropped;
                lines.extend(batch.lines);
            }
        }
        last == seq
    });
    (lines, dropped)
}

#[test]
fn an_owner_follows_the_log_in_batches_that_fit_a_frame() {
    let mut rig = log_rig();
    for n in 1..=30 {
        rig.device
            .tail
            .push(format!("{}\r\n", log_line(n)).as_bytes());
    }

    let mut owner = open(&mut rig, OWNER);
    let answer = ask(&mut rig, &mut owner, 1, Op::TailLog { after: None });
    assert_eq!(answer, Answer::Done { generation: None });
    let (lines, dropped) = follow_log(&mut rig, &mut owner, 30);
    let expected: Vec<_> = (1..=30).map(log_line).collect();
    assert_eq!(
        lines, expected,
        "every line, in order, across several frames"
    );
    assert_eq!(dropped, 0);

    // New lines follow live, and a request is still answered between them.
    rig.device.tail.push(log_line(31).as_bytes());
    let (lines, _) = follow_log(&mut rig, &mut owner, 31);
    assert_eq!(lines, [log_line(31)]);
    let answer = ask(&mut rig, &mut owner, 2, Op::Status);
    assert!(matches!(answer, Answer::Status(_)), "{answer:?}");

    // Asking again moves the cursor back.
    ask(&mut rig, &mut owner, 3, Op::TailLog { after: Some(29) });
    let (lines, _) = follow_log(&mut rig, &mut owner, 31);
    assert_eq!(lines, [log_line(30), log_line(31)]);
}

#[test]
fn lines_the_tail_overwrote_before_they_were_sent_are_counted_dropped() {
    let mut rig = log_rig();
    rig.device.tail.push(log_line(1).as_bytes());
    let mut owner = open(&mut rig, OWNER);
    ask(&mut rig, &mut owner, 1, Op::TailLog { after: None });
    follow_log(&mut rig, &mut owner, 1);

    // Far more than the tail holds, between two of the session's turns.
    for n in 2..=200 {
        rig.device.tail.push(log_line(n).as_bytes());
    }
    let (lines, dropped) = follow_log(&mut rig, &mut owner, 200);
    assert!(dropped > 100, "{dropped}");
    assert_eq!(u32::try_from(lines.len()).unwrap() + dropped, 199);
    assert_eq!(lines.last(), Some(&log_line(200)));
}

#[test]
fn only_an_owner_whose_session_both_sides_log_may_tail() {
    let tail = Op::TailLog { after: None };
    let mut rig = log_rig();
    rig.device.tail.push(log_line(1).as_bytes());
    let mut member = open(&mut rig, MEMBER);
    let answer = ask(&mut rig, &mut member, 1, tail.clone());
    assert_eq!(answer, Answer::Refused(model::Refusal::Forbidden));

    // An owner whose hello lacks the bit, or a feeder that keeps no log.
    let hello = Hello {
        major: 1,
        capabilities: model::Capabilities {
            log: false,
            ..model::Capabilities::KNOWN
        },
    };
    let slot = rig.accept().unwrap();
    let (session, outbox) =
        Session::connect_with(&Keypair::from_secret(OWNER), public(FEEDER), [7; 32], hello)
            .unwrap();
    let mut old = Client_ {
        slot,
        session,
        outbox,
    };
    run(&mut rig, &mut old, |session| {
        matches!(session.next_incoming(), Ok(Some(Incoming::Opened(_))))
    });
    let unavailable = Answer::Refused(model::Refusal::Unavailable);
    assert_eq!(ask(&mut rig, &mut old, 1, tail.clone()), unavailable);
    let mut plain = Rig::new();
    let mut owner = open(&mut plain, OWNER);
    assert_eq!(ask(&mut plain, &mut owner, 1, tail), unavailable);

    // None of them is ever pushed a line.
    for _ in 0..20 {
        rig.pass();
        plain.pass();
        for client in [&mut member, &mut old] {
            let bytes = rig.collect(client.slot);
            client.session.receive(&bytes).unwrap();
        }
        let bytes = plain.collect(owner.slot);
        owner.session.receive(&bytes).unwrap();
    }
    for session in [&mut member.session, &mut old.session, &mut owner.session] {
        while let Some(incoming) = session.next_incoming().unwrap() {
            assert!(!matches!(incoming, Incoming::Pushed(model::Pushed::Log(_))));
        }
    }
}
