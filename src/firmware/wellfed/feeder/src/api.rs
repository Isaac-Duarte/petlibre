//! The device API in the loop task. It owns the sockets and steps
//! `wellfed_feeder::api`'s server once per pass:
//! - the listener on [`PORT`] and up to [`SESSIONS`] connections, every
//!   call non-blocking, so the loop never waits on a client;
//! - the mDNS socket and the responder that advertises `_wellfed._tcp`
//!   (`wellfed_core::mdns`), once the identity is loaded and DHCP gave an
//!   address;
//! - the client table, copied from the settings writer when it changes, so
//!   a revoked key's sessions close;
//! - the live samples (power every 5 s, Wi-Fi signal every 30 s) for
//!   subscribers, and the durable events the rest of the loop records.
//!
//! [`Feeder`] is the server's view of the rest of the firmware: the HAL's
//! mailboxes, the clock and the event ring's mailbox. No record carries a
//! key, a secret or a credential.

use core::fmt;

use wellfed_core::mdns::{self, Destination, Responder};
use wellfed_core::store::events::{BODY_BYTES, Read};
use wellfed_core::store::net::Credentials;
use wellfed_core::tone::Tune;
use wellfed_core::transport::{
    LockFields, LockResult, PlayResult, SettingsFields, SettingsResult, TimeFields, TimeResult,
    WifiResult, WifiSource,
};
use wellfed_feeder::api::{
    self as server, Device, EventRead, KeyChange, KeyOutcome, Pass, SESSIONS, Server,
};
use wellfed_feeder::pairing::{self, Pairing};
use wellfed_proto::api::{
    self as proto, Capabilities, EventBody, Metric, PORT, Refusal, Sample, Source, Status,
};
use wellfed_sdk::hal::health::Boot;
use wellfed_sdk::hal::interlock::{INTERLOCK, Interlock};
use wellfed_sdk::hal::keys::{self, Change, KeyResult, Requested};
use wellfed_sdk::hal::sdk::{self, Accepted, ApiRecv, Datagram};
use wellfed_sdk::hal::{clock, events, net_status, network, panel, power, rng, settings, startup};
use wellfed_session::noise::Keypair;

/// A listener or mDNS socket that failed is opened again after this.
const RETRY_MS: u32 = 2_000;
const POWER_MS: u32 = power::SAMPLE_MS;
const RSSI_MS: u32 = 30_000;
/// mDNS datagrams read per pass, so a storm of queries costs a bounded turn.
const DATAGRAMS: usize = 4;
const HEAP_MS: u32 = 60_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Server(server::Record),
    /// The listener (or the mDNS socket) opened, or failed and will retry.
    Listen {
        mdns: bool,
        open: bool,
        /// The heap free just after, so each socket's cost shows.
        heap: u32,
    },
    /// The address mDNS advertises.
    Address {
        ip: u32,
    },
    /// Once a minute: the heap, and the mDNS datagrams seen and answered.
    Stats {
        free: u32,
        min: u32,
        queries: u32,
        answers: u32,
        failed: u32,
    },
}

/// One record's body (`PLAPI1`).
pub fn describe(record: Record, f: &mut impl fmt::Write) -> fmt::Result {
    match record {
        Record::Server(record) => server_record(record, f),
        Record::Listen { mdns, open, heap } => write!(
            f,
            "listen socket={} open={} heap_free={heap}",
            if mdns { "mdns" } else { "api" },
            u8::from(open)
        ),
        Record::Address { ip } => {
            let [a, b, c, d] = ip.to_be_bytes();
            write!(f, "mdns address={a}.{b}.{c}.{d} port={PORT}")
        }
        Record::Stats {
            free,
            min,
            queries,
            answers,
            failed,
        } => write!(
            f,
            "stats heap_free={free} heap_min={min} mdns_in={queries} mdns_out={answers} mdns_failed={failed}"
        ),
    }
}

fn server_record(record: server::Record, f: &mut impl fmt::Write) -> fmt::Result {
    use server::Record as R;
    match record {
        R::Accepted { slot } => write!(f, "session slot={slot} state=accepted"),
        R::Opened { slot, role, client } => write!(
            f,
            "session slot={slot} state=open role={role} client={}",
            wellfed_session::hex::Lower(&client)
        ),
        R::Request { slot, op, result } => {
            write!(f, "request slot={slot} op={op} result={result}")
        }
        R::Retried { slot } => write!(f, "request slot={slot} retried=1"),
        R::Ended { slot, reason } => write!(f, "session slot={slot} state=ended reason={reason}"),
        R::Pairing(event) => crate::owner::describe(crate::owner::Record::Pairing(event), f),
    }
}

