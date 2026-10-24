//! The device API end to end on the host: `wellfed-client`'s sessions
//! against the feeder's `Server` (`wellfed-feeder::api`), with a fake
//! device standing in for the mailboxes and the settings writer. Each test
//! moves bytes between the two by hand, so it controls every pass.

use wellfed_client::api::{Incoming, Invite, Join, JoinStep, Session};
use wellfed_core::buttons::{Button, Buttons, Gesture};
use wellfed_core::noise::Keypair;
use wellfed_core::setup_code::Code;
use wellfed_core::store::clients::{CLIENTS, Client, Label, Role};
use wellfed_core::store::net::Credentials;
use wellfed_core::tone::Tune;
use wellfed_core::transport::{
    LockResult, PlayResult, SettingsFields, SettingsResult, SettingsView, TimeFields, TimeResult,
    WifiResult, WifiSource, WifiView,
};
use wellfed_feeder::api::{
    Device, EventRead, KeyChange, KeyOutcome, Pass, Reason, Record, SESSIONS, Server,
};
use wellfed_feeder::pairing::{Config, Event as PairingEvent, Pairing};
use wellfed_feeder::panel::Lock;
use wellfed_proto::api::{
    self as proto, EventBody, FeederMessage, Joined, Op, OpenRefusal, Outcome, Refusal,
    SettingsChanges, Status, Welcome,
};

const FEEDER: [u8; 32] = [0x10; 32];
const OWNER: [u8; 32] = [0x20; 32];
const MEMBER: [u8; 32] = [0x30; 32];
const STRANGER: [u8; 32] = [0x40; 32];

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
    rebooted: bool,
    syncs: usize,
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

    fn time_sync(&mut self, _fields: TimeFields) -> TimeResult {
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
    recovery: Option<wellfed_feeder::pairing::Event>,
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

    fn pass(&mut self) {
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
            },
            |r| records.push(r),
        );
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

fn connect(rig: &mut Rig, secret: [u8; 32]) -> (Client_, Option<Welcome<'static>>) {
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
        if let Some(Incoming::Welcome(w)) = session.next_incoming().unwrap() {
            welcome = Some(match w {
                Welcome::Open { role, .. } => Welcome::Open {
                    major: 1,
                    capabilities: proto::Capabilities::V1,
                    build: "",
                    role,
                },
                Welcome::Refused(r) => Welcome::Refused(r),
            });
            return true;
        }
        false
    });
    (client, welcome)
}

fn open(rig: &mut Rig, secret: [u8; 32]) -> Client_ {
    let (client, welcome) = connect(rig, secret);
    assert!(matches!(welcome, Some(Welcome::Open { .. })), "{welcome:?}");
    client
}

/// Send one request and run until its answer; the answer, re-encoded so it
/// outlives the session's borrow.
fn ask(rig: &mut Rig, client: &mut Client_, id: u8, op: Op<'_>) -> Vec<u8> {
    let bytes = client.session.request([id; 16], op).unwrap();
    client.outbox.extend(bytes);
    let mut answer = None;
    run(rig, client, |session| {
        while let Some(incoming) = session.next_incoming().unwrap() {
            if let Incoming::Message(FeederMessage::Response { id: got, result }) = incoming
                && got == [id; 16]
            {
                let mut out = vec![0; 1024];
                let len = wellfed_proto::encode(&result, &mut out).unwrap();
                out.truncate(len);
                answer = Some(out);
                return true;
            }
        }
        false
    });
    answer.unwrap()
}

fn outcome(bytes: &[u8]) -> Outcome<'_> {
    wellfed_proto::decode(bytes).unwrap()
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
    let Outcome::Status(status) = outcome(&status) else {
        panic!("{status:?}");
    };
    assert_eq!(status.settings_generation, 7);
    assert_eq!(status.sessions, 1);

    let settings = ask(&mut rig, &mut owner, 2, Op::GetSettings);
    assert!(matches!(
        outcome(&settings),
        Outcome::Settings(proto::Settings {
            generation: 7,
            volume: 40,
            ..
        })
    ));
}

#[test]
fn an_unknown_key_and_the_wrong_feeder_are_refused() {
    let mut rig = Rig::new();
    let (_, welcome) = connect(&mut rig, STRANGER);
    assert_eq!(welcome, Some(Welcome::Refused(OpenRefusal::Unknown)));
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
    let changes = SettingsChanges {
        volume: Some(60),
        ..SettingsChanges::default()
    };

    let done = ask(
        &mut rig,
        &mut owner,
        1,
        Op::PutSettings { base: 7, changes },
    );
    assert_eq!(
        outcome(&done),
        Outcome::Done {
            generation: Some(8)
        }
    );
    assert_eq!(rig.device.settings.volume, 60);

    let stale = ask(
        &mut rig,
        &mut owner,
        2,
        Op::PutSettings { base: 7, changes },
    );
    assert_eq!(
        outcome(&stale),
        Outcome::Refused(Refusal::Stale { current: 8 })
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
    let changes = SettingsChanges {
        lights: Some(false),
        ..SettingsChanges::default()
    };
    let first = ask(
        &mut rig,
        &mut owner,
        9,
        Op::PutSettings { base: 7, changes },
    );
    assert_eq!(
        outcome(&first),
        Outcome::Done {
            generation: Some(8)
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
        Op::PutSettings { base: 7, changes },
    );
    assert_eq!(retry, first);
    assert_eq!(rig.device.settings.generation, 8, "written once");
    assert!(
        rig.records
            .iter()
            .any(|r| matches!(r, Record::Retried { .. }))
    );
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
            base: 3,
            wifi: None,
        },
    );
    assert_eq!(outcome(&wifi), Outcome::Refused(Refusal::Forbidden));
    let revoke = ask(
        &mut rig,
        &mut member,
        2,
        Op::RevokeClient { key: public(OWNER) },
    );
    assert_eq!(outcome(&revoke), Outcome::Refused(Refusal::Forbidden));
    let reboot = ask(&mut rig, &mut member, 3, Op::Reboot);
    assert_eq!(outcome(&reboot), Outcome::Refused(Refusal::Forbidden));

    let lock = ask(&mut rig, &mut member, 4, Op::SetLock { locked: true });
    assert_eq!(outcome(&lock), Outcome::Done { generation: None });
    let feed = ask(&mut rig, &mut member, 5, Op::Feed { portions: 1 });
    assert_eq!(outcome(&feed), Outcome::Refused(Refusal::Unavailable));
}

