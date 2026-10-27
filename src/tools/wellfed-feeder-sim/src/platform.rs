//! [`SimPlatform`], the product loop's [`Platform`] in the simulator: the
//! firmware's real stores on a [`Flash`] image, a writer turn that stands in
//! for the settings task, the update stage's real mailbox, and the device
//! API's sockets on the host.
//!
//! The firmware's mailboxes answer a write at once and complete it on the
//! settings task's next turn; the server polls for the result. This keeps
//! that shape: a change is queued, [`SimPlatform::write`] applies it after
//! the pass, and the server sees it complete on a later pass, so clients
//! meet the same asynchronous answers they get from hardware. Events, which
//! the firmware queues for the writer too, are appended at once: nothing
//! reads them between the queue and the ring.
//!
//! The listener belongs to the serve loop, which awaits its connections and
//! hands each one over ([`SimPlatform::accepted`]); the loop's driver then
//! takes it as it takes one from lwIP. What the board provides and the
//! simulator does not have (the indicator, BLE, the speaker, a feeding
//! owner) answers as an idle board would.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::rc::Rc;
use std::time::{Duration, Instant};

use wellfed_board_plaf103::storage;
use wellfed_core::buttons::Gesture;
use wellfed_core::clock::wall::{
    Fix, Jump, Origin, State as ClockState, Sync, SyncError, WallClock,
};
use wellfed_core::clock::zone::Rule;
use wellfed_core::indicator::Light;
use wellfed_core::log_tail::{Batch as LogBatch, LogTail};
use wellfed_core::ota::install::{Answer, Progress, Refused};
use wellfed_core::store::slot::crc32;
use wellfed_core::tone::Tune;
use wellfed_device_api::clients::{CLIENTS, Client, Label, Role};
use wellfed_device_api::mailbox::{
    Calibration, CalibrationResult, CalibrationView, ClientChange, KeyResult, PlayResult,
    Requested, SettingsFields, SettingsResult, SettingsView, TimeSettings, TimeSettingsResult,
    TimeSettingsView, WifiResult, WifiSource, WifiView,
};
use wellfed_device_api::wifi::Credentials;
use wellfed_feeder::feeder::Platform;
use wellfed_feeder::feeder::platform::{
    Accepted, BleDrop, BleStep, Boot, Check, Datagram, DriverStep, EventSlot, EventsFill, Link,
    Received, TaskCount, TaskStack, TraceRead, TracesFill,
};
use wellfed_feeder::stats::{SAVE_MS, TOTAL_SECTORS, Totals, reset_cause};
use wellfed_feeder::store::calibration::CalibrationStore;
use wellfed_feeder::store::clients::{ClientError, ClientStore};
use wellfed_feeder::store::events::{BODY_BYTES, EventLog, Read};
use wellfed_feeder::store::net::NetStore;
use wellfed_feeder::store::settings::{Settings, SettingsStatus, SettingsStore};
use wellfed_feeder::store::stats::{SaveKind, StatsStore};
use wellfed_feeder::store::time::TimeStore;
use wellfed_feeder::store::traces::{self, TraceRing};
use wellfed_hal::interlock::INTERLOCK;
use wellfed_hal::ota::mailbox::{self, Request};
use wellfed_hal::ota::signers;
use wellfed_hal::sdk::LOG_TAIL_BYTES;
use wellfed_proto::api::{self as proto, Capabilities, Counts, FLASH_SECTORS, TASKS_MAX};
use wellfed_proto::trace::PAGE_MAX;
use tokio::net::{TcpStream, UdpSocket};

use crate::banks::Banks;
use crate::flash::{Flash, Region};

/// This simulator's settings binding: a store written by one installation
/// is not read as another's.
const BINDING: u64 = 0x5046_5731_5349_4d01;

// The stats record counts erases of every sector the region has; growing
// the region changes the record and takes a new version of it.
const _: () = assert!(wellfed_board_plaf103::STORAGE_SECTORS as usize == TOTAL_SECTORS);

/// The supply the simulator reports: on the adapter, with no battery.
pub(crate) const POWER: proto::Power = proto::Power {
    adapter: true,
    battery_mv: None,
};