/// The feeding owner, as the device API reaches it: the loop's
/// `feeding::Feeding` on the target. Generic rather than `dyn`, so the
/// calls stay direct for the stack audit.
pub trait Feeds {
    /// A feed is being prepared or driven: handshakes wait.
    fn feeding(&self) -> bool;
    /// Hand one feed to the owner; the number its journal holds it under.
    fn submit(&mut self, portions: u8, by: Source) -> Result<u32, Refusal>;
    /// Whether the feed `feed` is reserved yet: `None` while it waits.
    fn reserved(&self, feed: u32) -> Option<Result<(), Refusal>>;
    /// Stop the feed `feed` if it is still running.
    fn cancel(&self, feed: u32) -> bool;
}

/// What the loop hands the device API each pass.
pub struct Inputs<'a, F> {
    pub now_ms: u32,
    pub pairing: &'a mut Pairing,
    pub identity: Option<&'a Keypair>,
    /// The panel lock, once the owner has read it.
    pub locked: Option<bool>,
    pub recovery: Option<pairing::Event>,
    pub feeds: &'a mut F,
}

pub struct Api {
    server: Server,
    listener: Option<i32>,
    sockets: [Option<i32>; SESSIONS],
    mdns_socket: Option<i32>,
    responder: Option<Responder>,
    /// The address last logged.
    advertised: Option<u32>,
    /// The writer's client table generation this copy holds.
    clients_seen: u32,
    boot: Option<Boot>,
    retry_at: u32,
    next_power: u32,
    next_rssi: u32,
    next_heap: u32,
    /// mDNS datagrams received and answers sent, for the stats record.
    queries: u32,
    answers: u32,
    failed: u32,
    packet: [u8; mdns::PACKET],
    reply: [u8; mdns::PACKET],
}

impl Api {
    /// Built at compile time: the server's table is larger than the loop's
    /// stack.
    pub const fn new() -> Self {
        Self {
            server: Server::new(),
            listener: None,
            sockets: [None; SESSIONS],
            mdns_socket: None,
            responder: None,
            advertised: None,
            clients_seen: 0,
            boot: None,
            retry_at: 0,
            next_power: 0,
            next_rssi: 0,
            next_heap: 0,
            queries: 0,
            answers: 0,
            failed: 0,
            packet: [0; mdns::PACKET],
            reply: [0; mdns::PACKET],
        }
    }

    /// One pass; the delay until the next.
    pub fn step<F: Feeds>(&mut self, inputs: Inputs<'_, F>, mut record: impl FnMut(Record)) -> u32 {
        let Inputs {
            now_ms,
            pairing,
            identity,
            locked,
            recovery,
            feeds,
        } = inputs;
        let boot = *self.boot.get_or_insert_with(|| {
            let boot = boot_facts();
            let body = EventBody::Boot {
                bank: boot.bank,
                reset_flags: boot.reset_flags,
            };
            record_event(body);
            boot
        });
        let lost = events::take_lost();
        if lost > 0 {
            let count = u16::try_from(lost).unwrap_or(u16::MAX);
            record_event(EventBody::Lost { count });
        }

        let (table, seen) = self.server.clients_table();
        if let Some(generation) = keys::clients_since(seen.max(self.clients_seen), table) {
            self.clients_seen = generation;
            self.server
                .clients_changed(generation, |r| record(Record::Server(r)));
        }

        let ip = sdk::ipv4();
        if self.responder.is_none()
            && let Some(identity) = identity
        {
            self.responder = Some(Responder::new(identity.public(), PORT));
        }
        if let Some(responder) = self.responder.as_mut() {
            responder.set_ip(ip, now_ms);
        }
        if ip != self.advertised {
            self.advertised = ip;
            if let Some(ip) = ip {
                record(Record::Address { ip });
            }
        }
        if ip.is_some() {
            self.open_sockets(now_ms, &mut record);
        }
        self.mdns(now_ms);
        self.accept(now_ms, &mut record);
        self.receive(&mut record);

        // No handshake while the outputs are claimed, not only while they
        // move: an invite's join or a recovery enrolls a key, a store write
        // that would wait out the feed, so the invite is redeemed (and the
        // recovery code taken) only once it can be written. Held for at most
        // the claim's bound, as joins are.
        let feeding = feeds.feeding() || INTERLOCK.claim_in_bound(now_ms);
        let mut device = Feeder {
            boot,
            locked,
            feeds,
        };
        let busy = self.server.step(
            Pass {
                now_ms,
                device: &mut device,
                pairing,
                identity,
                feeding,
                recovery,
            },
            |r| record(Record::Server(r)),
        );
        self.send(&mut record);
        self.samples(now_ms);

        if wellfed_core::deadline_passed(now_ms, self.next_heap) {
            self.next_heap = now_ms.wrapping_add(HEAP_MS);
            record(Record::Stats {
                free: sdk::heap_free(),
                min: sdk::heap_min_free(),
                queries: self.queries,
                answers: self.answers,
                failed: self.failed,
            });
        }

        if busy { 1 } else { crate::owner::POLL_MS }
    }

