//! [`Sim`], one simulated feeder: the device API server, the panel and
//! pairing policy, and the stores, stepped in the firmware loop's order.
//!
//! One pass does what the loop task does (`src/firmware/wellfed/feeder`):
//! the boot event once, the panel's queued remote lock and its idle relock,
//! the pairing timers, the client table if the writer changed it, one
//! server step, then the writer's turn and the live samples. Button presses
//! come from the caller ([`Sim::gesture`]) and go to pairing first, then to
//! the panel, as the button service's gestures do.
//!
//! Nothing here reads a clock: every call takes the time, so tests can run
//! virtual time and the driver can run real time through the same code.

use std::rc::Rc;

use wellfed_core::buttons::Gesture;
use wellfed_feeder::api::{self as server, Pass, Server};
use wellfed_feeder::pairing::{self, Pairing, Path, Refusal};
use wellfed_feeder::panel::{self, Action, Config, Lock, Panel, Source};
use wellfed_proto::api::{EventBody, Metric, Sample, Source as EventSource};
use wellfed_session::noise::Keypair;

use crate::device::{self, DeviceError, Random, SimDevice};
use crate::flash::Flash;

/// The feeder samples its supply this often.
const POWER_MS: u32 = 5_000;
const RSSI_MS: u32 = 30_000;

/// What one step did, for the simulator's log. No record carries a key or
/// a secret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Server(server::Record),
    Panel(panel::Event),
    Pairing(pairing::Event),
    /// A Reset hold on an unlocked panel.
    Reset(Reset),
}

/// How a Reset hold ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reset {
    /// A new identity, and no clients.
    Done,
    /// The store or the generator failed; nothing changed.
    Failed(DeviceError),
}

pub struct Sim {
    server: Box<Server>,
    device: SimDevice,
    pairing: Pairing,
    panel: Panel,
    identity: Keypair,
    clients_seen: u32,
    /// Pairing's decision on a LAN recovery offer, for the next server step.
    recovery: Option<pairing::Event>,
    booted: bool,
    next_power: u32,
    next_rssi: u32,
}

impl std::fmt::Debug for Sim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sim")
            .field("server", &self.server)
            .field("device", &self.device)
            .finish_non_exhaustive()
    }
}

impl Sim {
    /// The feeder as it boots from `flash` at `now_ms`, for the
    /// `boot_count`th time: the panel locked, the clock invalid, no sessions.
    pub fn open(
        flash: &Rc<Flash>,
        random: Random,
        now_ms: u32,
        boot_count: u32,
    ) -> Result<Self, DeviceError> {
        let device = SimDevice::open(flash, random, boot_count)?;
        let identity = keypair(&device)?;
        Ok(Self {
            server: Box::new(Server::new()),
            device,
            pairing: Pairing::new(pairing::Config::DEFAULT),
            panel: Panel::new(Config::STOCK, now_ms),
            identity,
            clients_seen: 0,
            recovery: None,
            booted: false,
            next_power: now_ms,
            next_rssi: now_ms,
        })
    }

    /// The feeder's identity public key.
    pub fn identity(&self) -> [u8; 32] {
        *self.identity.public()
    }

    pub fn device(&mut self) -> &mut SimDevice {
        &mut self.device
    }

    /// The server, for the driver to move each connection's bytes.
    pub(crate) fn server(&mut self) -> &mut Server {
        &mut self.server
    }

    /// Whether the server asked for a restart, taken once: the driver drops
    /// every connection and opens the feeder again from its flash.
    pub fn take_reboot(&mut self) -> bool {
        self.device.take_reboot()
    }

    /// One gesture from the buttons: pairing takes it first (a code being
    /// keyed in, the recovery or setup hold), otherwise the panel.
    pub fn gesture(&mut self, now_ms: u32, gesture: Gesture, mut record: impl FnMut(Record)) {
        self.device.set_now(now_ms);
        if let Some(event) = self.pairing.gesture(now_ms, gesture, self.panel.lock()) {
            self.panel.noted(now_ms, gesture);
            record(Record::Pairing(event));
            self.recovery = self.recovery.or(recovery_decision(event));
            return;
        }
        let Some(event) = self.panel.gesture(now_ms, gesture) else {
            return;
        };
        record(Record::Panel(event));
        self.panel_event(now_ms, event);

        if event
            == (panel::Event::Asked {
                action: Action::Reset,
                allowed: true,
            })
        {
            record(Record::Reset(self.reset()));
        }
    }