/// A signal the simulator reports, as a feeder near its access point.
pub(crate) const RSSI: i32 = -48;
/// The channel the simulator reports its access point on.
pub(crate) const CHANNEL: u8 = 6;

/// How often the simulator's supply "is sampled", as the feeder's monitor.
const POWER_SAMPLE_MS: u32 = 5_000;

/// Setup mode's link waits this long between steps: there is no radio.
const NO_RADIO_MS: u32 = 1_000;

/// 32 bytes from the generator, or `None` when it has none.
pub type Random = Box<dyn FnMut() -> Option<[u8; 32]>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DeviceError {
    #[error("the client store cannot be written (status {0:?})")]
    Clients(wellfed_feeder::store::clients::ClientStatus),
    #[error("the generator has no output for the identity")]
    Random,
    #[error("enroll the client: {0:?}")]
    Enroll(ClientError),
    #[error("the client store holds no identity")]
    NoIdentity,
    #[error("the key is already enrolled as {0:?}; it keeps that role")]
    EnrolledAs(Role),
}

/// A socket the loop's driver holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Socket {
    /// The serve loop's listener.
    Listener,
    /// The mDNS socket, which drops everything when the simulator does not
    /// advertise.
    Mdns,
    /// The NTP exchange's socket.
    Ntp,
    /// One accepted connection.
    Stream(u32),
}

pub struct SimPlatform {
    random: Random,
    flash: Rc<Flash>,
    /// The image's erases when this boot opened it: the statistics count
    /// this boot's.
    erases_before: [u32; TOTAL_SECTORS],
    settings: SettingsStore<Region>,
    settings_pending: Option<SettingsFields>,
    net: NetStore<Region>,
    wifi_pending: Option<Option<Credentials>>,
    time: TimeStore<Region>,
    time_pending: Option<TimeSettings>,
    /// The lifetime statistics, as the writer keeps them: the stored
    /// totals with this boot counted, this boot's counts as the loop last
    /// handed them, and when the next save is due.
    stats: StatsStore<Region>,
    stats_base: Totals,
    counts: Counts,
    stats_due: Instant,
    calibration: CalibrationStore<Region>,
    calibration_pending: Option<Calibration>,
    /// The NTP exchange's socket, on an ephemeral loopback-reachable port.
    ntp: Option<std::net::UdpSocket>,
    clients: ClientStore<Region>,
    /// Bumped on every change, as the writer's published table is.
    clients_generation: u32,
    /// Bumped when the identity changes, as the writer publishes it.
    identity_generation: u32,
    keys_pending: Option<(u32, ClientChange)>,
    keys_done: Option<(u32, KeyResult)>,
    ticket: u32,
    events: EventLog<Region>,
    clock: WallClock,
    reboot: bool,
    /// This process's boots so far, this one included; status reports it as
    /// the feeder reports its boot count.
    boot_count: u32,
    /// The bank this boot runs, and its build's identity as hex.
    bank: u8,
    build: &'static str,
    /// Gestures from the operator's console, for the loop to take.
    gestures: VecDeque<Gesture>,
    /// Connections the serve loop accepted, not yet taken by the driver,
    /// and whether the last accept failed.
    accepted: VecDeque<TcpStream>,
    accept_failed: bool,
    /// The driver holds the listener open.
    listening: bool,
    streams: Vec<(u32, TcpStream)>,
    next_stream: u32,
    mdns: Option<UdpSocket>,
    /// The address the A record gives.
    ip: Ipv4Addr,
    /// The lines this boot logged, as the firmware's RAM tail keeps them;
    /// the simulator copies its records in ([`crate::sim::Sim::pass`]).
    log: Rc<LogTail<LOG_TAIL_BYTES>>,
    /// The two banks, which the bank-0 role reads bank 1 of for the stock
    /// backup; `None` for a platform opened without them.
    banks: Option<Rc<RefCell<Banks>>>,
}

impl std::fmt::Debug for SimPlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimPlatform")
            .field("clients", &self.clients)
            .field("last_event", &self.events.last_seq())
            .finish_non_exhaustive()
    }
}

