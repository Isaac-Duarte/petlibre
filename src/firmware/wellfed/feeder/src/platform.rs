//! [`Plaf103`]: the product loop's [`Platform`] on the PLAF103, over the
//! SDK's drivers and the feeder runtime's mailboxes. Everything here is a
//! direct call into one of them, wired to this board: the four panel
//! indicators, the BLE peripheral and the speaker that setup mode steps, the
//! device API's lwIP sockets, the settings writer's mailboxes, the wall
//! clock, the outputs' interlock and the update stage's mailbox.
//!
//! Host builds run it against the SDK fake, for the loop's tests and the
//! device conformance suite.

use core::fmt;

use wellfed_core::buttons::Gesture;
use wellfed_core::clock::wall::{Fix, Jump, Origin, State as ClockState, Sync, SyncError};
use wellfed_core::clock::zone::Rule;
use wellfed_core::indicator::Light;
use wellfed_core::log_tail::Batch as LogBatch;
use wellfed_core::ota::install::{Answer, Progress, Refused};
use wellfed_core::store::slot::SlotIo;
use wellfed_core::supply::Reading;
use wellfed_core::tone::Tune;
use wellfed_device_api::clients::{CLIENTS, Client};
use wellfed_device_api::mailbox::{
    Calibration, CalibrationResult, ClientChange, KeyResult, PlansChange, PlansResult, PlayResult,
    Requested, SettingsFields, SettingsResult, SettingsView, TimeSettings, TimeSettingsResult,
    WifiResult, WifiSource,
};
use wellfed_device_api::wifi::Credentials;
use wellfed_feeder::feeder::Platform;
use wellfed_feeder::feeder::platform::{
    Accepted, BleDrop, BleLink, BleStep, Boot, Check, Datagram, DriverRecord, DriverStep,
    EventSlot, EventsFill, Link, Received, TaskCount, TaskStack, TraceRead, TracesFill,
};
use wellfed_feeder::feeder::supply::Radio;
use wellfed_feeder::stats::Totals;
use wellfed_feeder::store;
use wellfed_feeder::store::events::{BODY_BYTES, Read};
use wellfed_feeder_runtime::{calibration, events, keys, network, plans, stats, time, traces};
use wellfed_proto::api::{
    Capabilities, Counts, Erases, FLASH_SECTORS, Power, Refusal, STOCK_PLAN_ENTRIES_LEN, TASKS_MAX,
};
use wellfed_proto::trace::PAGE_MAX;
use wellfed_sdk::hal::audio::{self, Speaker};
use wellfed_sdk::hal::ble::{self, BleSetup, DropReason};
use wellfed_sdk::hal::interlock::INTERLOCK;
use wellfed_sdk::hal::leds::{self, Decision, Led};
use wellfed_sdk::hal::ota::mailbox::{self, Request};
use wellfed_sdk::hal::sdk::{self, ApiRecv};
use wellfed_sdk::hal::{buzzer, clock, link, net_status, power, rng, settings, startup};
use wellfed_sdk::storage::{TRACES_BASE, TRACES_UNITS};

/// What the board's own drivers record: `PLBLE1` for setup mode's link,
/// `PLAUD1` for its speaker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Driver {
    Ble(ble::Record),
    Audio(audio::Record),
}

impl DriverRecord for Driver {
    fn tag(&self) -> &'static str {
        match self {
            Self::Ble(_) => "PLBLE1",
            Self::Audio(_) => "PLAUD1",
        }
    }
}

impl fmt::Display for Driver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Ble(record) => ble::describe(record, f),
            Self::Audio(record) => audio::describe(record, f),
        }
    }
}

/// The loop's drivers that hold state: setup mode's BLE link and speaker,
/// which run only in setup mode. The rest of the board is the SDK's.
pub struct Plaf103 {
    ble: BleSetup,
    speaker: Speaker,
}

impl Plaf103 {
    /// Const, so the firmware builds it in its static.
    pub const fn new() -> Self {
        Self {
            ble: BleSetup::new(),
            speaker: Speaker::quiet(),
        }
    }
}

impl Default for Plaf103 {
    fn default() -> Self {
        Self::new()
    }
}