    fn open_sockets(&mut self, now_ms: u32, record: &mut impl FnMut(Record)) {
        if !wellfed_core::deadline_passed(now_ms, self.retry_at) {
            return;
        }
        if self.listener.is_none() {
            self.listener = sdk::api_listen(PORT);
            record(Record::Listen {
                mdns: false,
                open: self.listener.is_some(),
                heap: sdk::heap_free(),
            });
        }
        if self.mdns_socket.is_none() && self.responder.is_some() {
            self.mdns_socket = sdk::mdns_open();
            record(Record::Listen {
                mdns: true,
                open: self.mdns_socket.is_some(),
                heap: sdk::heap_free(),
            });
        }
        if self.listener.is_none() || self.mdns_socket.is_none() {
            self.retry_at = now_ms.wrapping_add(RETRY_MS);
        }
    }

    fn mdns(&mut self, now_ms: u32) {
        let (Some(socket), Some(responder)) = (self.mdns_socket, self.responder.as_mut()) else {
            return;
        };
        if let Some(len) = responder.announce(now_ms, &mut self.reply)
            && !sdk::mdns_send(socket, &self.reply[..len], mdns::GROUP, mdns::PORT)
        {
            self.close_mdns(now_ms);
            return;
        }
        for _ in 0..DATAGRAMS {
            match sdk::mdns_recv(socket, &mut self.packet) {
                Datagram::None => return,
                Datagram::Error => return self.close_mdns(now_ms),
                Datagram::Got { len, ip, port } => {
                    self.queries = self.queries.wrapping_add(1);
                    let Some(reply) = responder.query(&self.packet[..len], port, &mut self.reply)
                    else {
                        continue;
                    };
                    let answer = &self.reply[..reply.len];
                    let (to, to_port) = match reply.to {
                        Destination::Multicast => (mdns::GROUP, mdns::PORT),
                        Destination::Unicast => (ip, port),
                    };
                    let sent = sdk::mdns_send(socket, answer, to, to_port);
                    // A lost answer is asked again; it is only counted.
                    if sent {
                        self.answers = self.answers.wrapping_add(1);
                    } else {
                        self.failed = self.failed.wrapping_add(1);
                    }
                }
            }
        }
    }

    fn close_mdns(&mut self, now_ms: u32) {
        if let Some(socket) = self.mdns_socket.take() {
            sdk::tcp_close(socket);
        }
        self.retry_at = now_ms.wrapping_add(RETRY_MS);
    }

    fn accept(&mut self, now_ms: u32, record: &mut impl FnMut(Record)) {
        let Some(listener) = self.listener else {
            return;
        };
        match sdk::api_accept(listener) {
            Accepted::None => {}
            Accepted::Error => {
                sdk::tcp_close(listener);
                self.listener = None;
                self.retry_at = now_ms.wrapping_add(RETRY_MS);
                record(Record::Listen {
                    mdns: false,
                    open: false,
                    heap: sdk::heap_free(),
                });
            }
            Accepted::Socket(socket) => {
                match self.server.accept(now_ms, |r| record(Record::Server(r))) {
                    Some(slot) => self.sockets[slot] = Some(socket),
                    None => sdk::tcp_close(socket),
                }
            }
        }
    }

    fn receive(&mut self, record: &mut impl FnMut(Record)) {
        for slot in 0..SESSIONS {
            let Some(socket) = self.sockets[slot] else {
                continue;
            };
            let room = self.server.rx(slot);
            if room.is_empty() {
                continue;
            }
            match sdk::api_recv(socket, room) {
                ApiRecv::Empty => {}
                ApiRecv::Data(n) => self.server.received(slot, n),
                ApiRecv::Closed => self.drop_slot(slot, record),
            }
        }
    }

