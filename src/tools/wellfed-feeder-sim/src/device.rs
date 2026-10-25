//! [`SimDevice`], the device API server's view of the simulated feeder:
//! the firmware's real stores on a [`Flash`] image, and a writer turn that
//! stands in for the settings task.
//!
//! The firmware's mailboxes answer a write at once and complete it on the
//! settings task's next turn; the server polls for the result. This keeps
//! that shape: a change is queued, [`SimDevice::write`] applies it, and the
//! server sees it complete on a later pass, so clients meet the same
//! asynchronous answers they get from hardware. Events, which the firmware
//! queues for the writer too, are appended at once: nothing reads them
//! between the queue and the ring.

use std::rc::Rc;

use wellfed_amebad_sys::storage;
use wellfed_core::clock::wall::{Jump, Sync, SyncError, WallClock};
use wellfed_core::ota::install::Phase;
use wellfed_core::ota::offer::{KeyStatement, key_id};
use wellfed_core::store::clients::{CLIENTS, Client, ClientError, ClientStore, Label, Role};
use wellfed_core::store::events::{BODY_BYTES, EventLog, Read};
use wellfed_core::store::net::{Credentials, NetStore};
use wellfed_core::store::settings::{Settings, SettingsStatus, SettingsStore};
use wellfed_core::store::slot::crc32;
use wellfed_core::tone::Tune;
use wellfed_core::transport::{
    LockResult, PlayResult, SettingsFields, SettingsResult, SettingsView, TimeFields, TimeResult,
    WifiResult, WifiSource, WifiView,
};
use wellfed_feeder::api::{
    Device, EventRead, KeyChange, KeyOutcome, UpdateAnswer, UpdateKeyChange, UpdateRequest,
};
use wellfed_feeder::update as wire;
use wellfed_hal::ota::mailbox::{self, Answer, Request};
use wellfed_proto::api::{
    self as proto, Capabilities, EventBody, Refusal, Source, Status, UpdateRefusal, UpdateView,
};

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

/// 32 bytes from the generator, or `None` when it has none.
pub type Random = Box<dyn FnMut() -> Option<[u8; 32]>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DeviceError {
    #[error("the client store cannot be written (status {0:?})")]
    Clients(wellfed_core::store::clients::ClientStatus),
    #[error("the generator has no output for the identity")]
    Random,
    #[error("enroll the client: {0:?}")]
    Enroll(ClientError),
    #[error("the client store holds no identity")]
    NoIdentity,
    #[error("the key is already enrolled as {0:?}; it keeps that role")]
    EnrolledAs(Role),
}

pub struct SimDevice {
    random: Random,
    settings: SettingsStore<Region>,
    settings_pending: Option<SettingsFields>,
    net: NetStore<Region>,
    wifi_pending: Option<Option<Credentials>>,
    clients: ClientStore<Region>,
    /// Bumped on every change, as the writer's published table is.
    clients_generation: u32,
    keys_pending: Option<(u32, KeyChange)>,
    keys_done: Option<(u32, KeyOutcome)>,
    ticket: u32,
    events: EventLog<Region>,
    clock: WallClock,
    /// The panel's lock as the simulator last published it.
    locked: Option<bool>,
    lock_pending: Option<bool>,
    reboot: bool,
    /// This process's boots so far, this one included; status reports it as
    /// the feeder reports its boot count.
    boot_count: u32,
    /// The bank this boot runs, and its build's identity as hex.
    bank: u8,
    build: &'static str,
    /// The pass's time, which the clock anchors a sync to (the firmware
    /// reads its uptime there).
    now_ms: u32,
}

impl std::fmt::Debug for SimDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimDevice")
            .field("clients", &self.clients)
            .field("last_event", &self.events.last_seq())
            .finish_non_exhaustive()
    }
}

impl SimDevice {
    /// Open every store on `flash`, creating the identity on first use as
    /// the writer does, for boot number `boot_count`.
    pub fn open(
        flash: &Rc<Flash>,
        mut random: Random,
        boot_count: u32,
        bank: u8,
        build: [u8; 32],
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

        Ok(Self {
            random,
            settings,
            settings_pending: None,
            net,
            wifi_pending: None,
            clients,
            clients_generation: 1,
            keys_pending: None,
            keys_done: None,
            ticket: 0,
            events,
            clock: WallClock::new(),
            locked: None,
            lock_pending: None,
            reboot: false,
            boot_count,
            bank,
            // One per boot, for the welcome's borrowed build.
            build: Box::leak(
                wellfed_session::hex::Lower(&build)
                    .to_string()
                    .into_boxed_str(),
            ),
            now_ms: 0,
        })
    }

    /// Restart once this pass is done, as a watchdog reset would.
    pub(crate) fn request_reboot(&mut self) {
        self.reboot = true;
    }

    /// The identity secret the client store holds.
    pub fn identity(&self) -> Option<&[u8; 32]> {
        self.clients.identity()
    }