impl SimPlatform {
    /// Open every store on `flash`, creating the identity on first use as
    /// the writer does, for boot number `boot_count`. With `mdns`, the
    /// device API is advertised on it as `ip`.
    pub fn open(
        flash: &Rc<Flash>,
        mut random: Random,
        boot_count: u32,
        bank: u8,
        build: [u8; 32],
        mdns: Option<(UdpSocket, Ipv4Addr)>,
    ) -> Result<Self, DeviceError> {
        let mut settings = SettingsStore::default();
        settings.open(Region::new(flash, storage::SETTINGS), BINDING);
        let mut net = NetStore::default();
        net.open(Region::new(flash, storage::NET));
        let mut time = TimeStore::default();
        time.open(Region::new(flash, storage::TIME));
        let mut stats = StatsStore::default();
        stats.open(Region::new(flash, storage::STATS));
        let mut stats_base = stats.totals;
        // The simulator's boots carry no reset flags: each is a power-on.
        stats_base.count_boot(reset_cause(0));
        let mut calibration = CalibrationStore::default();
        calibration.open(Region::new(flash, storage::CALIBRATION));
        let mut clients = ClientStore::default();
        clients.open(Region::new(flash, storage::CLIENTS));
        if clients.identity().is_none() {
            if !clients.writable() {
                return Err(DeviceError::Clients(clients.status));
            }
            let secret = random().ok_or(DeviceError::Random)?;
            clients
                .set_identity(&secret)
                .map_err(|_| DeviceError::Clients(clients.status))?;
        }
        // A new ring's epoch, drawn as the writer draws it.
        let fresh = random()
            .and_then(|bytes| bytes.first_chunk().copied())
            .map_or(1, u32::from_le_bytes);
        let mut events = EventLog::default();
        events.open(
            Region::new(flash, storage::EVENTS),
            storage::EVENTS.count as usize,
            fresh,
        );
        let (mdns, ip) = match mdns {
            Some((socket, ip)) => (Some(socket), ip),
            None => (None, Ipv4Addr::LOCALHOST),
        };

        let platform = Self {
            random,
            flash: Rc::clone(flash),
            erases_before: flash.erases(),
            settings,
            settings_pending: None,
            net,
            wifi_pending: None,
            time,
            time_pending: None,
            stats,
            stats_base,
            counts: Counts::default(),
            stats_due: Instant::now() + Duration::from_millis(u64::from(SAVE_MS)),
            calibration,
            calibration_pending: None,
            ntp: None,
            clients,
            clients_generation: 1,
            identity_generation: 1,
            keys_pending: None,
            keys_done: None,
            ticket: 0,
            events,
            clock: WallClock::new(),
            reboot: false,
            boot_count,
            bank,
            // One per boot, for the welcome's borrowed build.
            build: Box::leak(
                wellfed_session::hex::Lower(&build)
                    .to_string()
                    .into_boxed_str(),
            ),
            gestures: VecDeque::new(),
            accepted: VecDeque::new(),
            accept_failed: false,
            listening: false,
            streams: Vec::new(),
            next_stream: 0,
            mdns,
            ip,
            log: Rc::new(LogTail::new()),
            banks: None,
        };
        platform.publish_signers();
        // The boot's own early save, so its reset counts however short it is.
        let mut platform = platform;
        platform.save_now(SaveKind::Early);
        Ok(platform)
    }

    /// Read bank 1 from these banks, as the firmware reads it through the
    /// flash window (from bank 0 only).
    pub fn attach_banks(&mut self, banks: &Rc<RefCell<Banks>>) {
        self.banks = Some(Rc::clone(banks));
    }

    /// The trace ring's sectors.
    fn traces(&self) -> Region {
        Region::new(&self.flash, storage::TRACES)
    }

    /// The identity secret the client store holds.
    pub fn identity(&self) -> Option<&[u8; 32]> {
        self.clients.identity()
    }

    /// Enroll `key` straight into the store, outside the protocol: how the
    /// simulator admits its first owner without BLE setup. A key already
    /// enrolled keeps the role it has, as the protocol never changes one;
    /// a different role is refused so the operator learns why.
    pub fn enroll(&mut self, key: [u8; 32], label: Label, role: Role) -> Result<(), DeviceError> {
        match self.clients.find(&key) {
            Some(client) if client.role == role => return Ok(()),
            Some(client) => return Err(DeviceError::EnrolledAs(client.role)),
            None => {}
        }
        self.clients
            .enroll(key, label, role)
            .map_err(DeviceError::Enroll)?;
        self.clients_changed();
        Ok(())
    }

