//! The device API in the loop. It owns the sockets and steps
//! [`wellfed_device_api`]'s server once per pass:
//! - the listener on [`PORT`] and up to [`SESSIONS`] connections, every
//!   call non-blocking, so the loop never waits on a client;
//! - the mDNS socket and the responder that advertises `_wellfed._tcp`
//!   (`wellfed_core::mdns`), once the identity is loaded and DHCP gave an
//!   address;
//! - the client table, copied from the settings writer when it changes, so
//!   a revoked key's sessions close;
//! - the time settings and the NTP exchange ([`super::time`]);
//! - the live samples (power each time the supply is sampled; Wi-Fi
//!   signal, and with `Capabilities::STATS` the heap, the loop's pass
//!   times, the gateway's round trip and the access point, every 30 s) for
//!   subscribers, and the boot's and lost records' events;
//! - the statistics ([`Keeper`]): what the pass sees goes in, and the
//!   `Stats` op reads it out; the `FeedTrace` op reads a feed's trace from
//!   the platform a page at a time.
//!
//! [`FeederDevice`] is the server's view of the rest of the feeder: the
//! platform's mailboxes and clock, the panel's lock and the feeding owner.
//! No record carries a key, a secret or a credential.

use core::fmt;

use wellfed_core::desiccant::{self, Count};
use wellfed_core::log_tail::Batch as LogBatch;
use wellfed_core::mdns::{self, Destination, Responder};
use wellfed_core::ota::BANK1_BYTES;
use wellfed_core::ota::install::{Answer, Phase};
use wellfed_core::ota::offer::{KeyStatement, key_id};
use wellfed_core::tone::Tune;
use wellfed_proto::api::{
    self as proto, BANK1_PAGE_MAX, Bank1Page, Battery, Capabilities, Desiccant, EventBody, Metric,
    PORT, Power, Readiness, Refusal, STOCK_PLAN_ENTRIES_LEN, Sample, Source, Stats, StatsSection,
    Status, StockPlanSectors, Supply, TASKS_MAX, TaskStack as WireTask, TracePage, UpdateRefusal,
    UpdateView,
};
use wellfed_proto::trace::PAGE_MAX;
use wellfed_session::noise::Keypair;

use super::Feeds;
use super::events;
use super::feeding;
use super::lock::LockBox;
use super::owner::{self, POLL_MS};
use super::platform::{Accepted, Boot, Datagram, Platform, Received, TaskCount, TaskStack};
use super::time::{self, Time};
use crate::stats::Keeper;
use crate::store::events::Read;
use crate::update as wire;
use wellfed_device_api::mailbox::{
    Calibration, CalibrationResult, ClientChange, KeyResult, LockFields, LockResult, NtpResult,
    Origin, PlansChange, PlansResult, PlayResult, Requested, SettingsFields, SettingsResult, Table,
    TimeFields, TimeResult, TimeSettings, TimeSettingsResult, TimeState, WifiResult, WifiSource,
};
use wellfed_device_api::pairing::{self, Pairing};
use wellfed_device_api::wifi::Credentials;
use wellfed_device_api::{
    self as server, Device, EventRead, KeyChange, KeyOutcome, Pass, SESSIONS, Server, UpdateAnswer,
    UpdateKeyChange, UpdateRequest,
};

/// The answer page buffer: a trace's page or a bank-1 page, whichever is
/// larger, or stock's plan entries.
pub(super) const PAGE_BYTES: usize = if PAGE_MAX > BANK1_PAGE_MAX {
    PAGE_MAX
} else {
    BANK1_PAGE_MAX
};

// Stock's plan entries borrow the same buffer.
const _: () = assert!(STOCK_PLAN_ENTRIES_LEN <= PAGE_BYTES);