    /// The Reset hold: a new identity and no clients, as the writer's reset.
    fn reset(&mut self) -> Reset {
        let result = self.device.reset().and_then(|()| keypair(&self.device));
        match result {
            Ok(identity) => {
                self.identity = identity;
                Reset::Done
            }
            Err(error) => Reset::Failed(error),
        }
    }

    /// A panel change made here, not by a client (whose change the server
    /// records with who made it), goes to the history.
    fn panel_event(&mut self, now_ms: u32, event: panel::Event) {
        let panel::Event::Changed { lock, source } = event else {
            return;
        };
        if source == Source::Remote {
            return;
        }
        let by = match source {
            Source::Button => EventSource::Panel,
            Source::Idle | Source::Remote => EventSource::Feeder,
        };
        let body = EventBody::Lock {
            locked: lock.locked(),
            by,
        };
        self.device.record_event(now_ms, body);
    }

    /// One pass of the loop at `now_ms`. Returns true while work is
    /// waiting, so the driver should pass again soon.
    pub fn pass(&mut self, now_ms: u32, mut record: impl FnMut(Record)) -> bool {
        self.device.set_now(now_ms);
        if !self.booted {
            self.booted = true;
            let body = EventBody::Boot {
                bank: 1,
                reset_flags: 0,
            };
            self.device.record_event(now_ms, body);
        }

        let remote = self
            .device
            .take_lock()
            .and_then(|locked| self.panel.remote(now_ms, Lock::from_locked(locked)));
        for event in [remote, self.panel.tick(now_ms)].into_iter().flatten() {
            record(Record::Panel(event));
            self.panel_event(now_ms, event);
        }
        while let Some(event) = self.pairing.tick(now_ms) {
            record(Record::Pairing(event));
            self.recovery = self.recovery.or(recovery_decision(event));
        }
        self.device.publish_lock(self.panel.lock().locked());

        self.refresh_clients(&mut record);
        let busy = self.server.step(
            Pass {
                now_ms,
                device: &mut self.device,
                pairing: &mut self.pairing,
                identity: Some(&self.identity),
                // No feeding owner yet.
                feeding: false,
                recovery: self.recovery.take(),
            },
            |r| record(Record::Server(r)),
        );
        self.device.write();
        self.samples(now_ms);
        busy
    }

    /// Copy the writer's client table into the server when it changed, so a
    /// revoked key's sessions close.
    fn refresh_clients(&mut self, record: &mut impl FnMut(Record)) {
        let (table, generation) = self.device.table();
        if generation == self.clients_seen {
            return;
        }
        self.clients_seen = generation;
        *self.server.clients_table().0 = table;
        self.server
            .clients_changed(generation, |r| record(Record::Server(r)));
    }

    fn samples(&mut self, now_ms: u32) {
        if wellfed_core::deadline_passed(now_ms, self.next_power) {
            self.next_power = now_ms.wrapping_add(POWER_MS);
            self.server.sample(Sample {
                at_ms: now_ms,
                metric: Metric::Power(device::POWER),
            });
        }
        if wellfed_core::deadline_passed(now_ms, self.next_rssi) {
            self.next_rssi = now_ms.wrapping_add(RSSI_MS);
            self.server.sample(Sample {
                at_ms: now_ms,
                metric: Metric::Rssi(device::RSSI),
            });
        }
    }
}

fn keypair(device: &SimDevice) -> Result<Keypair, DeviceError> {
    let secret = device.identity().ok_or(DeviceError::NoIdentity)?;
    Ok(Keypair::from_secret(*secret))
}

/// The recovery offer's decision, which the server's recovery session waits
/// for: its code keyed in, or its time run out.
fn recovery_decision(event: pairing::Event) -> Option<pairing::Event> {
    match event {
        pairing::Event::Entered {
            path: Path::Recovery,
            ..
        }
        | pairing::Event::Refused {
            path: Path::Recovery,
            refusal: Refusal::Unconfirmed,
        } => Some(event),
        _ => None,
    }
}
