//! [`Plaf103`]: the product loop's [`Platform`] on the PLAF103, over the
//! SDK's drivers and the feeder runtime's mailboxes. Everything here is a
//! direct call into one of them, wired to this board: the lock indicator on
//! PA4, the BLE peripheral and the speaker that setup mode steps, the
//! device API's lwIP sockets, the settings writer's mailboxes, the wall
//! clock, the outputs' interlock and the update stage's mailbox.
//!
//! Host builds run it against the SDK fake, for the loop's tests and the
//! device conformance suite.

use core::fmt;

use wellfed_core::buttons::Gesture;
use wellfed_core::clock::wall::{Jump, Sync, SyncError};
use wellfed_core::ota::install::{Answer, Progress, Refused};
use wellfed_core::tone::Tune;
use wellfed_device_api::clients::{CLIENTS, Client};
use wellfed_device_api::mailbox::{
    ClientChange, KeyResult, PlayResult, Requested, SettingsFields, SettingsResult, SettingsView,
    WifiResult, WifiSource,
};
use wellfed_device_api::wifi::Credentials;
use wellfed_feeder::feeder::Platform;
use wellfed_feeder::feeder::platform::{
    Accepted, BleDrop, BleLink, BleStep, Boot, Check, Datagram, DriverRecord, DriverStep,
    EventSlot, Link, Received,
};
use wellfed_feeder::store::events::{BODY_BYTES, Read};
use wellfed_feeder_runtime::{events, keys, network};
use wellfed_proto::api::{Capabilities, Power};
use wellfed_sdk::hal::audio::{self, Speaker};
use wellfed_sdk::hal::ble::{self, BleSetup, DropReason};
use wellfed_sdk::hal::interlock::INTERLOCK;
use wellfed_sdk::hal::leds::{self, Decision, Led};
use wellfed_sdk::hal::ota::mailbox::{self, Request};
use wellfed_sdk::hal::sdk::{self, ApiRecv};
use wellfed_sdk::hal::{clock, net_status, power, rng, settings, startup};

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

impl Platform for Plaf103 {
    type Socket = i32;
    type Record = Driver;

    const LOCK_INDICATOR: &'static str = Led::Lock.name();
    const INDICATOR_SETTLE_MS: u32 = leds::SETTLE_MS;
    const POWER_SAMPLE_MS: u32 = power::SAMPLE_MS;

    fn build(&self) -> &'static str {
        startup::BUILD_ID
    }

    fn boot(&self) -> Boot {
        boot_facts()
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::FEED.union(Capabilities::UPDATE)
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

    fn indicator(&mut self, on: bool) -> bool {
        leds::request(Led::Lock, on)
    }

    fn indicator_check(&mut self, on: bool) -> Check {
        match leds::observe(Led::Lock).decision(on) {
            // Port A has no input readback: a level whose latch, pad,
            // directions and stopped outputs agree is accepted.
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

    fn link(&mut self) -> Link {
        let status = net_status::read();
        Link {
            rssi: status.rssi,
            link_losses: status.link_losses,
            check_losses: status.check_losses,
        }
    }

    fn ipv4(&mut self) -> Option<u32> {
        sdk::ipv4()
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

    /// The clock reads its own uptime.
    fn wall_ms(&mut self, _now_ms: u32) -> Option<u64> {
        clock::health().0.map(|clock| clock.unix_ms)
    }

    fn time_sync(&mut self, _now_ms: u32, sync: Sync) -> Result<Jump, SyncError> {
        clock::sync(sync)
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
        match sdk::mdns_recv(socket, into) {
            sdk::Datagram::Got { len, ip, port } => Datagram::Got { len, ip, port },
            sdk::Datagram::None => Datagram::None,
            sdk::Datagram::Error => Datagram::Error,
        }
    }

    fn heap(&self) -> (u32, u32) {
        (sdk::heap_free(), sdk::heap_min_free())
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
