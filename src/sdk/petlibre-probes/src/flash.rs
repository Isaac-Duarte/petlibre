//! FW06 bench probe on KM4.
//!
//! Stage B: announce a fresh authority session and bind the KM0 peer's READY
//! into a `FlashCoord` only after stopped-output readback.
//!
//! FW15 (owner-approved region `sdk::STORAGE_OFFSET`): with `Provider`
//! installed in the SDK flash-lock hooks, stress the settings store on real
//! NOR for a physical power-cut test. Each boot opens the store and reports
//! its status, generation and contents; every saved value is derived from its
//! generation, so a recovered record can be checked for consistency. It then
//! saves back to back, capped at `STRESS_SAVES` per boot to bound wear, while
//! the operator cuts power at random. Every reopen must be consistent and
//! must hold the last completed save or the one in progress. Each open also
//! reports the slot it did not load, so a cut inside a save is visible.
//!
//! Arming sets `ATTEMPT_BIT` (survives a watchdog reset, cleared by power
//! loss); a boot that finds it set skips, so a hook fault resets at most once.
//!
//! The FW15 journal sequence (create, reserve/finish across resets, pending
//! refusal, torn record) passed on hardware and is retained in history.
//! FW06 stages C-E (park round trip, mediated reads, scratch-sector cycles)
//! passed on hardware and are retained in history; their lease sequence
//! lives in `Provider`.

use petlibre_core::clock::tick::TickClock;
use petlibre_core::feed::progress::BowlMode;
use petlibre_core::flash_coord::{FlashCoord, FlashFault, FlashPhase, FlashReply};
use petlibre_core::store::settings::{Settings, SettingsStatus, SettingsStore, SlotState};

use petlibre_hal::flash::park::{self, Mmio, Reply};
use petlibre_hal::flash::peer::MARKER_REGISTER;
use petlibre_hal::flash_lock::provider::{
    ATTEMPT_BIT, BOOT_COUNT_REGISTER, Provider, READY_TIMEOUT_MS,
};
use petlibre_hal::sdk;
use petlibre_hal::stopped;
use petlibre_hal::supervision::config::{FLASH_LIMITS, SYSTIMER_HZ, WATCHDOG};
use petlibre_hal::supervision::watchdog::Watchdog;

pub use petlibre_sdk::storage::{FlashOps, SETTINGS_BASE, SETTINGS_BINDING, slot_io};

pub const POLL_MS: u32 = 10;
pub const OP_GAP_MS: u32 = 10;
pub const FEED_PERIOD_MS: u32 = 250;
/// Saves run back to back, so most cuts land inside one.
pub const STRESS_SAVES: u32 = 1500;
pub const STRESS_BATCH: u32 = 25;
pub const SAVE_GAP_MS: u32 = 1;

/// The research unit's boot log reports flash ID c8 40 17 (GigaDevice, 8 MiB).
pub const EXPECTED_ID: [u8; 3] = [0xc8, 0x40, 0x17];