/// The trace ring as the loop reads it: mapped flash, which takes no
/// lease. The settings writer alone appends to it, and a lease holds every
/// other task off the flash, so a read never lands inside a program or an
/// erase; one between an append's two programs fails its checksum.
#[derive(Clone, Copy, Debug)]
struct TraceReads;

impl SlotIo for TraceReads {
    fn unit_bytes(&self) -> u32 {
        sdk::SECTOR_BYTES
    }

    fn read(&self, slot: u32, offset: u32, bytes: &mut [u8]) -> bool {
        slot < TRACES_UNITS && sdk::storage_read(TRACES_BASE + slot, offset, bytes)
    }

    fn program(&self, _: u32, _: u32, _: &[u8]) -> bool {
        false
    }

    fn erase(&self, _: u32) -> bool {
        false
    }

    fn writable(&self) -> bool {
        false
    }
}

/// The pad behind each panel light: PA0 Wi-Fi, PA2 low food, PA4 lock and
/// PB22 warning, each lit HIGH (physical, on the research unit).
const fn led(light: Light) -> Led {
    match light {
        Light::Connection => Led::Connection,
        Light::Food => Led::Food,
        Light::Lock => Led::Lock,
        Light::Fault => Led::Fault,
    }
}

impl Platform for Plaf103 {
    type Socket = i32;
    type Record = Driver;

    const INDICATOR_SETTLE_MS: u32 = leds::SETTLE_MS;
    const POWER_SAMPLE_MS: u32 = power::SAMPLE_MS;