/// A listener or mDNS socket that failed is opened again after this.
const RETRY_MS: u32 = 2_000;
/// Wi-Fi signal and, for subscribers with the statistics, the heap, the
/// loop, the gateway and the access point.
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
    Time(time::Record),
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
        Record::Time(record) => time::describe(record, f),
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
        R::Opened {
            slot,
            role,
            client,
            handshake_ms,
        } => write!(
            f,
            "session slot={slot} state=open role={role} client={} handshake_ms={handshake_ms}",
            wellfed_session::hex::Lower(&client)
        ),
        R::Request { slot, op, result } => {
            write!(f, "request slot={slot} op={op} result={result}")
        }
        R::Retried { slot } => write!(f, "request slot={slot} retried=1"),
        R::Ended { slot, reason } => write!(f, "session slot={slot} state=ended reason={reason}"),
        R::Pairing(event) => owner::pair(event, f),
        R::UpdateKey { slot, change, key } => write!(
            f,
            "update-key slot={slot} change={change} id={}",
            wellfed_session::hex::Lower(&key)
        ),
    }
}

/// What the loop hands the device API each pass.
pub struct Inputs<'a, F> {
    pub now_ms: u32,
    /// BLE setup mode runs: no feed starts.
    pub setup: bool,
    pub pairing: &'a mut Pairing,
    pub identity: Option<&'a Keypair>,
    /// The panel lock, once the owner has read it.
    pub locked: Option<bool>,
    pub recovery: Option<pairing::Event>,
    /// Pairing's decision on an update key's offer, this step.
    pub signer: Option<pairing::Event>,
    /// The supply this pass judged.
    pub supply: Supply,
    /// A brownout holds feeding until the adapter is seen.
    pub battery_hold: bool,
    /// A plan's claimed feed must be the next one.
    pub meal: bool,
    pub feeds: &'a mut F,
}

pub struct Serve<S> {
    server: Server,
    listener: Option<S>,
    sockets: [Option<S>; SESSIONS],
    mdns_socket: Option<S>,
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
    stats: Keeper,
    /// The task stacks a memory answer borrows its names from.
    tasks: TaskCache,
    /// The trace or bank-1 page an answer borrows its bytes from.
    page: [u8; PAGE_BYTES],
    packet: [u8; mdns::PACKET],
    reply: [u8; mdns::PACKET],
    time: Time<S>,
}