    /// The client table and its generation, as the writer publishes them.
    pub(crate) fn table(&self) -> ([Option<Client>; CLIENTS], u32) {
        let mut table = [None; CLIENTS];
        for (slot, client) in table.iter_mut().zip(self.clients.clients()) {
            *slot = Some(*client);
        }
        (table, self.clients_generation)
    }

    /// The time of the pass about to run.
    pub(crate) fn set_now(&mut self, now_ms: u32) {
        self.now_ms = now_ms;
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
        self.clients_generation += 1;
        Ok(())
    }

    /// Forget every client and take a new identity (the Reset hold). A
    /// reset draws its identity from the generator or does not happen.
    pub(crate) fn reset(&mut self) -> Result<(), DeviceError> {
        let secret = (self.random)().ok_or(DeviceError::Random)?;
        self.clients
            .reset(&secret)
            .map_err(|_| DeviceError::Clients(self.clients.status))?;
        self.clients_generation += 1;
        Ok(())
    }

    /// A queued lock change for the panel, taken once.
    pub(crate) fn take_lock(&mut self) -> Option<bool> {
        self.lock_pending.take()
    }

    /// The panel's lock after this step.
    pub(crate) fn publish_lock(&mut self, locked: bool) {
        self.locked = Some(locked);
    }

    /// Whether the server asked for a restart, taken once.
    pub(crate) fn take_reboot(&mut self) -> bool {
        core::mem::take(&mut self.reboot)
    }

    /// Record one durable event from outside the server (the boot, the
    /// panel's own lock changes), stamped with the clock when valid.
    pub(crate) fn record_event(&mut self, now_ms: u32, body: EventBody) {
        let at = self
            .clock
            .read(now_ms)
            .and_then(|r| u32::try_from(r.unix_ms / 1000).ok());
        let mut bytes = [0u8; BODY_BYTES];
        // Every body fits the ring's 16 bytes; one that did not would be
        // a lost event, as on the feeder.
        if wellfed_proto::encode(&body, &mut bytes).is_ok() {
            let _ = self.events.append(at, &bytes);
        }
    }

    /// The writer's turn: apply what is queued. A key change bumps the
    /// table's generation, which the next pass copies to the server.
    pub(crate) fn write(&mut self) {
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

        let Some((ticket, change)) = self.keys_pending.take() else {
            return;
        };
        let result = match change {
            KeyChange::Enroll { key, label, role } => self.clients.enroll(key, label, role),
            KeyChange::Revoke { key } => self.clients.revoke(&key),
        };
        let outcome = match result {
            Ok(()) => KeyOutcome::Done,
            Err(ClientError::Full) => KeyOutcome::Full,
            Err(ClientError::Duplicate) => KeyOutcome::Duplicate,
            Err(ClientError::NotFound) => KeyOutcome::NotFound,
            Err(ClientError::NotWritable | ClientError::IdentitySet | ClientError::Io) => {
                KeyOutcome::NotWritable
            }
        };
        self.keys_done = Some((ticket, outcome));
        self.clients_generation += 1;
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
            // The feeder reports the source of its last join, which follows
            // a save only at the next join; there is no join here, so the
            // store stands in for it.
            source: if stored.is_some() {
                WifiSource::Stored
            } else {
                WifiSource::Build
            },
        }
    }
}

impl Device for SimDevice {
    fn random(&mut self, out: &mut [u8; 32]) -> bool {
        match (self.random)() {
            Some(bytes) => {
                *out = bytes;
                true
            }
            None => false,
        }
    }