    /// A gesture from the buttons, for the next pass.
    pub(crate) fn gesture_in(&mut self, gesture: Gesture) {
        self.gestures.push_back(gesture);
    }

    /// Whether the serve loop should take connections: the driver holds
    /// the listener open.
    pub(crate) fn listening(&self) -> bool {
        self.listening
    }

    /// A connection the serve loop accepted, for the driver's next accept.
    pub(crate) fn accepted(&mut self, stream: TcpStream) {
        self.accepted.push_back(stream);
    }

    /// The serve loop's accept failed: the driver's next accept reports it
    /// and pauses the listener.
    pub(crate) fn accept_failed(&mut self) {
        self.accept_failed = true;
    }

    /// Save the lifetime totals as the writer does: an early save at a
    /// restart (`restarting`; the simulator's restart has no watchdog to
    /// race), otherwise the timed save when due. A failed save makes the
    /// store read-only for the boot, as on the feeder.
    pub(crate) fn save_stats(&mut self, restarting: bool) {
        let now = Instant::now();
        if restarting {
            self.save_now(SaveKind::Early);
        } else if now >= self.stats_due {
            self.save_now(SaveKind::Settled);
            self.stats_due = now + Duration::from_millis(u64::from(SAVE_MS));
        }
    }

    /// One save of `kind`, if the store allows it, with this boot's counts
    /// and erases, its own erase included.
    fn save_now(&mut self, kind: SaveKind) {
        if !self.stats.may_save(kind) {
            return;
        }
        let erases = self.flash.erases();
        let mut boot = [0; TOTAL_SECTORS];
        for ((out, now), before) in boot.iter_mut().zip(erases).zip(self.erases_before) {
            *out = now.saturating_sub(before);
        }
        let own = storage::STATS.first + self.stats.next_slot();
        if let Some(count) = usize::try_from(own).ok().and_then(|own| boot.get_mut(own)) {
            *count = count.saturating_add(1);
        }
        let totals = self.stats_base.plus(&self.counts, &boot, 0);
        self.stats.save(&totals, kind);
    }

    /// Whether the feeder asked for a restart, taken once.
    pub(crate) fn take_reboot(&mut self) -> bool {
        core::mem::take(&mut self.reboot)
    }

    /// Restart once this pass is done, as a watchdog reset would.
    pub(crate) fn request_reboot(&mut self) {
        self.reboot = true;
    }

    /// The writer's turn: apply what is queued. A key change bumps the
    /// table's generation, which the next pass copies to the server; its
    /// result, for the log.
    pub fn write(&mut self) -> Option<KeyResult> {
        if let Some(fields) = self.settings_pending.take() {
            let current = self.settings.settings;
            let target = Settings {
                lights: fields.lights.unwrap_or(current.lights),
                sound: fields.sound.unwrap_or(current.sound),
                volume: fields.volume.unwrap_or(current.volume),
                ..current
            };
            // A failed save leaves the generation where it was; the client
            // sees no change, as on the feeder.
            if target != current {
                self.settings.save(&target);
            }
        }
        if let Some(credentials) = self.wifi_pending.take() {
            self.net.save(credentials.as_ref());
        }
        if let Some(settings) = self.time_pending.take() {
            self.time.save(&settings);
        }
        self.save_stats(false);
        if let Some(change) = self.calibration_pending.take() {
            self.calibration.save(&change);
        }

        let (ticket, change) = self.keys_pending.take()?;
        let result = match change {
            ClientChange::Enroll { key, label, role } => {
                applied(self.clients.enroll(key, label, role))
            }
            ClientChange::Revoke { key } => applied(self.clients.revoke(&key)),
            // A reset draws its identity from the generator or does not
            // happen, as the writer's.
            ClientChange::Reset => match (self.random)() {
                Some(secret) => {
                    let result = applied(self.clients.reset(&secret));
                    if result == KeyResult::Done {
                        self.identity_generation += 1;
                    }
                    result
                }
                None => KeyResult::NoGenerator,
            },
        };
        self.keys_done = Some((ticket, result));
        self.clients_changed();
        Some(result)
    }

