//! The device settings writer, normal role. It opens the store in
//! storage sectors 2 and 3, publishes it for the transport
//! (`wellfed_hal::settings`), arms the flash-lock hooks once as the
//! confirmation does, and then saves each signed change the transport queues.
//! Each save sets the arming attempt marker and clears it after, so a hang
//! inside a save resets at most once per power cycle.
//!
//! Once armed it also keeps the keys. It mixes the seed stored across
//! boots into the generator and stores the next one (sectors 7 and 8), and
//! opens the identity and client store (sectors 9 and 10), creating the
//! feeder's identity secret on the first boot that has a generator. It
//! publishes the identity for the loop task and applies the client changes
//! the loop queues (`wellfed_hal::keys`): enrollments, revocations and the
//! Reset, which forgets every client and draws a new identity.
//!
//! It also keeps the event history (sectors 13-24,
//! `wellfed_core::store::events`). It appends the records the loop queues
//! and answers the device API's reads (`wellfed_hal::events`), polling
//! faster while either waits, so a reconnecting client catches up in
//! seconds.

use core::fmt;

use wellfed_core::named_enum;

use wellfed_core::store::clients::{ClientError, ClientStatus, ClientStore};
use wellfed_core::store::events::EventLog;
use wellfed_core::store::net::{NetStatus, NetStore};
use wellfed_core::store::seed::{SeedStatus, SeedStore};
use wellfed_core::store::settings::{Settings, SettingsStatus, SettingsStore};
use wellfed_core::transport::{SettingsFields, SettingsView};
use wellfed_hal::flash::park::Mmio;
use wellfed_hal::flash_lock::arming::{self, Arming, Hooks, Poll};
use wellfed_hal::flash_lock::provider::Provider;
pub use wellfed_hal::keys::KeyResult;
use wellfed_hal::keys::{self, Change};
use wellfed_hal::network::{self, NET_BASE};
use wellfed_hal::{events, rng, settings};

use crate::storage::{
    CLIENTS_BASE, EVENTS_BASE, EVENTS_UNITS, FlashOps, Region, SEED_BASE, SETTINGS_BASE,
    SETTINGS_BINDING, Slots,
};

pub const POLL_MS: u32 = 200;
/// While events wait to be written or read.
pub const EVENTS_POLL_MS: u32 = 5;

named_enum! {
    /// Why a settings or Wi-Fi save did not happen (`error stage=`).
    pub enum ErrorStage {
        Unarmed = "unarmed",
        Save = "save",
    }
}

named_enum! {
    /// What became of the feeder's identity secret this boot.
    pub enum Identity {
        Loaded = "loaded",
        Created = "created",
        /// No generator, a store it may not write, or unarmed.
        Missing = "missing",
        Failed = "failed",
    }
}