    fn send(&mut self, record: &mut impl FnMut(Record)) {
        for slot in 0..SESSIONS {
            let Some(socket) = self.sockets[slot] else {
                continue;
            };
            let pending = self.server.tx(slot);
            if !pending.is_empty() {
                match sdk::api_send(socket, pending) {
                    Some(n) => self.server.sent(slot, n),
                    None => {
                        self.drop_slot(slot, record);
                        continue;
                    }
                }
            }
            if self.server.closing(slot) {
                self.drop_slot(slot, record);
            }
        }
    }

    fn drop_slot(&mut self, slot: usize, record: &mut impl FnMut(Record)) {
        if let Some(socket) = self.sockets[slot].take() {
            sdk::tcp_close(socket);
        }
        self.server.closed(slot, |r| record(Record::Server(r)));
    }

    fn samples(&mut self, now_ms: u32) {
        if wellfed_core::deadline_passed(now_ms, self.next_power) {
            self.next_power = now_ms.wrapping_add(POWER_MS);
            if let Some(sample) = power::health(now_ms) {
                let power = proto::Power {
                    adapter: sample.adapter,
                    battery_mv: sample.battery_mv,
                };
                self.server.sample(Sample {
                    at_ms: now_ms,
                    metric: Metric::Power(power),
                });
            }
        }
        if wellfed_core::deadline_passed(now_ms, self.next_rssi) {
            self.next_rssi = now_ms.wrapping_add(RSSI_MS);
            self.server.sample(Sample {
                at_ms: now_ms,
                metric: Metric::Rssi(net_status::read().rssi),
            });
        }
    }
}

impl Default for Api {
    fn default() -> Self {
        Self::new()
    }
}

/// Queue one durable event for the settings writer, stamped with the wall
/// clock when it is valid: the server's, and those from elsewhere in the
/// loop (the panel's lock, a feed's end).
pub fn record_event(body: EventBody) {
    queue_event(body, events::record);
}

/// As [`record_event`] for a feed's `Fed` event, which may take the queue's
/// reserved slot.
pub fn record_outcome(body: EventBody) {
    queue_event(body, events::record_outcome);
}

// Generic, so each push is a direct call the stack audit follows.
fn queue_event(body: EventBody, push: impl FnOnce(Option<u32>, [u8; BODY_BYTES]) -> bool) {
    let at = clock::health()
        .0
        .and_then(|clock| u32::try_from(clock.unix_ms / 1000).ok());
    let mut bytes = [0u8; BODY_BYTES];
    if wellfed_proto::encode(&body, &mut bytes).is_ok() {
        // A full queue is counted and recorded as lost.
        let _ = push(at, bytes);
    }
}

// A store write waits while a feed holds the outputs (a flash lease then
// would reset the board), longer than a client waits for its answer. So a
// client's write during a feed is refused `Busy`, which changed nothing,
// rather than left unanswered and applied after the feed.

/// A settings read, or a change queued for the settings writer: `Busy`
/// while `interlock` has the outputs claimed.
pub fn settings_request(interlock: &Interlock, change: SettingsFields) -> SettingsResult {
    if change.changes() && interlock.outputs_claimed() {
        return SettingsResult::Busy;
    }
    settings::request(change)
}

/// The Wi-Fi store, or a change queued for the writer: `Busy` while
/// `interlock` has the outputs claimed.
pub fn wifi_request(interlock: &Interlock, change: Option<Option<Credentials>>) -> WifiResult {
    match change {
        None => network::current(),
        Some(_) if interlock.outputs_claimed() => WifiResult::Busy,
        Some(credentials) => network::change(credentials),
    }
}

/// A client store change queued for the writer, and its ticket. `None`
/// while `interlock` has the outputs claimed, as when the slot is taken: a
/// revocation is refused `Busy`, and an enrollment asks again.
pub fn keys_request(interlock: &Interlock, change: KeyChange) -> Option<u32> {
    if interlock.outputs_claimed() {
        return None;
    }
    let change = match change {
        KeyChange::Enroll { key, label, role } => Change::Enroll { key, label, role },
        KeyChange::Revoke { key } => Change::Revoke { key },
    };
    (keys::request(change) == Requested::Queued).then(|| keys::ticket().max(1))
}

/// The rest of the firmware, as the server sees it.
struct Feeder<'a, F> {
    boot: Boot,
    locked: Option<bool>,
    feeds: &'a mut F,
}