    fn build(&self) -> &'static str {
        self.build
    }

    fn status(&mut self, now_ms: u32) -> Status<'static> {
        Status {
            build: self.build,
            bank: self.bank,
            uptime_ms: now_ms,
            boot_count: self.boot_count,
            reset_flags: 0,
            faults: 0,
            rssi: RSSI,
            wifi: match self.wifi_view().source {
                WifiSource::Build => proto::WifiSource::Build,
                WifiSource::Stored => proto::WifiSource::Stored,
                WifiSource::Fallback => proto::WifiSource::Fallback,
            },
            link_losses: 0,
            dns_losses: 0,
            clock_ms: self.clock.read(now_ms).map(|r| r.unix_ms),
            power: Some(POWER),
            locked: self.locked,
            settings_generation: self.settings_view().generation,
            wifi_generation: self.wifi_view().generation,
            last_event: self.events.last_seq(),
            sessions: 0,
        }
    }

    fn settings(&mut self, change: SettingsFields) -> SettingsResult {
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

    fn wifi(&mut self, change: Option<Option<Credentials>>) -> WifiResult {
        let view = self.wifi_view();
        if let Some(change) = change {
            if !view.writable {
                return WifiResult::ReadOnly;
            }
            if self.wifi_pending.is_some() {
                return WifiResult::Busy;
            }
            self.wifi_pending = Some(change);
        }
        WifiResult::Current {
            view,
            pending: self.wifi_pending.is_some(),
        }
    }

    fn lock(&mut self, locked: bool) -> LockResult {
        let Some(current) = self.locked else {
            return LockResult::Unserved;
        };
        if self.lock_pending.is_some() {
            return LockResult::Busy;
        }
        self.lock_pending = Some(locked);
        LockResult::Current {
            locked: current,
            pending: true,
        }
    }

    fn play(&mut self, _tune: Tune) -> Option<PlayResult> {
        // The feeder firmware plays tunes only in setup mode.
        None
    }

    fn time_sync(&mut self, fields: TimeFields) -> TimeResult {
        let sync = Sync {
            unix_ms: fields.unix_ms,
            offset_s: fields.offset_s,
            next_change_s: fields.next_change_s,
            next_offset_s: fields.next_offset_s,
        };
        match self.clock.sync(self.now_ms, sync) {
            Ok(Jump::First) => TimeResult::Synced { jump_ms: None },
            Ok(Jump::Step(delta) | Jump::Jumped(delta)) => TimeResult::Synced {
                jump_ms: Some(delta),
            },
            Err(SyncError::Time) => TimeResult::Refused("time"),
            Err(SyncError::Offset) => TimeResult::Refused("offset"),
            Err(SyncError::NextChange) => TimeResult::Refused("next_change"),
        }
    }

    fn keys(&mut self, change: KeyChange) -> Option<u32> {
        if self.keys_pending.is_some() {
            return None;
        }
        self.ticket = self.ticket.wrapping_add(1).max(1);
        self.keys_pending = Some((self.ticket, change));
        Some(self.ticket)
    }

    fn keys_outcome(&mut self, ticket: u32) -> Option<KeyOutcome> {
        self.keys_done
            .filter(|&(done, _)| done == ticket)
            .map(|(_, outcome)| outcome)
    }

    fn capabilities(&self) -> Capabilities {
        // No motor: the server refuses feeds before they reach `feed`.
        Capabilities::UPDATE
    }

    fn feed(&mut self, _portions: u8, _by: Source) -> Result<u32, Refusal> {
        Err(Refusal::Unavailable)
    }

    fn feed_reserved(&mut self, _feed: u32) -> Option<Result<(), Refusal>> {
        Some(Err(Refusal::Unavailable))
    }

    fn cancel_feed(&mut self, _feed: u32) -> bool {
        false
    }

    fn record(&mut self, now_ms: u32, body: EventBody) {
        self.record_event(now_ms, body);
    }

    fn event_after(&mut self, after: Option<u64>) -> EventRead {
        match self.events.read_after(after) {
            Read::End => EventRead::None,
            Read::Gap { from, to } => EventRead::Gap { from, to },
            Read::Record(record) => {
                match wellfed_proto::decode_padded::<EventBody>(&record.body) {
                    Some(body) => EventRead::Event(proto::Event {
                        seq: record.seq,
                        at: record.at,
                        body,
                    }),
                    // As on the feeder: a body this build cannot read is
                    // skipped as lost rather than stalling the subscriber.
                    None => EventRead::Gap {
                        from: record.seq,
                        to: record.seq,
                    },
                }
            }
        }
    }

    fn epoch(&self) -> Option<u32> {
        Some(self.events.epoch())
    }

    fn reboot(&mut self) {
        self.reboot = true;
    }

    // As the firmware's: the update stage behind the mailbox.
    fn update(&mut self, request: UpdateRequest<'_>) -> Result<u32, Refusal> {
        let request = match request {
            UpdateRequest::Offer {
                manifest,
                downgrade,
                device,
            } => Request::offer(manifest, downgrade, device),
            UpdateRequest::Statement { statement } => Request::statement(statement),
            UpdateRequest::Cancel => Ok(Request::Cancel),
        };
        request.and_then(mailbox::request).map_err(wire::refusal)
    }

    fn update_outcome(&mut self, ticket: u32) -> Option<Result<UpdateAnswer, Refusal>> {
        mailbox::outcome(ticket).map(|answer| match answer {
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
        let progress = mailbox::progress().ok_or(Refusal::Busy)?;
        match progress.phase {
            Phase::Receiving => {}
            Phase::Activated => return Ok(wire::view(&progress)),
            Phase::Idle | Phase::Checking | Phase::GivingWay => {
                return Err(Refusal::Update(UpdateRefusal::NotInstalling));
            }
        }
        mailbox::chunk(offset, data).map_err(wire::refusal)?;
        let progress = mailbox::progress().ok_or(Refusal::Busy)?;
        Ok(wire::view(&progress))
    }

    fn update_view(&mut self) -> Option<UpdateView> {
        mailbox::progress().map(|progress| wire::view(&progress))
    }

    fn installing(&self) -> bool {
        mailbox::installing()
    }
}
