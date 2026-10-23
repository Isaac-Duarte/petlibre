//! FW25: the device settings writer, normal role. It opens the FW15 store in
//! storage sectors 2 and 3, publishes it for the transport
//! (`petlibre_amebad::settings`), arms the flash-lock hooks once as the
//! confirmation does, and then saves each signed change the transport queues.
//! Each save sets the arming attempt marker and clears it after, so a hang
//! inside a save resets at most once per power cycle.

use core::fmt;

use petlibre_core::named_enum;

use petlibre_amebad::arming::{self, Arming, Hooks, Poll};
use petlibre_amebad::flash::park::Mmio;
use petlibre_amebad::network::{self, NET_BASE};
use petlibre_amebad::provider::Provider;
use petlibre_amebad::settings;
use petlibre_core::net_store::{NetStatus, NetStore};
use petlibre_core::settings_store::{Settings, SettingsStatus, SettingsStore};
use petlibre_core::transport::{SettingsFields, SettingsView};

use crate::probes::flash::{FlashOps, SETTINGS_BASE, SETTINGS_BINDING, slot_io};

pub const POLL_MS: u32 = 200;

named_enum! {
    /// Why a settings or Wi-Fi save did not happen (`error stage=`).
    pub enum ErrorStage {
        Unarmed = "unarmed",
        Save = "save",
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
    /// FW25 Wi-Fi credentials (sectors 5 and 6): what the store holds.
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
            write!(f, "open status={} generation={generation} ", status.0)?;
            values(f, &settings)
        }
        Record::Arming(arming::Event::Ready { reply, inactive }) => {
            write!(f, "ready reply={} inactive={}", reply.0, u8::from(inactive))
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
        Record::Error { stage, status } => write!(f, "error stage={stage} status={}", status.0),
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

#[derive(Default)]
pub struct Writer {
    stage: Stage,
    arming: Arming,
    store: SettingsStore,
    net: NetStore,
    /// The credential store's open record, logged on the next step.
    net_opened: bool,
    armed: bool,
}

fn changed(current: Settings, fields: SettingsFields) -> Settings {
    Settings {
        lights: fields.lights.unwrap_or(current.lights),
        sound: fields.sound.unwrap_or(current.sound),
        volume: fields.volume.unwrap_or(current.volume),
        ..current
    }
}

impl Writer {
    fn publish(&self) {
        let writable = self.armed
            && matches!(
                self.store.status,
                SettingsStatus::LOADED | SettingsStatus::EMPTY | SettingsStatus::RECOVERED
            );
        settings::publish(SettingsView {
            lights: self.store.settings.lights,
            sound: self.store.settings.sound,
            volume: self.store.settings.volume,
            generation: self.store.generation as u32,
            writable,
        });
        network::publish(&self.net, self.armed);
    }

    /// One task iteration.
    ///
    /// # Safety
    /// `provider` is valid for the program's lifetime, stays at a fixed
    /// address and is not otherwise borrowed.
    pub unsafe fn step<M: Mmio, H: Hooks, F: FlashOps>(&mut self, provider: *mut Provider) -> Step {
        match self.stage {
            Stage::Open => {
                // SAFETY: the slot I/O callbacks are static functions.
                let status = unsafe {
                    self.store
                        .open(slot_io::<F, SETTINGS_BASE>(), SETTINGS_BINDING)
                };
                // SAFETY: as above.
                unsafe { self.net.open(slot_io::<F, NET_BASE>()) };
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
                    return next(POLL_MS, None);
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