#[test]
fn revoking_a_client_closes_its_open_session() {
    let mut rig = Rig::new();
    let mut owner = open(&mut rig, OWNER);
    let member = open(&mut rig, MEMBER);

    let listed = ask(&mut rig, &mut owner, 1, Op::ListClients);
    let Outcome::Clients(clients) = outcome(&listed) else {
        panic!("{listed:?}");
    };
    assert_eq!(clients.clients.iter().flatten().count(), 2);
    assert!(
        clients
            .clients
            .iter()
            .flatten()
            .any(|c| c.you && c.role == proto::Role::Owner)
    );

    let revoke = ask(
        &mut rig,
        &mut owner,
        2,
        Op::RevokeClient {
            key: public(MEMBER),
        },
    );
    assert_eq!(outcome(&revoke), Outcome::Done { generation: None });
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
            role: proto::Role::Member,
        },
    );
    let Outcome::Invite(invite) = outcome(&invite) else {
        panic!("{invite:?}");
    };
    let invite = Invite {
        feeder: public(FEEDER),
        id: invite.id,
        secret: invite.secret,
    };

    let joined = redeem(&mut rig, invite, STRANGER);
    assert_eq!(
        joined,
        Some(Joined::Enrolled {
            role: proto::Role::Member
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
                Incoming::Message(FeederMessage::Gap { from, to }) => seen.push((from, to)),
                Incoming::Message(FeederMessage::Event(event)) => seen.push((event.seq, 0)),
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
        if let Some(Incoming::Message(FeederMessage::Event(event))) =
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
        let bytes = flood.session.request([id; 16], Op::Status).unwrap();
        flood.outbox.extend(bytes);
    }
    for _ in 0..3 {
        rig.deliver(flood.slot, &mut flood.outbox);
        rig.pass();
        let _ = rig.collect(flood.slot);
    }
    let status = ask(&mut rig, &mut owner, 1, Op::Status);
    assert!(matches!(outcome(&status), Outcome::Status(_)));
}

#[test]
fn an_idle_session_and_a_stalled_handshake_time_out() {
    let mut rig = Rig::new();
    let owner = open(&mut rig, OWNER);
    let stalled = rig.accept().unwrap();

    rig.now += wellfed_feeder::api::HANDSHAKE_MS;
    rig.pass();
    assert!(rig.server.closing(stalled));
    rig.now += wellfed_feeder::api::IDLE_MS;
    rig.pass();
    assert!(rig.server.closing(owner.slot));
}

#[test]
fn reboot_and_time_sync_reach_the_device() {
    let mut rig = Rig::new();
    let mut owner = open(&mut rig, OWNER);
    let sync = proto::TimeSync {
        unix_ms: 1_700_000_000_000,
        offset_s: 0,
        next_change_s: 0,
        next_offset_s: 0,
    };
    let synced = ask(&mut rig, &mut owner, 1, Op::TimeSync(sync));
    assert_eq!(outcome(&synced), Outcome::Done { generation: None });
    assert_eq!(rig.device.syncs, 1);

    let reboot = ask(&mut rig, &mut owner, 2, Op::Reboot);
    assert_eq!(outcome(&reboot), Outcome::Done { generation: None });
    rig.now += wellfed_feeder::api::REBOOT_MS;
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
) -> Option<wellfed_proto::setup::Reply> {
    use wellfed_client::setup::{Ask, Deframer, Session as Setup, Step, frame};
    use wellfed_proto::setup::Reply;

    let slot = rig.accept().unwrap();
    let ask = Ask {
        label: "laptop".into(),
        wifi: None,
    };
    let keys = Keypair::from_secret(secret);
    let mut setup = Setup::new(&keys, [0x61; 32], ask, code, [0x62; 32]).unwrap();
    // The clear opening frame, then the setup characteristic's framing.
    let mut outbox = vec![1, 0, 2];
    outbox.extend(frame(&setup.start().unwrap()).unwrap());
    let mut deframer = Deframer::default();

    for _ in 0..400 {
        rig.deliver(slot, &mut outbox);
        rig.pass();
        deframer.push(&rig.collect(slot));
        rig.device.write();
        while let Some(message) = deframer.next_message().unwrap() {
            let step = match setup.receive(&message) {
                Ok(step) => step,
                // A mismatched code fails at whichever side opens a press
                // first; with the feeder writing first that is the client.
                Err(wellfed_client::setup::Error::WrongCode) => {
                    return Some(Reply::Refused(wellfed_proto::setup::Refusal::WrongCode));
                }
                Err(error) => panic!("{error}"),
            };
            match step {
                Step::Send(bytes) => outbox.extend(frame(&bytes).unwrap()),
                Step::Reply(Reply::EnterCode { .. }) => {
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
                Step::Reply(reply) => return Some(reply),
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
    use wellfed_proto::setup::{Enrolled, Reply, WifiSaved};

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
    use wellfed_proto::setup::{Refusal as SetupRefusal, Reply};

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