    /// The client table changed: the loop copies it on its next pass, and
    /// the update stage reads the owners' update keys where the firmware's
    /// settings writer publishes them.
    fn clients_changed(&mut self) {
        self.clients_generation += 1;
        self.publish_signers();
    }

    fn publish_signers(&self) {
        signers::publish(
            self.clients
                .clients()
                .filter(|client| client.role == Role::UpdateSigner)
                .map(|client| client.key),
        );
    }

    fn settings_view(&self) -> SettingsView {
        let settings = &self.settings.settings;
        SettingsView {
            lights: settings.lights,
            sound: settings.sound,
            volume: settings.volume,
            generation: u32::try_from(self.settings.generation).unwrap_or(u32::MAX),
            writable: matches!(
                self.settings.status,
                SettingsStatus::Loaded | SettingsStatus::Empty | SettingsStatus::Recovered
            ),
        }
    }

    fn wifi_view(&self) -> WifiView {
        let stored = self.net.credentials.as_ref();
        WifiView {
            stored_ssid_crc: stored.map(|c| crc32(c.ssid())),
            generation: u32::try_from(self.net.generation).unwrap_or(u32::MAX),
            writable: self.net.writable(),
            source: self.wifi_source(),
        }
    }

    fn time_view(&self) -> TimeSettingsView {
        TimeSettingsView {
            settings: self.time.settings,
            generation: u32::try_from(self.time.generation).unwrap_or(u32::MAX),
            writable: self.time.writable(),
        }
    }

    fn calibration_view(&self) -> CalibrationView {
        CalibrationView {
            calibration: self.calibration.calibration,
            generation: u32::try_from(self.calibration.generation).unwrap_or(u32::MAX),
            writable: self.calibration.writable(),
        }
    }

    /// This boot's log tail, which the simulator's records go into.
    pub(crate) fn log_tail(&self) -> Rc<LogTail<LOG_TAIL_BYTES>> {
        Rc::clone(&self.log)
    }

    fn stream(&mut self, id: u32) -> Option<&mut TcpStream> {
        self.streams
            .iter_mut()
            .find(|(stream, _)| *stream == id)
            .map(|(_, stream)| stream)
    }
}

/// A client store write's result, as the writer reports it.
fn applied(result: Result<(), ClientError>) -> KeyResult {
    result.map_or_else(KeyResult::from, |()| KeyResult::Done)
}

impl Platform for SimPlatform {
    type Socket = Socket;
    type Record = Infallible;

    const INDICATOR_SETTLE_MS: u32 = 10;
    const POWER_SAMPLE_MS: u32 = POWER_SAMPLE_MS;