    fn build(&self) -> &'static str {
        startup::BUILD_ID
    }

    fn boot(&self) -> Boot {
        boot_facts()
    }

    /// The log ships in release images: an owner's feeder has no UART
    /// wired, so the tail is the only way to see what it logged, and
    /// keeping the bit costs only the tail's RAM ([device API]).
    ///
    /// [device API]: ../../../../../docs/firmware/device-api.md#debug-log
    fn capabilities(&self) -> Capabilities {
        Capabilities::FEED
            .union(Capabilities::UPDATE)
            .union(Capabilities::TIME)
            .union(Capabilities::LOG)
            .union(Capabilities::STATS)
            .union(Capabilities::READINESS)
            .union(Capabilities::TRACES)
            .union(Capabilities::CALIBRATION)
            .union(Capabilities::POWER)
            .union(Capabilities::BACKUP)
            .union(Capabilities::PLANS)
            .union(Capabilities::DESICCANT)
    }

    fn reset(&mut self) {
        #[cfg(target_os = "none")]
        wellfed_sdk::request_reset();
    }

    fn random(&mut self, out: &mut [u8; 32]) -> bool {
        rng::fill(out)
    }

    fn identity_since(&mut self, seen: u32) -> Option<(u32, [u8; 32])> {
        keys::identity_since(seen)
    }

    fn gesture(&mut self) -> Option<Gesture> {
        wellfed_sdk::buttons::next()
    }

    fn indicator(&mut self, light: Light, on: bool) -> bool {
        leds::show(led(light), on)
    }

    fn indicator_check(&mut self, light: Light, on: bool) -> Check {
        match leds::shown(led(light), on) {
            // Port A has no input readback: a level whose latch, pad and
            // directions agree is accepted; stopped outputs are not required.
            Decision::Verified | Decision::OutputOnly => Check::Held,
            Decision::InputMismatch => Check::InputMismatch,
            Decision::Abort => Check::Abort,
        }
    }

    fn indicators_off(&mut self) {
        let _ = leds::all_low();
    }

    fn power(&mut self, now_ms: u32) -> Option<Power> {
        power::health(now_ms).map(|sample| Power {
            adapter: sample.adapter,
            battery_mv: sample.battery_mv,
        })
    }

    fn supply(&mut self, now_ms: u32) -> Reading {
        power::supply(now_ms)
    }

    fn radio(&mut self, radio: Radio) {
        link::set_radio(radio);
    }

    fn link(&mut self) -> Link {
        let status = net_status::read();
        Link {
            rssi: status.rssi,
            link_losses: status.link_losses,
            check_losses: status.check_losses,
            dhcp_renewals: status.dhcp_renewals,
            gateway_answers: status.gateway_answers,
            gateway_ms: status.gateway_ms,
            channel: status.channel,
            ap_changes: status.ap_changes,
        }
    }

    fn ipv4(&mut self) -> Option<u32> {
        sdk::ipv4()
    }

    fn gateway(&mut self) -> Option<u32> {
        sdk::gateway()
    }

    fn settings(&mut self) -> Option<SettingsView> {
        settings::current()
    }

    fn settings_request(&mut self, change: SettingsFields) -> SettingsResult {
        settings::request(change)
    }

    fn wifi(&mut self) -> WifiResult {
        network::current()
    }

    fn wifi_change(&mut self, credentials: Option<Credentials>) -> WifiResult {
        network::change(credentials)
    }

    fn wifi_saving(&self) -> bool {
        network::saving()
    }

    fn wifi_source(&self) -> WifiSource {
        network::source()
    }

    fn keys_request(&mut self, change: ClientChange) -> Requested {
        keys::request(change)
    }

    fn keys_ticket(&self) -> u32 {
        keys::ticket()
    }

    fn keys_outcome(&self, ticket: u32) -> Option<KeyResult> {
        keys::outcome(ticket)
    }

    fn clients_since(&mut self, seen: u32, table: &mut [Option<Client>; CLIENTS]) -> Option<u32> {
        keys::clients_since(seen, table)
    }

    fn time_settings(&mut self) -> TimeSettingsResult {
        time::current()
    }

    fn time_settings_change(&mut self, settings: TimeSettings) -> TimeSettingsResult {
        time::change(settings)
    }

    fn calibration(&mut self) -> CalibrationResult {
        calibration::current()
    }

    fn calibration_change(&mut self, change: Calibration) -> CalibrationResult {
        calibration::change(change)
    }

    fn plans(&mut self) -> PlansResult {
        plans::current()
    }

    fn plans_change(&mut self, change: PlansChange) -> PlansResult {
        plans::change(change)
    }

    fn queue_event(&mut self, at: Option<u32>, body: [u8; BODY_BYTES], slot: EventSlot) -> bool {
        match slot {
            EventSlot::Shared => events::record(at, body),
            EventSlot::Outcome => events::record_outcome(at, body),
        }
    }

    fn take_lost(&mut self) -> u32 {
        events::take_lost()
    }

    fn event_after(&mut self, after: Option<u64>) -> Option<Read> {
        events::read_result(after)
    }

    fn epoch(&self) -> Option<u32> {
        events::epoch()
    }

    fn last_event(&self) -> Option<u64> {
        events::last_seq()
    }

    fn log_after(&self, after: Option<u32>, text: &mut [u8]) -> Option<LogBatch> {
        wellfed_sdk::log_tail(after, text)
    }

    /// The SysTimer's count, which flash erases do not stop.
    fn clock_ms(&mut self, _now_ms: u32) -> u32 {
        clock::now_ms()
    }

    /// The clock reads its own count.
    fn wall_ms(&mut self, _now_ms: u32) -> Option<u64> {
        clock::state().reading.map(|reading| reading.unix_ms)
    }

    fn time_sync(&mut self, _now_ms: u32, sync: Sync, origin: Origin) -> Result<Jump, SyncError> {
        clock::sync(sync, origin)
    }

    /// `now_ms` is the clock's count the answer was read at.
    fn time_fix(&mut self, now_ms: u32, fix: Fix) -> Result<Jump, SyncError> {
        clock::fix(fix, now_ms)
    }

    fn clock_state(&mut self, _now_ms: u32) -> ClockState {
        clock::state()
    }

    fn offset_at(&self, unix_ms: u64) -> Option<i32> {
        clock::offset_at(unix_ms)
    }

    fn set_zone_rule(&mut self, rule: Option<Rule>) {
        clock::set_rule(rule);
    }

    fn outputs_claimed(&self) -> bool {
        INTERLOCK.outputs_claimed()
    }

    fn claim_in_bound(&self, now_ms: u32) -> bool {
        INTERLOCK.claim_in_bound(now_ms)
    }

    fn update_offer(
        &mut self,
        manifest: &[u8],
        downgrade: Option<&[u8]>,
        device: [u8; 32],
    ) -> Result<u32, Refused> {
        Request::offer(manifest, downgrade, device).and_then(mailbox::request)
    }

    fn update_statement(&mut self, statement: &[u8]) -> Result<u32, Refused> {
        Request::statement(statement).and_then(mailbox::request)
    }

    fn update_cancel(&mut self) -> Result<u32, Refused> {
        mailbox::request(Request::Cancel)
    }

    fn update_outcome(&mut self, ticket: u32) -> Option<Result<Answer, Refused>> {
        mailbox::outcome(ticket)
    }

    fn update_chunk(&mut self, offset: u32, data: &[u8]) -> Result<(), Refused> {
        mailbox::chunk(offset, data)
    }

    fn update_progress(&mut self) -> Option<Progress> {
        mailbox::progress()
    }

    fn installing(&self) -> bool {
        mailbox::installing()
    }

    /// Through the mapped flash window, a sector at a time as the SDK
    /// takes it; it refuses every read unless the flash MMU maps bank 0,
    /// so only the recovery role reads.
    fn bank1_read(&mut self, offset: u32, out: &mut [u8]) -> bool {
        let mut at = offset;
        let mut rest = out;
        while !rest.is_empty() {
            let within = at % sdk::SECTOR_BYTES;
            let room = usize::try_from(sdk::SECTOR_BYTES - within).unwrap_or(usize::MAX);
            let (piece, after) = rest.split_at_mut(room.min(rest.len()));
            if !sdk::bank1_read(at / sdk::SECTOR_BYTES, within, piece) {
                return false;
            }
            let Ok(len) = u32::try_from(piece.len()) else {
                return false;
            };
            at += len;
            rest = after;
        }
        true
    }

    /// Stock's plan sectors through the mapped flash: two fixed spans
    /// outside both banks and the storage region, read and never written.
    fn stock_plans_read(&mut self, entries: &mut [u8; STOCK_PLAN_ENTRIES_LEN]) -> Option<u8> {
        Some(sdk::stock_plans_read(entries))
    }

    fn listen(&mut self, port: u16) -> Option<i32> {
        sdk::api_listen(port)
    }

    fn accept(&mut self, listener: i32) -> Accepted<i32> {
        match sdk::api_accept(listener) {
            sdk::Accepted::Socket(socket) => Accepted::Socket(socket),
            sdk::Accepted::None => Accepted::None,
            sdk::Accepted::Error => Accepted::Error,
        }
    }

    fn receive(&mut self, socket: i32, into: &mut [u8]) -> Received {
        match sdk::api_recv(socket, into) {
            ApiRecv::Data(n) => Received::Data(n),
            ApiRecv::Empty => Received::Empty,
            ApiRecv::Closed => Received::Closed,
        }
    }

    fn send(&mut self, socket: i32, bytes: &[u8]) -> Option<usize> {
        sdk::api_send(socket, bytes)
    }

    fn close(&mut self, socket: i32) {
        sdk::tcp_close(socket);
    }

    fn mdns_open(&mut self) -> Option<i32> {
        sdk::mdns_open()
    }

    fn mdns_send(&mut self, socket: i32, bytes: &[u8], ip: u32, port: u16) -> bool {
        sdk::mdns_send(socket, bytes, ip, port)
    }

    fn mdns_receive(&mut self, socket: i32, into: &mut [u8]) -> Datagram {
        datagram(sdk::mdns_recv(socket, into))
    }

    fn ntp_open(&mut self) -> Option<i32> {
        sdk::udp_open()
    }

    fn ntp_send(&mut self, socket: i32, bytes: &[u8], ip: u32, port: u16) -> bool {
        sdk::mdns_send(socket, bytes, ip, port)
    }

    fn ntp_receive(&mut self, socket: i32, into: &mut [u8]) -> Datagram {
        datagram(sdk::mdns_recv(socket, into))
    }

    fn heap(&self) -> (u32, u32) {
        (sdk::heap_free(), sdk::heap_min_free())
    }

    fn task_stacks(&mut self, out: &mut [TaskStack; TASKS_MAX]) -> TaskCount {
        let mut raw = [sdk::TaskStack::default(); TASKS_MAX];
        let count = sdk::task_stacks(&mut raw);
        for (out, task) in out.iter_mut().zip(&raw[..count.listed]) {
            *out = TaskStack::new(task.name(), task.free_min);
        }
        TaskCount {
            listed: count.listed,
            total: count.total,
        }
    }

    fn erases(&self) -> Erases {
        let mut storage = [0; FLASH_SECTORS];
        for (sector, count) in (0u32..).zip(storage.iter_mut()) {
            *count = sdk::storage_erase_count(sector);
        }
        Erases {
            storage,
            update: sdk::bank1_erase_count(),
        }
    }

    fn events_fill(&self) -> EventsFill {
        stats::fill()
    }

    fn traces_fill(&self) -> TracesFill {
        traces::fill()
    }

    /// The latest feed's trace from RAM, any other from the storage ring.
    fn trace_page(
        &mut self,
        feed: u32,
        offset: u16,
        out: &mut [u8; PAGE_MAX],
    ) -> Result<TraceRead, Refusal> {
        if let Some(read) = traces::page(feed, offset, out) {
            return read;
        }
        store::traces::page(&TraceReads, TRACES_UNITS as usize, feed, offset, out)
    }

    fn totals(&self) -> Option<Totals> {
        stats::totals()
    }

    fn hand_counts(&mut self, counts: Counts) {
        stats::hand_counts(counts);
    }

    fn ble_restart(&mut self) {
        self.ble = BleSetup::new();
    }

    fn ble_step(&mut self, advertise: bool) -> BleStep<Driver> {
        let step = self.ble.step(advertise);
        BleStep {
            delay_ms: step.delay_ms,
            record: step.record.map(Driver::Ble),
            link: step.link.map(|link| match link {
                ble::Link::Opened => BleLink::Opened,
                ble::Link::Message => BleLink::Message,
                ble::Link::Closed => BleLink::Closed,
            }),
        }
    }

    fn ble_message(&self) -> Option<&[u8]> {
        self.ble.message()
    }

    fn ble_consume(&mut self) {
        self.ble.consume();
    }

    fn ble_send(&mut self, body: &[u8]) -> bool {
        self.ble.send(body).is_ok()
    }

    fn ble_close(&mut self, reason: BleDrop) -> Option<Driver> {
        let reason = match reason {
            BleDrop::Done => DropReason::Done,
            BleDrop::Protocol => DropReason::Protocol,
        };
        Some(Driver::Ble(self.ble.close(reason)))
    }

    fn speaker_restart(&mut self) {
        self.speaker = Speaker::quiet();
    }

    fn speaker_idle(&self) -> bool {
        self.speaker.is_idle()
    }

    fn play(&mut self, tune: Tune) -> PlayResult {
        audio::request(tune)
    }

    fn speaker_step(&mut self) -> DriverStep<Driver> {
        let step = self.speaker.step();
        DriverStep {
            delay_ms: step.delay_ms,
            record: step.record.map(Driver::Audio),
        }
    }

    // The `buzzer` variant's probe owns PA25 alone, so its records describe
    // only what it drove: the product's beep stays silent there.
    fn buzzer_enable(&mut self) {
        if !cfg!(feature = "buzzer") {
            // The latch is written LOW before the direction, so enabling
            // never pulses the line.
            buzzer::enable();
        }
    }

    fn buzzer(&mut self, on: bool) {
        if !cfg!(feature = "buzzer") {
            buzzer::set(on);
        }
    }
}

#[cfg(target_os = "none")]
fn boot_facts() -> Boot {
    let boot = wellfed_sdk::hal::health::Boot::read(startup::snapshot());
    Boot {
        bank: boot.bank,
        reset_flags: boot.reset_flags,
        boot_count: boot.boot_count,
    }
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

fn datagram(received: sdk::Datagram) -> Datagram {
    match received {
        sdk::Datagram::Got { len, ip, port } => Datagram::Got { len, ip, port },
        sdk::Datagram::None => Datagram::None,
        sdk::Datagram::Error => Datagram::Error,
    }
}