impl<F: Feeds> Device for Feeder<'_, F> {
    fn random(&mut self, out: &mut [u8; 32]) -> bool {
        rng::fill(out)
    }

    fn build(&self) -> &'static str {
        startup::BUILD_ID
    }

    fn status(&mut self, now_ms: u32) -> Status<'static> {
        let wifi = net_status::read();
        let settings_generation = settings::current().map_or(0, |view| view.generation);
        let wifi_generation = match network::current() {
            WifiResult::Current { view, .. } => view.generation,
            _ => 0,
        };
        Status {
            build: startup::BUILD_ID,
            bank: self.boot.bank,
            uptime_ms: now_ms,
            boot_count: self.boot.boot_count,
            reset_flags: self.boot.reset_flags,
            faults: 0,
            rssi: wifi.rssi,
            wifi: match network::source() {
                WifiSource::Build => proto::WifiSource::Build,
                WifiSource::Stored => proto::WifiSource::Stored,
                WifiSource::Fallback => proto::WifiSource::Fallback,
            },
            link_losses: wifi.link_losses,
            dns_losses: wifi.dns_losses,
            clock_ms: clock::health().0.map(|clock| clock.unix_ms),
            power: power::health(now_ms).map(|sample| proto::Power {
                adapter: sample.adapter,
                battery_mv: sample.battery_mv,
            }),
            locked: self.locked,
            settings_generation,
            wifi_generation,
            last_event: events::last_seq(),
            sessions: 0,
        }
    }

    fn settings(&mut self, change: SettingsFields) -> SettingsResult {
        settings_request(&INTERLOCK, change)
    }

    fn wifi(&mut self, change: Option<Option<Credentials>>) -> WifiResult {
        wifi_request(&INTERLOCK, change)
    }

    fn lock(&mut self, locked: bool) -> LockResult {
        panel::request(LockFields {
            locked: Some(locked),
        })
    }

    fn play(&mut self, _tune: Tune) -> Option<PlayResult> {
        // Only setup mode steps the speaker in this firmware's loop.
        None
    }

    fn time_sync(&mut self, fields: TimeFields) -> TimeResult {
        clock::sync(fields)
    }

    fn keys(&mut self, change: KeyChange) -> Option<u32> {
        keys_request(&INTERLOCK, change)
    }

    fn keys_outcome(&mut self, ticket: u32) -> Option<KeyOutcome> {
        keys::outcome(ticket).map(|result| match result {
            KeyResult::Done => KeyOutcome::Done,
            KeyResult::Full => KeyOutcome::Full,
            KeyResult::Duplicate => KeyOutcome::Duplicate,
            KeyResult::NotFound => KeyOutcome::NotFound,
            KeyResult::Unarmed
            | KeyResult::NoGenerator
            | KeyResult::NotWritable
            | KeyResult::Io => KeyOutcome::NotWritable,
        })
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::FEED
    }

    fn feed(&mut self, portions: u8, by: Source) -> Result<u32, Refusal> {
        self.feeds.submit(portions, by)
    }

    fn feed_reserved(&mut self, feed: u32) -> Option<Result<(), Refusal>> {
        self.feeds.reserved(feed)
    }

    fn cancel_feed(&mut self, feed: u32) -> bool {
        self.feeds.cancel(feed)
    }

    fn record(&mut self, _now_ms: u32, body: EventBody) {
        record_event(body);
    }

    fn event_after(&mut self, after: Option<u64>) -> EventRead {
        match events::read_result(after) {
            None => EventRead::Wait,
            Some(Read::End) => EventRead::None,
            Some(Read::Gap { from, to }) => EventRead::Gap { from, to },
            Some(Read::Record(record)) => {
                match wellfed_proto::decode_padded::<EventBody>(&record.body) {
                    Some(body) => EventRead::Event(proto::Event {
                        seq: record.seq,
                        at: record.at,
                        body,
                    }),
                    // A body this firmware cannot read (a newer one's): skip
                    // it as lost rather than stall the subscriber.
                    None => EventRead::Gap {
                        from: record.seq,
                        to: record.seq,
                    },
                }
            }
        }
    }

    fn epoch(&self) -> Option<u32> {
        events::epoch()
    }

    fn reboot(&mut self) {
        #[cfg(target_os = "none")]
        wellfed_sdk::request_reset();
    }
}

#[cfg(target_os = "none")]
fn boot_facts() -> Boot {
    Boot::read(startup::snapshot())
}

/// Host builds have no startup snapshot.
#[cfg(not(target_os = "none"))]
fn boot_facts() -> Boot {
    Boot {
        bank: 1,
        reset_flags: 0,
        boot_count: 0,
    }
}