named_enum! {
    /// A client store change the loop queued.
    pub enum KeyChange {
        Enroll = "enroll",
        Revoke = "revoke",
        Reset = "reset",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Open {
        status: SettingsStatus,
        generation: u64,
        settings: Settings,
    },
    Arming(arming::Event),
    Saved {
        generation: u64,
        settings: Settings,
    },
    Error {
        stage: ErrorStage,
        status: SettingsStatus,
    },
    /// Wi-Fi credentials (sectors 5 and 6): what the store holds.
    NetOpen {
        status: NetStatus,
        generation: u64,
        stored: bool,
    },
    NetSaved {
        generation: u64,
        stored: bool,
    },
    NetError {
        stage: ErrorStage,
    },
    /// The stored seed and the client store.
    Keys {
        seed: SeedStatus,
        /// The stored seed went into the generator.
        mixed: bool,
        /// The next boot's seed was stored.
        saved: bool,
        clients: ClientStatus,
        enrolled: usize,
        identity: Identity,
    },
    KeysChanged {
        change: KeyChange,
        result: KeyResult,
        enrolled: usize,
    },
    /// The event ring as opened.
    Events {
        open: bool,
        last: Option<u64>,
        oldest: Option<u64>,
        epoch: u32,
    },
    /// An append failed; the record is dropped.
    EventError,
}

/// The body of a `PLCFG1` line.
pub fn describe(record: Record, f: &mut impl fmt::Write) -> fmt::Result {
    let values = |f: &mut dyn fmt::Write, s: &Settings| {
        write!(
            f,
            "lights={} sound={} volume={}",
            u8::from(s.lights),
            u8::from(s.sound),
            s.volume
        )
    };
    match record {
        Record::Open {
            status,
            generation,
            settings,
        } => {
            write!(f, "open status={} generation={generation} ", status.code())?;
            values(f, &settings)
        }
        Record::Arming(arming::Event::Ready { reply, inactive }) => {
            write!(
                f,
                "ready reply={} inactive={}",
                reply.code(),
                u8::from(inactive)
            )
        }
        Record::Arming(arming::Event::Armed {
            watchdog,
            installed,
        }) => write!(
            f,
            "armed watchdog={} installed={}",
            u8::from(watchdog),
            u8::from(installed)
        ),
        Record::Arming(arming::Event::Skip { marker }) => write!(f, "skip marker={marker:#x}"),
        Record::Arming(arming::Event::Timeout) => f.write_str("error stage=timeout"),
        Record::Arming(arming::Event::Clock) => f.write_str("error stage=clock"),
        Record::Saved {
            generation,
            settings,
        } => {
            write!(f, "saved generation={generation} ")?;
            values(f, &settings)
        }
        Record::Error { stage, status } => {
            write!(f, "error stage={stage} status={}", status.code())
        }
        // Never the SSID or passphrase.
        Record::NetOpen {
            status,
            generation,
            stored,
        } => write!(
            f,
            "net open status={} generation={generation} stored={}",
            status as u8,
            u8::from(stored)
        ),
        Record::NetSaved { generation, stored } => write!(
            f,
            "net saved generation={generation} stored={}",
            u8::from(stored)
        ),
        Record::NetError { stage } => write!(f, "net error stage={stage}"),
        Record::Keys {
            seed,
            mixed,
            saved,
            clients,
            enrolled,
            identity,
        } => write!(
            f,
            "keys seed={} mixed={} saved={} clients={} enrolled={enrolled} identity={identity}",
            seed_status(seed),
            u8::from(mixed),
            u8::from(saved),
            client_status(clients),
        ),
        Record::KeysChanged {
            change,
            result,
            enrolled,
        } => write!(
            f,
            "keys change={change} result={result} enrolled={enrolled}"
        ),
        Record::Events {
            open,
            last,
            oldest,
            epoch,
        } => write!(
            f,
            "events open={} last={} oldest={} epoch={epoch:08x}",
            u8::from(open),
            last.unwrap_or(0),
            oldest.unwrap_or(0)
        ),
        Record::EventError => f.write_str("events error stage=append"),
    }
}

fn seed_status(status: SeedStatus) -> &'static str {
    match status {
        SeedStatus::Unopened => "unopened",
        SeedStatus::Empty => "empty",
        SeedStatus::Loaded => "loaded",
        SeedStatus::ReadOnly => "read-only",
        SeedStatus::Io => "io",
    }
}

fn client_status(status: ClientStatus) -> &'static str {
    match status {
        ClientStatus::Unopened => "unopened",
        ClientStatus::Empty => "empty",
        ClientStatus::Loaded => "loaded",
        ClientStatus::ReadOnly => "read-only",
        ClientStatus::Io => "io",
    }
}

/// `delay_ms == 0` ends the task; `armed` reports the arming outcome once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub delay_ms: u32,
    pub record: Option<Record>,
    /// Arming finished this step, armed or not: later writers may start.
    pub settled: bool,
}