    fn build(&self) -> &'static str {
        self.build
    }

    fn boot(&self) -> Boot {
        Boot {
            bank: self.bank,
            reset_flags: 0,
            boot_count: self.boot_count,
        }
    }

    fn capabilities(&self) -> Capabilities {
        // No motor: the server refuses feeds before they reach the owner,
        // and the status says why (`Unserved`). It records no trace either,
        // but serves the ones its storage ring holds.
        Capabilities::UPDATE
            .union(Capabilities::READINESS)
            .union(Capabilities::TIME)
            .union(Capabilities::LOG)
            .union(Capabilities::STATS)
            .union(Capabilities::TRACES)
            .union(Capabilities::CALIBRATION)
            .union(Capabilities::POWER)
            .union(Capabilities::BACKUP)
    }

    fn reset(&mut self) {
        self.reboot = true;
    }

    fn random(&mut self, out: &mut [u8; 32]) -> bool {
        match (self.random)() {
            Some(bytes) => {
                *out = bytes;
                true
            }
            None => false,
        }
    }

    fn identity_since(&mut self, seen: u32) -> Option<(u32, [u8; 32])> {
        let secret = self.clients.identity()?;
        (self.identity_generation != seen).then_some((self.identity_generation, *secret))
    }

    fn gesture(&mut self) -> Option<Gesture> {
        self.gestures.pop_front()
    }

    fn indicator(&mut self, _light: Light, _on: bool) -> bool {
        true
    }

    fn indicator_check(&mut self, _light: Light, _on: bool) -> Check {
        Check::Held
    }

    fn indicators_off(&mut self) {}

    fn power(&mut self, _now_ms: u32) -> Option<proto::Power> {
        Some(POWER)
    }

    fn link(&mut self) -> Link {
        Link {
            rssi: RSSI,
            channel: Some(CHANNEL),
            ..Link::default()
        }
    }

    fn ipv4(&mut self) -> Option<u32> {
        Some(u32::from(self.ip))
    }

    /// The simulator has no router: an NTP setting names an address.
    fn gateway(&mut self) -> Option<u32> {
        None
    }

    fn time_settings(&mut self) -> TimeSettingsResult {
        TimeSettingsResult::Current {
            view: self.time_view(),
            pending: self.time_pending.is_some(),
        }
    }

    fn time_settings_change(&mut self, settings: TimeSettings) -> TimeSettingsResult {
        let view = self.time_view();
        if !view.writable {
            return TimeSettingsResult::ReadOnly;
        }
        if self.time_pending.is_some() {
            return TimeSettingsResult::Busy;
        }
        self.time_pending = Some(settings);
        TimeSettingsResult::Current {
            view,
            pending: true,
        }
    }

    fn calibration(&mut self) -> CalibrationResult {
        CalibrationResult::Current {
            view: self.calibration_view(),
            pending: self.calibration_pending.is_some(),
        }
    }

    fn calibration_change(&mut self, change: Calibration) -> CalibrationResult {
        let view = self.calibration_view();
        if !view.writable {
            return CalibrationResult::ReadOnly;
        }
        if self.calibration_pending.is_some() {
            return CalibrationResult::Busy;
        }
        self.calibration_pending = Some(change);
        CalibrationResult::Current {
            view,
            pending: true,
        }
    }

    fn settings(&mut self) -> Option<SettingsView> {
        Some(self.settings_view())
    }

    fn settings_request(&mut self, change: SettingsFields) -> SettingsResult {
        let view = self.settings_view();
        if !change.changes() {
            let pending = self.settings_pending.is_some();
            return SettingsResult::Current { view, pending };
        }
        if !view.writable {
            return SettingsResult::ReadOnly;
        }
        if self.settings_pending.is_some() {
            return SettingsResult::Busy;
        }
        self.settings_pending = Some(change);
        SettingsResult::Current {
            view,
            pending: true,
        }
    }

    fn wifi(&mut self) -> WifiResult {
        WifiResult::Current {
            view: self.wifi_view(),
            pending: self.wifi_pending.is_some(),
        }
    }

    fn wifi_change(&mut self, credentials: Option<Credentials>) -> WifiResult {
        let view = self.wifi_view();
        if !view.writable {
            return WifiResult::ReadOnly;
        }
        if self.wifi_pending.is_some() {
            return WifiResult::Busy;
        }
        self.wifi_pending = Some(credentials);
        WifiResult::Current {
            view,
            pending: true,
        }
    }

    fn wifi_saving(&self) -> bool {
        self.wifi_pending.is_some()
    }

    /// The feeder reports the source of its last join, which follows a save
    /// only at the next join; there is no join here, so the store stands in
    /// for it.
    fn wifi_source(&self) -> WifiSource {
        if self.net.credentials.is_some() {
            WifiSource::Stored
        } else {
            WifiSource::Build
        }
    }

    fn keys_request(&mut self, change: ClientChange) -> Requested {
        if self.keys_pending.is_some() {
            return Requested::Busy;
        }
        self.ticket = self.ticket.wrapping_add(1).max(1);
        self.keys_pending = Some((self.ticket, change));
        Requested::Queued
    }

    fn keys_ticket(&self) -> u32 {
        self.ticket
    }

    fn keys_outcome(&self, ticket: u32) -> Option<KeyResult> {
        self.keys_done
            .filter(|&(done, _)| done == ticket)
            .map(|(_, result)| result)
    }

    fn clients_since(&mut self, seen: u32, table: &mut [Option<Client>; CLIENTS]) -> Option<u32> {
        if self.clients_generation == seen {
            return None;
        }
        *table = [None; CLIENTS];
        for (slot, client) in table.iter_mut().zip(self.clients.clients()) {
            *slot = Some(*client);
        }
        Some(self.clients_generation)
    }

    fn queue_event(&mut self, at: Option<u32>, body: [u8; BODY_BYTES], _slot: EventSlot) -> bool {
        self.events.append(at, &body).is_ok()
    }

    fn take_lost(&mut self) -> u32 {
        0
    }

    fn event_after(&mut self, after: Option<u64>) -> Option<Read> {
        Some(self.events.read_after(after))
    }

    fn epoch(&self) -> Option<u32> {
        Some(self.events.epoch())
    }

    fn last_event(&self) -> Option<u64> {
        self.events.last_seq()
    }

    fn log_after(&self, after: Option<u32>, text: &mut [u8]) -> Option<LogBatch> {
        self.log.read(after, text)
    }

    /// The simulated uptime never stops.
    fn clock_ms(&mut self, now_ms: u32) -> u32 {
        now_ms
    }

    fn wall_ms(&mut self, now_ms: u32) -> Option<u64> {
        self.clock.read(now_ms).map(|reading| reading.unix_ms)
    }

    fn time_sync(&mut self, now_ms: u32, sync: Sync, origin: Origin) -> Result<Jump, SyncError> {
        self.clock.sync(now_ms, sync, origin)
    }

    fn time_fix(&mut self, now_ms: u32, fix: Fix) -> Result<Jump, SyncError> {
        self.clock.fix(now_ms, fix)
    }

    fn clock_state(&mut self, now_ms: u32) -> ClockState {
        self.clock.state(now_ms)
    }

    fn set_zone_rule(&mut self, rule: Option<Rule>) {
        self.clock.set_rule(rule);
    }

    fn outputs_claimed(&self) -> bool {
        INTERLOCK.outputs_claimed()
    }

    fn claim_in_bound(&self, now_ms: u32) -> bool {
        INTERLOCK.claim_in_bound(now_ms)
    }

    // As the firmware's: the update stage behind the mailbox.
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

    fn bank1_read(&mut self, offset: u32, out: &mut [u8]) -> bool {
        let Some(banks) = self.banks.as_ref().filter(|_| self.bank == 0) else {
            return false;
        };
        let start = offset as usize;
        let banks = banks.borrow();
        let Some(bytes) = banks.bank1().get(start..start + out.len()) else {
            return false;
        };
        out.copy_from_slice(bytes);
        true
    }

    fn listen(&mut self, _port: u16) -> Option<Socket> {
        // The serve loop bound its listener before the first boot.
        self.listening = true;
        Some(Socket::Listener)
    }

    fn accept(&mut self, _listener: Socket) -> Accepted<Socket> {
        if core::mem::take(&mut self.accept_failed) {
            return Accepted::Error;
        }
        let Some(stream) = self.accepted.pop_front() else {
            return Accepted::None;
        };
        let id = self.next_stream;
        self.next_stream = self.next_stream.wrapping_add(1);
        self.streams.push((id, stream));
        Accepted::Socket(Socket::Stream(id))
    }

    fn receive(&mut self, socket: Socket, into: &mut [u8]) -> Received {
        let Socket::Stream(id) = socket else {
            return Received::Closed;
        };
        let Some(stream) = self.stream(id) else {
            return Received::Closed;
        };
        match stream.try_read(into) {
            Ok(0) => Received::Closed,
            Ok(n) => Received::Data(n),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Received::Empty,
            Err(_) => Received::Closed,
        }
    }

    fn send(&mut self, socket: Socket, bytes: &[u8]) -> Option<usize> {
        let Socket::Stream(id) = socket else {
            return None;
        };
        match self.stream(id)?.try_write(bytes) {
            Ok(n) => Some(n),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Some(0),
            Err(_) => None,
        }
    }

    fn close(&mut self, socket: Socket) {
        match socket {
            Socket::Listener => self.listening = false,
            Socket::Mdns => {}
            Socket::Ntp => self.ntp = None,
            Socket::Stream(id) => self.streams.retain(|(stream, _)| *stream != id),
        }
    }

    fn mdns_open(&mut self) -> Option<Socket> {
        Some(Socket::Mdns)
    }

    /// A failed send is only a lost answer: the asker asks again.
    fn mdns_send(&mut self, _socket: Socket, bytes: &[u8], ip: u32, port: u16) -> bool {
        if let Some(mdns) = &self.mdns {
            let to = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::from(ip), port));
            let _ = mdns.try_send_to(bytes, to);
        }
        true
    }

    fn mdns_receive(&mut self, _socket: Socket, into: &mut [u8]) -> Datagram {
        let Some(mdns) = &self.mdns else {
            return Datagram::None;
        };
        match mdns.try_recv_from(into) {
            Ok((len, SocketAddr::V4(from))) => Datagram::Got {
                len,
                ip: u32::from(*from.ip()),
                port: from.port(),
            },
            Ok(_) | Err(_) => Datagram::None,
        }
    }

    fn ntp_open(&mut self) -> Option<Socket> {
        let socket = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
        socket.set_nonblocking(true).ok()?;
        self.ntp = Some(socket);
        Some(Socket::Ntp)
    }

    /// A failed send is only a lost request: the exchange asks again.
    fn ntp_send(&mut self, _socket: Socket, bytes: &[u8], ip: u32, port: u16) -> bool {
        let Some(ntp) = &self.ntp else {
            return false;
        };
        let to = SocketAddrV4::new(Ipv4Addr::from(ip), port);
        ntp.send_to(bytes, to).is_ok()
    }

    fn ntp_receive(&mut self, _socket: Socket, into: &mut [u8]) -> Datagram {
        let Some(ntp) = &self.ntp else {
            return Datagram::Error;
        };
        match ntp.recv_from(into) {
            Ok((len, SocketAddr::V4(from))) => Datagram::Got {
                len,
                ip: u32::from(*from.ip()),
                port: from.port(),
            },
            Ok(_) => Datagram::None,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Datagram::None,
            // A refused port (nothing listens yet) is not the socket's end.
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => Datagram::None,
            Err(_) => Datagram::Error,
        }
    }

    fn heap(&self) -> (u32, u32) {
        (0, 0)
    }

    /// The simulator runs no RTOS tasks.
    fn task_stacks(&mut self, _out: &mut [TaskStack; TASKS_MAX]) -> TaskCount {
        TaskCount::default()
    }

    fn erases(&self) -> proto::Erases {
        let now = self.flash.erases();
        let mut storage = [0; FLASH_SECTORS];
        for ((out, now), before) in storage.iter_mut().zip(now).zip(self.erases_before) {
            *out = now.saturating_sub(before);
        }
        proto::Erases { storage, update: 0 }
    }

    fn events_fill(&self) -> EventsFill {
        EventsFill {
            kept: self.events.kept(),
            capacity: self.events.capacity(),
        }
    }

    fn traces_fill(&self) -> TracesFill {
        let mut ring = TraceRing::default();
        ring.open(self.traces(), storage::TRACES.count as usize);
        TracesFill {
            kept: ring.kept(),
            capacity: ring.capacity(),
            unsaved: 0,
        }
    }

    fn trace_page(
        &mut self,
        feed: u32,
        offset: u16,
        out: &mut [u8; PAGE_MAX],
    ) -> Result<TraceRead, proto::Refusal> {
        let units = storage::TRACES.count as usize;
        traces::page(&self.traces(), units, feed, offset, out)
    }

    fn totals(&self) -> Option<Totals> {
        self.stats.writable().then_some(self.stats_base)
    }

    fn hand_counts(&mut self, counts: Counts) {
        self.counts = counts;
    }

    fn ble_restart(&mut self) {}

    fn ble_step(&mut self, _advertise: bool) -> BleStep<Infallible> {
        BleStep {
            delay_ms: NO_RADIO_MS,
            record: None,
            link: None,
        }
    }

    fn ble_message(&self) -> Option<&[u8]> {
        None
    }

    fn ble_consume(&mut self) {}

    fn ble_send(&mut self, _body: &[u8]) -> bool {
        false
    }

    fn ble_close(&mut self, _reason: BleDrop) -> Option<Infallible> {
        None
    }

    fn speaker_restart(&mut self) {}

    fn speaker_idle(&self) -> bool {
        true
    }

    fn play(&mut self, _tune: Tune) -> PlayResult {
        // No speaker: as if `sound` were off.
        PlayResult::Muted
    }

    fn speaker_step(&mut self) -> DriverStep<Infallible> {
        DriverStep {
            delay_ms: NO_RADIO_MS,
            record: None,
        }
    }
}
