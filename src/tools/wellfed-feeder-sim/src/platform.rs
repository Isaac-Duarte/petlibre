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

use std::collections::VecDeque;
use std::convert::Infallible;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::rc::Rc;

use wellfed_board_plaf103::storage;
use wellfed_core::buttons::Gesture;
use wellfed_core::clock::wall::{Jump, Sync, SyncError, WallClock};
use wellfed_core::ota::install::{Answer, Progress, Refused};
use wellfed_core::store::slot::crc32;
use wellfed_core::tone::Tune;
use wellfed_device_api::clients::{CLIENTS, Client, Label, Role};
use wellfed_device_api::mailbox::{
    ClientChange, KeyResult, PlayResult, Requested, SettingsFields, SettingsResult, SettingsView,
    WifiResult, WifiSource, WifiView,
};
use wellfed_device_api::wifi::Credentials;
use wellfed_feeder::feeder::Platform;
use wellfed_feeder::feeder::platform::{
    Accepted, BleDrop, BleStep, Boot, Check, Datagram, DriverStep, EventSlot, Link, Received,
};
use wellfed_feeder::store::clients::{ClientError, ClientStore};
use wellfed_feeder::store::events::{BODY_BYTES, EventLog, Read};
use wellfed_feeder::store::net::NetStore;
use wellfed_feeder::store::settings::{Settings, SettingsStatus, SettingsStore};
use wellfed_hal::interlock::INTERLOCK;
use wellfed_hal::ota::mailbox::{self, Request};
use wellfed_hal::ota::signers;
use wellfed_proto::api::{self as proto, Capabilities};
use tokio::net::{TcpStream, UdpSocket};

use crate::flash::{Flash, Region};

/// This simulator's settings binding: a store written by one installation
/// is not read as another's.
const BINDING: u64 = 0x5046_5731_5349_4d01;

/// The supply the simulator reports: on the adapter, with no battery.
pub(crate) const POWER: proto::Power = proto::Power {
    adapter: true,
    battery_mv: None,
};

/// A signal the simulator reports, as a feeder near its access point.
pub(crate) const RSSI: i32 = -48;

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
    /// One accepted connection.
    Stream(u32),
}

pub struct SimPlatform {
    random: Random,
    settings: SettingsStore<Region>,
    settings_pending: Option<SettingsFields>,
    net: NetStore<Region>,
    wifi_pending: Option<Option<Credentials>>,
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
            settings,
            settings_pending: None,
            net,
            wifi_pending: None,
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
        };
        platform.publish_signers();
        Ok(platform)
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

    const LOCK_INDICATOR: &'static str = "PA4";
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
        // No motor: the server refuses feeds before they reach the owner.
        Capabilities::UPDATE
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

    fn indicator(&mut self, _on: bool) -> bool {
        true
    }

    fn indicator_check(&mut self, _on: bool) -> Check {
        Check::Held
    }

    fn indicators_off(&mut self) {}

    fn power(&mut self, _now_ms: u32) -> Option<proto::Power> {
        Some(POWER)
    }

    fn link(&mut self) -> Link {
        Link {
            rssi: RSSI,
            link_losses: 0,
            check_losses: 0,
        }
    }

    fn ipv4(&mut self) -> Option<u32> {
        Some(u32::from(self.ip))
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

    fn wall_ms(&mut self, now_ms: u32) -> Option<u64> {
        self.clock.read(now_ms).map(|reading| reading.unix_ms)
    }

    fn time_sync(&mut self, now_ms: u32, sync: Sync) -> Result<Jump, SyncError> {
        self.clock.sync(now_ms, sync)
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

    fn heap(&self) -> (u32, u32) {
        (0, 0)
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