/// The stress value for generation `g`; generation 0 is the defaults.
pub fn stress_settings(generation: u64) -> Settings {
    if generation == 0 {
        return Settings::default();
    }
    Settings {
        bowl_mode: BowlMode::Single,
        lights: generation % 2 == 1,
        sound: true,
        volume: (generation % 100) as u8,
        utc_offset_minutes: 0,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Hello {
        authority: u64,
        count: u32,
    },
    Ready {
        peer: u64,
        reply: FlashReply,
        phase: FlashPhase,
        fault: FlashFault,
        inactive: bool,
    },
    Timeout {
        ms: u32,
    },
    Foreign,

    /// Watchdog and hook installation before any operation.
    Start {
        mode: u32,
        marker: u32,
        watchdog: bool,
        installed: bool,
    },
    /// The previous boot faulted before its planned end; nothing was armed.
    Skip {
        marker: u32,
    },

    /// The settings store as opened by this boot.
    Open {
        status: SettingsStatus,
        generation: u64,
        volume: u8,
        lights: bool,
        slot: u32,
        /// The slot not loaded: anything but VALID shows a save was cut.
        other: SlotState,
        consistent: bool,
    },

    /// Every `STRESS_BATCH` completed saves.
    Batch {
        saves: u32,
        generation: u64,
        ticks_max: u32,
    },
    /// A save that did not complete: stops the stress.
    SaveFailed {
        generation: u64,
        status: SettingsStatus,
    },
    /// `STRESS_SAVES` completed: idle with the watchdog fed.
    End {
        saves: u32,
        generation: u64,
        leases: u64,
        frozen: u64,
        phase: FlashPhase,
        fault: FlashFault,
    },
    ClockError,
}

/// `delay_ms == 0` ends the probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub delay_ms: u32,
    pub record: Option<Record>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Stage {
    #[default]
    Start,
    Wait,
    Arm,
    Run,
    Idle,
    Done,
}

/// Keep at a fixed address once started: the watchdog registers itself.
#[derive(Debug, Default)]
pub struct Probe {
    pub watchdog: Watchdog,
    pub settings: SettingsStore,
    clock: TickClock,
    stage: Stage,
    authority: u64,
    progress: u64,
    foreign_seen: bool,
    opened: bool,
    saves: u32,
    ticks_max: u32,
}

fn next(delay_ms: u32, record: Option<Record>) -> Step {
    Step { delay_ms, record }
}

impl Probe {
    fn end(&mut self, record: Record) -> Step {
        self.stage = Stage::Done;
        next(0, Some(record))
    }

    fn now(&mut self) -> Option<u32> {
        self.clock.read(sdk::systimer_ticks())
    }

    fn feed(&mut self) {
        self.progress += 1;
        let _ = self.watchdog.checkpoint(self.progress);
    }

    /// Stop on an unexpected result: clear the marker and idle, fed.
    fn stop(&mut self, record: Record) -> Step {
        sdk::backup_write(MARKER_REGISTER, 0);
        self.stage = Stage::Idle;
        next(FEED_PERIOD_MS, Some(record))
    }

    /// One task iteration in task context. `provider` stays at a fixed
    /// address; it is touched here only between hook calls.
    ///
    /// # Safety
    /// `provider` is valid for the probe's lifetime and not otherwise borrowed.
    pub unsafe fn step<M: Mmio, F: FlashOps>(&mut self, provider: *mut Provider) -> Step {
        match self.stage {
            Stage::Start => {
                let count = sdk::backup_read(BOOT_COUNT_REGISTER).wrapping_add(1);
                sdk::backup_write(BOOT_COUNT_REGISTER, count);
                let raw = sdk::systimer_ticks();
                self.clock = TickClock::new(SYSTIMER_HZ, raw, 0).unwrap_or_default();
                self.authority = park::session(count, raw, false);
                if !park::hello::<M>(self.authority) {
                    return self.end(Record::ClockError);
                }

                self.stage = Stage::Wait;
                next(
                    POLL_MS,
                    Some(Record::Hello {
                        authority: self.authority,
                        count,
                    }),
                )
            }

            Stage::Wait => {
                let Some(now) = self.now() else {
                    return self.end(Record::ClockError);
                };
                match park::reply::<M>(self.authority) {
                    Reply::Ready(peer) => {
                        // Bind only with verified STOP and infrared OFF readback.
                        let inactive = stopped::observe().verified();
                        let Some(mut coord) = FlashCoord::new(&FLASH_LIMITS, self.authority, now)
                        else {
                            return self.end(Record::ClockError);
                        };
                        let reply = coord.ready(self.authority, peer, inactive, now);
                        let record = Record::Ready {
                            peer,
                            reply,
                            phase: coord.phase,
                            fault: coord.fault,
                            inactive,
                        };

                        // SAFETY: the caller's contract; no hook is armed yet.
                        let p = unsafe { &mut *provider };
                        *p = Provider::bound(coord, self.clock, self.authority);

                        if reply != FlashReply::Accepted {
                            return self.end(record);
                        }
                        self.stage = Stage::Arm;
                        next(POLL_MS, Some(record))
                    }
                    _ if now >= READY_TIMEOUT_MS => self.end(Record::Timeout { ms: now }),
                    Reply::Foreign if !self.foreign_seen => {
                        self.foreign_seen = true;
                        next(POLL_MS, Some(Record::Foreign))
                    }
                    _ => next(POLL_MS, None),
                }
            }

            Stage::Arm => {
                let marker = sdk::backup_read(MARKER_REGISTER);
                if marker != 0 {
                    sdk::backup_write(MARKER_REGISTER, 0);
                    return self.end(Record::Skip { marker });
                }

                sdk::backup_write(MARKER_REGISTER, ATTEMPT_BIT);
                let watchdog = self.watchdog.open(&WATCHDOG);
                let installed = watchdog && F::install(provider);
                let record = Record::Start {
                    mode: 0,
                    marker,
                    watchdog,
                    installed,
                };

                if !installed {
                    if watchdog {
                        return self.stop(record);
                    }
                    sdk::backup_write(MARKER_REGISTER, 0);
                    return self.end(record);
                }
                self.stage = Stage::Run;
                next(OP_GAP_MS, Some(record))
            }

            Stage::Run => self.run::<F>(provider),
            Stage::Idle => {
                // Feed only while every iteration re-verifies stopped outputs,
                // and, once the hooks are armed, an idle unfaulted coordinator.
                // SAFETY: the caller's contract.
                let idle = unsafe { (*provider).ready() };
                if stopped::observe().verified() && (idle || !F::armed()) {
                    self.feed();
                }
                next(FEED_PERIOD_MS, None)
            }
            Stage::Done => next(0, None),
        }
    }

    fn run<F: FlashOps>(&mut self, provider: *mut Provider) -> Step {
        if !self.opened {
            self.opened = true;
            let id = F::read_id();
            self.feed();
            // SAFETY: `slot_io` callbacks are sound for any context.
            let status = unsafe {
                self.settings
                    .open(slot_io::<F, SETTINGS_BASE>(), SETTINGS_BINDING)
            };
            self.feed();
            let s = self.settings.settings;
            let generation = self.settings.generation;
            let record = Record::Open {
                status,
                generation,
                volume: s.volume,
                lights: s.lights,
                slot: self.settings.slot,
                other: self.settings.slots[1 - self.settings.slot as usize],
                consistent: s == stress_settings(generation),
            };

            let usable = matches!(
                status,
                SettingsStatus::Loaded | SettingsStatus::Empty | SettingsStatus::Recovered
            );

            if id != EXPECTED_ID || !usable || s != stress_settings(generation) {
                return self.stop(record);
            }
            return next(SAVE_GAP_MS, Some(record));
        }

        let generation = self.settings.generation + 1;
        let begin = sdk::systimer_ticks();
        let ok = self.settings.save(&stress_settings(generation));
        let ticks = sdk::systimer_ticks().wrapping_sub(begin);
        self.feed();

        if !ok {
            return self.stop(Record::SaveFailed {
                generation: self.settings.generation,
                status: self.settings.status,
            });
        }

        self.saves += 1;
        self.ticks_max = self.ticks_max.max(ticks);
        if self.saves >= STRESS_SAVES {
            sdk::backup_write(MARKER_REGISTER, 0);
            // SAFETY: the caller's contract; no hook call is in progress.
            let p = unsafe { &*provider };
            self.stage = Stage::Idle;
            return next(
                FEED_PERIOD_MS,
                Some(Record::End {
                    saves: self.saves,
                    generation: self.settings.generation,
                    leases: p.leases,
                    frozen: p.frozen,
                    phase: p.coord.phase,
                    fault: p.coord.fault,
                }),
            );
        }

        let record = self
            .saves
            .is_multiple_of(STRESS_BATCH)
            .then_some(Record::Batch {
                saves: self.saves,
                generation: self.settings.generation,
                ticks_max: self.ticks_max,
            });
        next(SAVE_GAP_MS, record)
    }
}