impl<S: Copy> Serve<S> {
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
            stats: Keeper::new(),
            tasks: TaskCache::new(),
            page: [0; PAGE_BYTES],
            packet: [0; mdns::PACKET],
            reply: [0; mdns::PACKET],
            time: Time::new(),
        }
    }

    /// The server, for a host test to look into.
    pub fn server(&mut self) -> &mut Server {
        &mut self.server
    }

    /// What the last NTP exchange came to.
    pub const fn ntp_last(&self) -> Option<NtpResult> {
        self.time.last()
    }

    /// The statistics, which the rest of the loop feeds.
    pub fn stats(&mut self) -> &mut Keeper {
        &mut self.stats
    }

    /// The statistics and the buffers a memory or trace answer borrows
    /// from.
    pub(super) fn stats_parts(&mut self) -> (&mut Keeper, &mut TaskCache, &mut [u8; PAGE_BYTES]) {
        (&mut self.stats, &mut self.tasks, &mut self.page)
    }

    /// This boot's facts, read once.
    pub fn boot<P: Platform<Socket = S>>(&mut self, platform: &mut P, now_ms: u32) -> Boot {
        *self.boot.get_or_insert_with(|| {
            let boot = platform.boot();
            let body = EventBody::Boot {
                bank: boot.bank,
                reset_flags: boot.reset_flags,
            };
            events::record(platform, now_ms, body);
            self.stats.boot(boot.reset_flags);
            boot
        })
    }

    /// One pass; the delay until the next.
    pub fn step<P: Platform<Socket = S>, F: Feeds>(
        &mut self,
        platform: &mut P,
        lock: &mut LockBox,
        inputs: Inputs<'_, F>,
        mut record: impl FnMut(Record),
    ) -> u32 {
        let Inputs {
            now_ms,
            setup,
            pairing,
            identity,
            locked,
            recovery,
            signer,
            supply,
            battery_hold,
            meal,
            feeds,
        } = inputs;
        let boot = self.boot(platform, now_ms);
        let lost = platform.take_lost();
        if lost > 0 {
            let count = u16::try_from(lost).unwrap_or(u16::MAX);
            events::record(platform, now_ms, EventBody::Lost { count });
        }
        self.keep_stats(platform, &*feeds, now_ms);

        let (table, seen) = self.server.clients_table();
        if let Some(generation) = platform.clients_since(seen.max(self.clients_seen), table) {
            self.clients_seen = generation;
            self.server
                .clients_changed(generation, |r| record(Record::Server(r)));
        }

        let ip = platform.ipv4();
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
            self.open_sockets(platform, now_ms, &mut record);
        }
        self.mdns(platform, now_ms);
        let clock_ms = platform.clock_ms(now_ms);
        self.time
            .step(platform, clock_ms, ip, |r| record(Record::Time(r)));
        self.accept(platform, now_ms, &mut record);
        self.receive(platform, &mut record);

        // While the outputs are claimed, not only while they move, only IK
        // session handshakes go on (until one is refused): an invite's join or a
        // recovery enrolls a key, a store write that would wait out the
        // feed, so the invite is redeemed (and the recovery code taken) only
        // once it can be written. Held for at most the claim's bound, as
        // joins are.
        let feeding = feeds.feeding() || platform.claim_in_bound(now_ms);
        // The sessions this pass opened, counted once the device lets go of
        // the statistics.
        let mut opened = [None; SESSIONS];
        let mut device = FeederDevice {
            platform: &mut *platform,
            lock,
            feeds,
            boot,
            locked,
            setup,
            now_ms,
            ntp_last: self.time.last(),
            stats: &mut self.stats,
            tasks: &mut self.tasks,
            page: &mut self.page,
            battery_hold,
            supply,
            meal,
        };
        let busy = self.server.step(
            Pass {
                now_ms,
                device: &mut device,
                pairing,
                identity,
                feeding,
                recovery,
                signer,
            },
            |r| {
                if let server::Record::Opened { handshake_ms, .. } = r
                    && let Some(free) = opened.iter_mut().find(|o| o.is_none())
                {
                    *free = Some(handshake_ms);
                }
                record(Record::Server(r));
            },
        );
        for handshake_ms in opened.into_iter().flatten() {
            self.stats.opened(handshake_ms);
        }
        self.send(platform, &mut record);
        self.samples(platform, now_ms, supply);

        if wellfed_core::deadline_passed(now_ms, self.next_heap) {
            self.next_heap = now_ms.wrapping_add(HEAP_MS);
            let (free, min) = platform.heap();
            record(Record::Stats {
                free,
                min,
                queries: self.queries,
                answers: self.answers,
                failed: self.failed,
            });
        }

        if busy { 1 } else { POLL_MS }
    }

    fn open_sockets<P: Platform<Socket = S>>(
        &mut self,
        platform: &mut P,
        now_ms: u32,
        record: &mut impl FnMut(Record),
    ) {
        if !wellfed_core::deadline_passed(now_ms, self.retry_at) {
            return;
        }
        if self.listener.is_none() {
            self.listener = platform.listen(PORT);
            record(Record::Listen {
                mdns: false,
                open: self.listener.is_some(),
                heap: platform.heap().0,
            });
        }
        if self.mdns_socket.is_none() && self.responder.is_some() {
            self.mdns_socket = platform.mdns_open();
            record(Record::Listen {
                mdns: true,
                open: self.mdns_socket.is_some(),
                heap: platform.heap().0,
            });
        }
        if self.listener.is_none() || self.mdns_socket.is_none() {
            self.retry_at = now_ms.wrapping_add(RETRY_MS);
        }
    }

    fn mdns<P: Platform<Socket = S>>(&mut self, platform: &mut P, now_ms: u32) {
        let (Some(socket), Some(responder)) = (self.mdns_socket, self.responder.as_mut()) else {
            return;
        };
        if let Some(len) = responder.announce(now_ms, &mut self.reply)
            && !platform.mdns_send(socket, &self.reply[..len], mdns::GROUP, mdns::PORT)
        {
            self.close_mdns(platform, now_ms);
            return;
        }
        for _ in 0..DATAGRAMS {
            match platform.mdns_receive(socket, &mut self.packet) {
                Datagram::None => return,
                Datagram::Error => return self.close_mdns(platform, now_ms),
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
                    let sent = platform.mdns_send(socket, answer, to, to_port);
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

    fn close_mdns<P: Platform<Socket = S>>(&mut self, platform: &mut P, now_ms: u32) {
        if let Some(socket) = self.mdns_socket.take() {
            platform.close(socket);
        }
        self.retry_at = now_ms.wrapping_add(RETRY_MS);
    }

    fn accept<P: Platform<Socket = S>>(
        &mut self,
        platform: &mut P,
        now_ms: u32,
        record: &mut impl FnMut(Record),
    ) {
        let Some(listener) = self.listener else {
            return;
        };
        match platform.accept(listener) {
            Accepted::None => {}
            Accepted::Error => {
                platform.close(listener);
                self.listener = None;
                self.retry_at = now_ms.wrapping_add(RETRY_MS);
                record(Record::Listen {
                    mdns: false,
                    open: false,
                    heap: platform.heap().0,
                });
            }
            Accepted::Socket(socket) => {
                match self.server.accept(now_ms, |r| record(Record::Server(r))) {
                    Some(slot) => self.sockets[slot] = Some(socket),
                    None => platform.close(socket),
                }
            }
        }
    }

    fn receive<P: Platform<Socket = S>>(
        &mut self,
        platform: &mut P,
        record: &mut impl FnMut(Record),
    ) {
        for slot in 0..SESSIONS {
            let Some(socket) = self.sockets[slot] else {
                continue;
            };
            let room = self.server.rx(slot);
            if room.is_empty() {
                continue;
            }
            match platform.receive(socket, room) {
                Received::Empty => {}
                Received::Data(n) => self.server.received(slot, n),
                Received::Closed => self.drop_slot(platform, slot, record),
            }
        }
    }

    fn send<P: Platform<Socket = S>>(&mut self, platform: &mut P, record: &mut impl FnMut(Record)) {
        for slot in 0..SESSIONS {
            let Some(socket) = self.sockets[slot] else {
                continue;
            };
            let pending = self.server.tx(slot);
            if !pending.is_empty() {
                match platform.send(socket, pending) {
                    Some(n) => self.server.sent(slot, n),
                    None => {
                        self.drop_slot(platform, slot, record);
                        continue;
                    }
                }
            }
            if self.server.closing(slot) {
                self.drop_slot(platform, slot, record);
            }
        }
    }

    fn drop_slot<P: Platform<Socket = S>>(
        &mut self,
        platform: &mut P,
        slot: usize,
        record: &mut impl FnMut(Record),
    ) {
        if let Some(socket) = self.sockets[slot].take() {
            platform.close(socket);
        }
        self.server.closed(slot, |r| record(Record::Server(r)));
    }

    /// What the statistics take from this pass: the link, the food beam
    /// and the battery's history.
    fn keep_stats<P: Platform<Socket = S>, F: Feeds>(
        &mut self,
        platform: &mut P,
        feeds: &F,
        now_ms: u32,
    ) {
        let stats = &mut self.stats;
        stats.link(platform.link());
        if let Some(present) = feeds.food() {
            stats.food(present);
        }
        stats.tick(now_ms);
    }

    fn samples<P: Platform<Socket = S>>(&mut self, platform: &mut P, now_ms: u32, supply: Supply) {
        if wellfed_core::deadline_passed(now_ms, self.next_power) {
            self.next_power = now_ms.wrapping_add(P::POWER_SAMPLE_MS);
            if let Some(power) = platform.power(now_ms) {
                // The history keeps only readings a pack can give.
                let battery_mv = match supply.battery {
                    Battery::Reading { mv, .. } => Some(u32::from(mv)),
                    Battery::Absent
                    | Battery::Implausible { .. }
                    | Battery::Unread
                    | Battery::Stale { .. } => None,
                };
                self.stats.power(
                    now_ms,
                    Power {
                        battery_mv,
                        ..power
                    },
                );
                self.server.sample(Sample {
                    at_ms: now_ms,
                    metric: Metric::Power(power),
                });
            }
            if platform.capabilities().contains(Capabilities::POWER) {
                self.server.sample(Sample {
                    at_ms: now_ms,
                    metric: Metric::Supply(supply),
                });
            }
        }
        if wellfed_core::deadline_passed(now_ms, self.next_rssi) {
            self.next_rssi = now_ms.wrapping_add(RSSI_MS);
            // The writer saves the totals far less often than this.
            platform.hand_counts(self.stats.counts(now_ms));
            self.server.sample(Sample {
                at_ms: now_ms,
                metric: Metric::Rssi(platform.link().rssi),
            });
            // Taken every window whether or not anyone subscribes, so each
            // covers the same 30 s.
            if platform.capabilities().contains(Capabilities::STATS) {
                let (free, min) = platform.heap();
                for metric in [
                    Metric::Heap { free, min },
                    self.stats.loop_metric(),
                    self.stats.gateway_metric(),
                    self.stats.wifi_metric(),
                ] {
                    self.server.sample(Sample {
                        at_ms: now_ms,
                        metric,
                    });
                }
            }
        }
    }
}

impl<S: Copy> Default for Serve<S> {
    fn default() -> Self {
        Self::new()
    }
}

// A store write waits while a feed holds the outputs (a flash lease then
// would reset the board), longer than a client waits for its answer. So a
// client's write during a feed is refused `Busy`, which changed nothing,
// rather than left unanswered and applied after the feed.

/// The rest of the feeder, as the server sees it.
pub struct FeederDevice<'a, P, F> {
    pub(super) platform: &'a mut P,
    pub(super) lock: &'a mut LockBox,
    pub(super) feeds: &'a mut F,
    pub(super) boot: Boot,
    pub(super) locked: Option<bool>,
    /// BLE setup mode runs.
    pub(super) setup: bool,
    /// The pass's time, which a clock sync is anchored to.
    pub(super) now_ms: u32,
    /// What the last NTP exchange came to.
    pub(super) ntp_last: Option<NtpResult>,
    pub(super) stats: &'a mut Keeper,
    pub(super) tasks: &'a mut TaskCache,
    /// The page a trace or bank-1 answer borrows its bytes from.
    pub(super) page: &'a mut [u8; PAGE_BYTES],
    /// A brownout holds feeding until the adapter is seen.
    pub(super) battery_hold: bool,
    /// The supply this pass judged.
    pub(super) supply: Supply,
    /// A plan's claimed feed must be the next one.
    pub(super) meal: bool,
}

/// How long a walk of the task stacks answers memory requests: the walk
/// pauses the scheduler, so a client asking again and again repeats none.
const TASKS_MS: u32 = 5_000;

/// The task stacks as last walked.
pub struct TaskCache {
    stacks: [TaskStack; TASKS_MAX],
    count: TaskCount,
    at: Option<u32>,
}

impl TaskCache {
    pub(super) const fn new() -> Self {
        Self {
            stacks: [TaskStack::EMPTY; TASKS_MAX],
            count: TaskCount {
                listed: 0,
                total: 0,
            },
            at: None,
        }
    }

    /// The stacks, walked again once the last walk is [`TASKS_MS`] old.
    fn read<P: Platform>(&mut self, platform: &mut P, now_ms: u32) -> (&[TaskStack], u32) {
        if self.at.is_none_or(|at| now_ms.wrapping_sub(at) >= TASKS_MS) {
            self.count = platform.task_stacks(&mut self.stacks);
            self.at = Some(now_ms);
        }
        let listed = self.count.listed.min(TASKS_MAX);
        (&self.stacks[..listed], self.count.total)
    }
}

impl<P: Platform, F: Feeds> Device for FeederDevice<'_, P, F> {
    fn random(&mut self, out: &mut [u8; 32]) -> bool {
        self.platform.random(out)
    }

    fn build(&self) -> &'static str {
        self.platform.build()
    }

    fn status(&mut self, now_ms: u32) -> Status<'static> {
        let link = self.platform.link();
        let settings = self.platform.settings();
        let settings_generation = settings.map_or(0, |view| view.generation);
        let today = self.today(now_ms);
        let desiccant = match settings.map(|view| view.desiccant(today)) {
            Some(Count::Remaining(days)) => Desiccant::Remaining { days },
            Some(Count::Unset) => Desiccant::Unset,
            Some(Count::Unknown) | None => Desiccant::Unknown,
        };
        let wifi_generation = match self.platform.wifi() {
            WifiResult::Current { view, .. } => view.generation,
            _ => 0,
        };
        Status {
            build: self.platform.build(),
            bank: self.boot.bank,
            uptime_ms: now_ms,
            boot_count: self.boot.boot_count,
            reset_flags: self.boot.reset_flags,
            faults: 0,
            rssi: link.rssi,
            wifi: match self.platform.wifi_source() {
                WifiSource::Build => proto::WifiSource::Build,
                WifiSource::Stored => proto::WifiSource::Stored,
                WifiSource::Fallback => proto::WifiSource::Fallback,
            },
            link_losses: link.link_losses,
            dns_losses: link.check_losses,
            clock_ms: self.platform.wall_ms(now_ms),
            power: self.platform.power(now_ms),
            locked: self.locked,
            settings_generation,
            wifi_generation,
            last_event: self.platform.last_event(),
            sessions: 0,
            desiccant,
        }
    }

    fn today(&mut self, now_ms: u32) -> Option<i32> {
        let reading = self.platform.clock_state(now_ms).reading?;
        desiccant::today(&reading)
    }

    fn settings(&mut self, change: SettingsFields) -> SettingsResult {
        if change.changes() && self.platform.outputs_claimed() {
            return SettingsResult::Busy;
        }
        self.platform.settings_request(change)
    }

    fn wifi(&mut self, change: Option<Option<Credentials>>) -> WifiResult {
        match change {
            None => self.platform.wifi(),
            Some(_) if self.platform.outputs_claimed() => WifiResult::Busy,
            Some(credentials) => self.platform.wifi_change(credentials),
        }
    }

    fn lock(&mut self, locked: bool) -> LockResult {
        self.lock.request(LockFields {
            locked: Some(locked),
        })
    }

    fn play(&mut self, _tune: Tune) -> Option<PlayResult> {
        // Only setup mode steps the speaker in the loop.
        None
    }

    fn time_sync(&mut self, fields: TimeFields, origin: Origin) -> TimeResult {
        TimeResult::from_sync(self.platform.time_sync(self.now_ms, fields.sync(), origin))
    }

    fn time_settings(&mut self, change: Option<TimeSettings>) -> TimeSettingsResult {
        match change {
            None => self.platform.time_settings(),
            Some(_) if self.platform.outputs_claimed() => TimeSettingsResult::Busy,
            Some(settings) => self.platform.time_settings_change(settings),
        }
    }

    fn calibration(&mut self, change: Option<Calibration>) -> CalibrationResult {
        match change {
            None => self.platform.calibration(),
            Some(_) if self.platform.outputs_claimed() => CalibrationResult::Busy,
            Some(calibration) => self.platform.calibration_change(calibration),
        }
    }

    /// A client's table is saved at the wall time now, so a plan it adds
    /// or moves never runs an occurrence already past.
    fn plans(&mut self, change: Option<Table>) -> PlansResult {
        match change {
            None => self.platform.plans(),
            Some(_) if self.platform.outputs_claimed() => PlansResult::Busy,
            Some(table) => {
                let at_s = self
                    .platform
                    .wall_ms(self.now_ms)
                    .and_then(|ms| u32::try_from(ms / 1000).ok());
                self.platform
                    .plans_change(PlansChange::Replace { table, at_s })
            }
        }
    }

    fn phase(&self) -> Option<proto::Phase> {
        self.feeds.marks().map(|marks| marks.phase)
    }

    fn time_state(&mut self, now_ms: u32) -> Option<TimeState> {
        Some(TimeState {
            clock: self.platform.clock_state(now_ms),
            now_ms,
            ntp_last: self.ntp_last,
        })
    }

    /// `None` while the outputs are claimed, as when the slot is taken: a
    /// revocation is refused `Busy`, and an enrollment asks again.
    fn keys(&mut self, change: KeyChange) -> Option<u32> {
        if self.platform.outputs_claimed() {
            return None;
        }
        let change = match change {
            KeyChange::Enroll { key, label, role } => ClientChange::Enroll { key, label, role },
            KeyChange::Revoke { key } => ClientChange::Revoke { key },
        };
        (self.platform.keys_request(change) == Requested::Queued)
            .then(|| self.platform.keys_ticket().max(1))
    }

    fn keys_outcome(&mut self, ticket: u32) -> Option<KeyOutcome> {
        self.platform
            .keys_outcome(ticket)
            .map(|result| match result {
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
        self.platform.capabilities()
    }

    fn readiness(&mut self) -> Readiness {
        let ready = feeding::readiness(
            &*self.feeds,
            self.setup,
            self.platform.installing(),
            self.battery_hold,
        );
        feeding::held(ready, self.meal)
    }

    fn supply(&mut self) -> Option<Supply> {
        Some(self.supply)
    }

    fn feed(&mut self, portions: u8, by: Source) -> Result<u32, Refusal> {
        let ready = self.readiness();
        feeding::queue(self.feeds, ready, portions, by)
    }

    fn feed_reserved(&mut self, feed: u32) -> Option<Result<(), Refusal>> {
        self.feeds.reserved(feed)
    }

    fn cancel_feed(&mut self, feed: u32) -> bool {
        self.feeds.cancel(feed)
    }

    fn record(&mut self, now_ms: u32, body: EventBody) {
        events::record(self.platform, now_ms, body);
    }

    fn event_after(&mut self, after: Option<u64>) -> EventRead {
        match self.platform.event_after(after) {
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
                    // A body this build cannot read (a newer one's): skip it
                    // as lost rather than stall the subscriber.
                    None => EventRead::Gap {
                        from: record.seq,
                        to: record.seq,
                    },
                }
            }
        }
    }

    fn epoch(&self) -> Option<u32> {
        self.platform.epoch()
    }

    fn reboot(&mut self) {
        // The writer's restart save takes this boot's counts as they are now.
        self.platform.hand_counts(self.stats.counts(self.now_ms));
        self.platform.reset();
    }

    fn stats(&mut self, section: StatsSection, now_ms: u32) -> Stats<'_> {
        match section {
            StatsSection::Counters => {
                let stored = self.platform.totals();
                Stats::Counters(self.stats.counters(now_ms, stored.as_ref()))
            }
            StatsSection::Memory => {
                let (heap_free, heap_min) = self.platform.heap();
                let (stacks, tasks_total) = self.tasks.read(&mut *self.platform, now_ms);
                let mut tasks = [None; TASKS_MAX];
                for (out, task) in tasks.iter_mut().zip(stacks) {
                    *out = Some(WireTask {
                        name: task.name(),
                        free_min: task.free_min,
                    });
                }
                Stats::Memory(proto::Memory {
                    heap_free,
                    heap_min,
                    tasks,
                    tasks_total,
                })
            }
            StatsSection::Flash => {
                let fill = self.platform.events_fill();
                let stored = self.platform.totals();
                Stats::Flash(Keeper::flash(
                    self.platform.erases(),
                    stored.as_ref(),
                    fill.kept,
                    fill.capacity,
                ))
            }
            StatsSection::Power => Stats::Power(self.stats.power_stats()),
            StatsSection::Food => Stats::Food(self.stats.food_stats()),
            StatsSection::Feeds => Stats::Feeds(self.stats.feed_stats(self.platform.traces_fill())),
            StatsSection::Portions => {
                let portion_mg = match self.platform.calibration() {
                    CalibrationResult::Current { view, .. } => view.calibration.portion_mg,
                    _ => None,
                };
                Stats::Portions(self.stats.portions_stats(self.feeds.marks(), portion_mg))
            }
        }
    }

    fn feed_trace(&mut self, feed: u32, offset: u16) -> Result<TracePage<'_>, Refusal> {
        let (page, _) = self
            .page
            .split_first_chunk_mut::<PAGE_MAX>()
            .ok_or(Refusal::Busy)?;
        let read = self.platform.trace_page(feed, offset, page)?;
        Ok(TracePage {
            summary: read.summary,
            stored: read.stored,
            offset,
            total: read.total,
            bytes: self.page.get(..read.len).unwrap_or(&[]),
        })
    }

    /// Bank 1 as the flash holds it. Only the bank-0 role reads it: there
    /// it may still hold stock, the feeder's only copy until a promotion
    /// overwrites it, while on bank 1 it is the running build. An install
    /// rewrites it under the read, so none is served while one holds the
    /// flash. Nothing here vouches for the bytes: the installer checks the
    /// copy against the pinned stock image.
    fn bank1_page(&mut self, offset: u32) -> Result<Bank1Page<'_>, Refusal> {
        if self.boot.bank != 0 {
            return Err(Refusal::Unavailable);
        }
        if self.platform.installing() {
            return Err(Refusal::Busy);
        }

        let left = BANK1_BYTES
            .checked_sub(offset)
            .filter(|&left| left > 0)
            .ok_or(Refusal::Invalid)?;
        let len = usize::try_from(left).map_or(BANK1_PAGE_MAX, |left| left.min(BANK1_PAGE_MAX));
        let out = self.page.get_mut(..len).ok_or(Refusal::Busy)?;
        if !self.platform.bank1_read(offset, out) {
            return Err(Refusal::Unavailable);
        }
        Ok(Bank1Page {
            offset,
            total: BANK1_BYTES,
            bytes: out,
        })
    }

    /// Stock's plan sectors, served where bank 1 is and for the same
    /// reason: the bank-0 role still has stock's flash as stock left it,
    /// for the installer to import before a promotion.
    fn stock_plans(&mut self) -> Result<StockPlanSectors<'_>, Refusal> {
        if self.boot.bank != 0 {
            return Err(Refusal::Unavailable);
        }
        if self.platform.installing() {
            return Err(Refusal::Busy);
        }

        let (entries, _) = self
            .page
            .split_first_chunk_mut::<STOCK_PLAN_ENTRIES_LEN>()
            .ok_or(Refusal::Busy)?;
        let count = self
            .platform
            .stock_plans_read(entries)
            .ok_or(Refusal::Unavailable)?;
        Ok(StockPlanSectors {
            count,
            entries: &entries[..],
        })
    }

    /// Stock refuses an install on its batteries: one that browns out
    /// mid-write costs nothing (the marker comes last), but the radio goes
    /// off within seconds and the install would time out. A cancel and a
    /// key statement still go through.
    fn update(&mut self, request: UpdateRequest<'_>) -> Result<u32, Refusal> {
        if matches!(request, UpdateRequest::Offer { .. }) && self.supply.adapter == Some(false) {
            return Err(Refusal::Update(UpdateRefusal::Battery));
        }
        let ticket = match request {
            UpdateRequest::Offer {
                manifest,
                downgrade,
                device,
            } => self.platform.update_offer(manifest, downgrade, device),
            UpdateRequest::Statement { statement } => self.platform.update_statement(statement),
            UpdateRequest::Cancel => self.platform.update_cancel(),
        };
        ticket.map_err(wire::refusal)
    }

    fn update_outcome(&mut self, ticket: u32) -> Option<Result<UpdateAnswer, Refusal>> {
        self.platform
            .update_outcome(ticket)
            .map(|answer| match answer {
                Ok(Answer::Progress(progress)) => Ok(UpdateAnswer::View(wire::view(&progress))),
                Ok(Answer::Key(KeyStatement::Revoke(id))) => Ok(UpdateAnswer::Key {
                    change: UpdateKeyChange::Retired,
                    id,
                }),
                Ok(Answer::Key(KeyStatement::Add(key))) => Ok(UpdateAnswer::Key {
                    change: UpdateKeyChange::Added,
                    id: key_id(&key),
                }),
                Err(refused) => Err(wire::refusal(refused)),
            })
    }

    fn update_chunk(&mut self, offset: u32, data: &[u8]) -> Result<UpdateView, Refusal> {
        let progress = self.platform.update_progress().ok_or(Refusal::Busy)?;
        match progress.phase {
            Phase::Receiving => {}
            // The last chunk's answer may have been lost: the state says so.
            Phase::Activated => return Ok(wire::view(&progress)),
            Phase::Idle | Phase::Checking | Phase::GivingWay => {
                return Err(Refusal::Update(UpdateRefusal::NotInstalling));
            }
        }
        self.platform
            .update_chunk(offset, data)
            .map_err(wire::refusal)?;
        let progress = self.platform.update_progress().ok_or(Refusal::Busy)?;
        Ok(wire::view(&progress))
    }

    fn update_view(&mut self) -> Option<UpdateView> {
        self.platform
            .update_progress()
            .map(|progress| wire::view(&progress))
    }

    fn installing(&self) -> bool {
        self.platform.installing()
    }

    fn feed_active(&self) -> bool {
        self.feeds.busy()
    }

    fn log_after(&mut self, after: Option<u32>, text: &mut [u8]) -> Option<LogBatch> {
        self.platform.log_after(after, text)
    }
}