fn next(delay_ms: u32, record: Option<Record>) -> Step {
    Step {
        delay_ms,
        record,
        settled: false,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Stage {
    #[default]
    Open,
    Arm,
    Idle,
    /// Read-only for this boot (unarmed or a store it may not write).
    Serve,
}

/// Writes through `F`, the storage region's mediated flash.
pub struct Writer<F> {
    stage: Stage,
    arming: Arming,
    store: SettingsStore<Slots<F, SETTINGS_BASE>>,
    net: NetStore<Slots<F, NET_BASE>>,
    /// The credential store's open record, logged on the next step.
    net_opened: bool,
    armed: bool,
    seed: SeedStore<Slots<F, SEED_BASE>>,
    clients: ClientStore<Slots<F, CLIENTS_BASE>>,
    keys_opened: bool,
    events: EventLog<Region<F, EVENTS_BASE, EVENTS_UNITS>>,
    events_opened: bool,
}

impl<F> Default for Writer<F> {
    fn default() -> Self {
        Self {
            stage: Stage::default(),
            arming: Arming::default(),
            store: SettingsStore::default(),
            net: NetStore::default(),
            net_opened: false,
            armed: false,
            seed: SeedStore::default(),
            clients: ClientStore::default(),
            keys_opened: false,
            events: EventLog::default(),
            events_opened: false,
        }
    }
}

/// The epoch a new event ring takes, drawn after the seed is mixed in and
/// never 0, which a ring written before epochs reads as. A boot without a
/// generator holds no session either (every handshake needs one), so its
/// fallback only has to differ from 0; a later ring re-created with a
/// generator draws again.
fn fresh_epoch() -> u32 {
    let mut bytes = [0; 4];
    let drawn = if rng::fill(&mut bytes) {
        u32::from_le_bytes(bytes)
    } else {
        0
    };
    drawn.max(1)
}

fn changed(current: Settings, fields: SettingsFields) -> Settings {
    Settings {
        lights: fields.lights.unwrap_or(current.lights),
        sound: fields.sound.unwrap_or(current.sound),
        volume: fields.volume.unwrap_or(current.volume),
        ..current
    }
}

impl<F: FlashOps> Writer<F> {
    /// The keys: mix the stored seed in and store the next, then open
    /// the client store and create the identity if it has none. Writes only
    /// when `writable` (armed, normal stage), each under its own attempt
    /// marker.
    fn keys(&mut self, writable: bool) -> Record {
        self.seed.open(Slots::new());
        let mixed = self.seed.seed().is_some_and(|seed| rng::mix(seed));
        let saved = writable
            && rng::bytes().is_some_and(|next| {
                arming::set_attempt();
                let saved = self.seed.save(&next);
                arming::clear_attempt();
                saved
            });

        self.clients.open(Slots::new());
        let identity = if self.clients.identity().is_some() {
            Identity::Loaded
        } else if let Some(secret) = rng::bytes().filter(|_| writable && self.clients.writable()) {
            arming::set_attempt();
            let created = self.clients.set_identity(&secret).is_ok();
            arming::clear_attempt();
            if created {
                Identity::Created
            } else {
                Identity::Failed
            }
        } else {
            Identity::Missing
        };
        if let Some(secret) = self.clients.identity() {
            keys::publish_identity(secret);
        }
        keys::publish_clients(self.clients.clients().copied());

        Record::Keys {
            seed: self.seed.status,
            mixed,
            saved,
            clients: self.clients.status,
            enrolled: self.clients.clients().count(),
            identity,
        }
    }

    /// Apply one queued client change, each write under its own attempt
    /// marker, and free the mailbox with its result. A reset draws its new identity from the generator or does not
    /// happen: a weaker key is never written.
    fn change(&mut self, change: Change, writable: bool) -> Record {
        let kind = match change {
            Change::Enroll { .. } => KeyChange::Enroll,
            Change::Revoke { .. } => KeyChange::Revoke,
            Change::Reset => KeyChange::Reset,
        };
        let result = match change {
            _ if !writable => KeyResult::Unarmed,
            Change::Enroll { key, label, role } => {
                self.apply(|clients| clients.enroll(key, label, role))
            }
            Change::Revoke { key } => self.apply(|clients| clients.revoke(&key)),
            Change::Reset => match rng::bytes() {
                Some(identity) => self.apply(|clients| clients.reset(&identity)),
                None => KeyResult::NoGenerator,
            },
        };

        if kind == KeyChange::Reset
            && result == KeyResult::Done
            && let Some(secret) = self.clients.identity()
        {
            keys::publish_identity(secret);
        }
        keys::publish_clients(self.clients.clients().copied());
        keys::finish(result);
        Record::KeysChanged {
            change: kind,
            result,
            enrolled: self.clients.clients().count(),
        }
    }

    fn apply(
        &mut self,
        write: impl FnOnce(&mut ClientStore<Slots<F, CLIENTS_BASE>>) -> Result<(), ClientError>,
    ) -> KeyResult {
        arming::set_attempt();
        let result = write(&mut self.clients);
        arming::clear_attempt();
        result.map_or_else(KeyResult::from, |()| KeyResult::Done)
    }

    /// Append one queued event, or answer one read; the faster poll while
    /// either has more waiting.
    fn events_step(&mut self, writable: bool) -> Step {
        let mut record = None;
        if let Some(queued) = events::pending() {
            if writable {
                arming::set_attempt();
                let appended = self.events.append(queued.at, &queued.body);
                arming::clear_attempt();
                if appended.is_err() {
                    record = Some(Record::EventError);
                }
            }
            events::finish_pending(self.events.last_seq());
        }
        // A reader catching up asks again as soon as it has the answer.
        let read = events::read_request();
        if let Some(after) = read {
            events::finish_read(after, self.events.read_after(after));
        }

        let more = events::pending().is_some() || read.is_some();
        next(if more { EVENTS_POLL_MS } else { POLL_MS }, record)
    }

    fn publish(&self) {
        let writable = self.armed
            && matches!(
                self.store.status,
                SettingsStatus::Loaded | SettingsStatus::Empty | SettingsStatus::Recovered
            );
        settings::publish(SettingsView {
            lights: self.store.settings.lights,
            sound: self.store.settings.sound,
            volume: self.store.settings.volume,
            // Reported as 32 bits; saturate rather than wrap to an old value.
            generation: u32::try_from(self.store.generation).unwrap_or(u32::MAX),
            writable,
        });
        network::publish(&self.net, self.armed);
    }

    /// One task iteration.
    ///
    /// # Safety
    /// `provider` is valid for the program's lifetime, stays at a fixed
    /// address and is not otherwise borrowed.
    pub unsafe fn step<M: Mmio, H: Hooks>(&mut self, provider: *mut Provider) -> Step {
        match self.stage {
            Stage::Open => {
                let status = self.store.open(Slots::new(), SETTINGS_BINDING);
                self.net.open(Slots::new());
                self.armed = H::armed();
                self.stage = if self.armed { Stage::Idle } else { Stage::Arm };
                self.publish();
                let mut step = next(
                    POLL_MS,
                    Some(Record::Open {
                        status,
                        generation: self.store.generation,
                        settings: self.store.settings,
                    }),
                );
                step.settled = self.armed;
                step
            }

            // SAFETY: the caller's contract.
            Stage::Arm => match unsafe { self.arming.step::<M, H>(provider) } {
                Poll::Pending(event) => next(arming::POLL_MS, event.map(Record::Arming)),
                Poll::Armed(event) => {
                    // Armed for the rest of the boot; each save marks its own attempt.
                    arming::clear_attempt();
                    self.armed = H::armed();
                    self.stage = Stage::Idle;
                    self.publish();
                    Step {
                        delay_ms: POLL_MS,
                        record: Some(Record::Arming(event)),
                        settled: true,
                    }
                }
                Poll::Failed(event) => {
                    self.stage = Stage::Serve;
                    self.publish();
                    Step {
                        delay_ms: POLL_MS,
                        record: Some(Record::Arming(event)),
                        settled: true,
                    }
                }
            },

            Stage::Idle | Stage::Serve => {
                if !self.net_opened {
                    self.net_opened = true;
                    return next(
                        POLL_MS,
                        Some(Record::NetOpen {
                            status: self.net.status,
                            generation: self.net.generation,
                            stored: self.net.credentials.is_some(),
                        }),
                    );
                }
                if !self.keys_opened {
                    self.keys_opened = true;
                    let writable = self.stage == Stage::Idle && H::armed();
                    let record = self.keys(writable);
                    return next(POLL_MS, Some(record));
                }
                if !self.events_opened {
                    self.events_opened = true;
                    self.events
                        .open(Region::new(), EVENTS_UNITS as usize, fresh_epoch());
                    events::publish_open(self.events.epoch(), self.events.last_seq());
                    let record = Record::Events {
                        open: self.events.is_open(),
                        last: self.events.last_seq(),
                        oldest: self.events.oldest_seq(),
                        epoch: self.events.epoch(),
                    };
                    return next(POLL_MS, Some(record));
                }
                if let Some(change) = keys::pending() {
                    let writable = self.stage == Stage::Idle && H::armed();
                    let record = self.change(change, writable);
                    return next(POLL_MS, Some(record));
                }
                if let Some(credentials) = network::pending() {
                    let record = if self.stage == Stage::Serve || !H::armed() {
                        Record::NetError {
                            stage: ErrorStage::Unarmed,
                        }
                    } else {
                        arming::set_attempt();
                        let saved = self.net.save(credentials.as_ref());
                        arming::clear_attempt();
                        if saved {
                            Record::NetSaved {
                                generation: self.net.generation,
                                stored: self.net.credentials.is_some(),
                            }
                        } else {
                            Record::NetError {
                                stage: ErrorStage::Save,
                            }
                        }
                    };
                    self.publish();
                    network::finish();
                    return next(POLL_MS, Some(record));
                }
                let Some(fields) = settings::pending() else {
                    let writable = self.stage == Stage::Idle && H::armed();
                    return self.events_step(writable);
                };
                let target = changed(self.store.settings, fields);
                let record = if self.stage == Stage::Serve || !H::armed() {
                    Record::Error {
                        stage: ErrorStage::Unarmed,
                        status: self.store.status,
                    }
                } else if target == self.store.settings {
                    Record::Saved {
                        generation: self.store.generation,
                        settings: target,
                    }
                } else {
                    arming::set_attempt();
                    let saved = self.store.save(&target);
                    arming::clear_attempt();
                    if saved {
                        Record::Saved {
                            generation: self.store.generation,
                            settings: self.store.settings,
                        }
                    } else {
                        Record::Error {
                            stage: ErrorStage::Save,
                            status: self.store.status,
                        }
                    }
                };
                self.publish();
                settings::finish();
                next(POLL_MS, Some(record))
            }
        }
    }
}
