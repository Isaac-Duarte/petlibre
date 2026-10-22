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

use core::ffi::{c_uint, c_void};

use petlibre_core::feed_progress::BowlMode;
use petlibre_core::flash_coord::{
    FlashCoord, FlashFault, FlashParty, FlashPhase, FlashReply, FlashToken,
};
use petlibre_core::settings_store::{Settings, SettingsStatus, SettingsStore, SlotState};
use petlibre_core::storage::JournalIo;
use petlibre_core::tick_clock::TickClock;

use crate::config::{FLASH_LIMITS, SYSTIMER_HZ, WATCHDOG};
use crate::flash::hooks::HookTicket;
use crate::flash::park::{self, Mmio, ParkToken, Reply};
use crate::flash::peer::{HEARTBEAT_REGISTER, MARKER_REGISTER, PARK_CHANNEL};
use crate::sdk;
use crate::stopped;
use crate::watchdog::Watchdog;

/// KM4 boot count; KM0 keeps its own in backup register 4.
pub const BOOT_COUNT_REGISTER: u32 = 3;
pub const POLL_MS: u32 = 10;
pub const READY_TIMEOUT_MS: u32 = 2000;
pub const OP_GAP_MS: u32 = 10;
pub const FEED_PERIOD_MS: u32 = 250;
pub const ATTEMPT_BIT: u32 = 0x100;
/// Settings slots within the storage region: sectors 2 and 3 (0x342000).
pub const SETTINGS_BASE: u32 = 2;
/// Installation binding for the research unit's settings.
pub const SETTINGS_BINDING: u64 = 0x5046_5731_3502_0001;
/// Saves run back to back, so most cuts land inside one.
pub const STRESS_SAVES: u32 = 1500;
pub const STRESS_BATCH: u32 = 25;
pub const SAVE_GAP_MS: u32 = 1;
/// The research unit's boot log reports flash ID c8 40 17 (GigaDevice, 8 MiB).
pub const EXPECTED_ID: [u8; 3] = [0xc8, 0x40, 0x17];
/// Begin and end waits, inside the coordinator's 50 ms park/release limits.
pub const EXCHANGE_TICKS: u32 = 40 * SYSTIMER_HZ / 1000;
pub const EXCHANGE_POLLS: u32 = 10_000_000;

c_enum!(LeaseStage {
    REQUEST = 0,
    QUIESCE = 1,
    BEGIN = 2,
    PARKED = 3,
    COMPLETE = 4,
    END = 5,
    RELEASED = 6,
    TICKET = 7
});

fn park_token(t: &FlashToken) -> ParkToken {
    ParkToken {
        authority_lo: t.authority_session as u32,
        authority_hi: (t.authority_session >> 32) as u32,
        peer_lo: t.peer_session as u32,
        peer_hi: (t.peer_session >> 32) as u32,
        lease_lo: t.lease as u32,
        lease_hi: (t.lease >> 32) as u32,
    }
}

fn beat() -> u32 {
    sdk::backup_read(HEARTBEAT_REGISTER)
}

/// The KM4 side of the SDK flash-lock hooks. Keep at a fixed address for the
/// armed lifetime; callbacks reach it only through the hook context pointer.
#[derive(Debug, Default)]
pub struct Provider {
    pub coord: FlashCoord,
    clock: TickClock,
    authority: u64,
    saved_systick: u32,
    parked_beat: u32,
    pub leases: u64,
    pub park_max: u32,
    pub release_max: u32,
    pub frozen: u64,
    /// The first failed step, for the report after reset or halt.
    pub failure: Option<(LeaseStage, FlashReply)>,
}

impl Provider {
    /// A provider for `coord`, already bound to the KM0 peer.
    pub fn bound(coord: FlashCoord, clock: TickClock, authority: u64) -> Self {
        Self {
            coord,
            clock,
            authority,
            ..Self::default()
        }
    }

    fn now(&mut self) -> u32 {
        self.clock
            .read(sdk::systimer_ticks())
            .unwrap_or(self.clock.millis)
    }

    fn fail(&mut self, stage: LeaseStage, reply: FlashReply) -> bool {
        self.failure.get_or_insert((stage, reply));
        false
    }

    /// Hook `ready`: a bound, idle, unfaulted coordinator.
    pub fn ready(&self) -> bool {
        self.coord.phase == FlashPhase::IDLE && self.coord.fault == FlashFault::OK
    }

    /// Hook `acquire`, task context with IRQs enabled: reserve the lease,
    /// verify stopped outputs, mask IRQs, stop SysTick and park KM0 in RAM.
    /// Returns true with IRQs masked and SysTick stopped. A false return is a
    /// hook fault, which halts in RAM.
    pub fn acquire<M: Mmio>(&mut self, ticket: &mut HookTicket) -> bool {
        let now = self.now();
        let id = self.coord.token.lease + 1;
        let reply = self
            .coord
            .request(FlashParty::LOCAL, self.authority, id, now);
        if reply != FlashReply::ACCEPTED {
            return self.fail(LeaseStage::REQUEST, reply);
        }
        let token = self.coord.token;
        let reply = self
            .coord
            .quiesced(&token, stopped::observe().verified(), now);
        if reply != FlashReply::ACCEPTED {
            return self.fail(LeaseStage::QUIESCE, reply);
        }
        sdk::disable_irq();
        let systick = sdk::systick_ctrl();
        sdk::set_systick_ctrl(systick & !park::SYSTICK_CONTROL_BITS);
        let begin_at = sdk::systimer_ticks();
        let ram_token = park_token(&token);
        let acked =
            park::ram_owner_begin::<M>(&ram_token, PARK_CHANNEL, EXCHANGE_TICKS, EXCHANGE_POLLS);
        let acked_at = sdk::systimer_ticks();
        if !acked {
            return self.fail(LeaseStage::BEGIN, FlashReply::FAILED);
        }
        let now = self.now();
        let reply = self.coord.parked(&token, FlashParty::PEER, now);
        if reply != FlashReply::ACCEPTED {
            return self.fail(LeaseStage::PARKED, reply);
        }
        self.parked_beat = beat();
        self.park_max = self.park_max.max(acked_at.wrapping_sub(begin_at));
        self.saved_systick = systick & crate::flash::hooks::SYSTICK_SAVED_BITS;
        *ticket = HookTicket {
            id: token.lease,
            saved_systick: self.saved_systick,
        };
        true
    }

    /// Hook `release`, IRQs masked and SysTick stopped, after the flash
    /// operation returned the controller to XIP: release the exact ticket's
    /// lease and verify stopped outputs. Returns with IRQs still masked.
    pub fn release<M: Mmio>(&mut self, ticket: &HookTicket) -> bool {
        let token = self.coord.token;
        if ticket.id != token.lease || self.coord.phase != FlashPhase::ACTIVE {
            return self.fail(LeaseStage::TICKET, FlashReply::STALE);
        }
        self.frozen += u64::from(beat() == self.parked_beat);
        let now = self.now();
        let reply = self.coord.complete(&token, FlashParty::LOCAL, now);
        if reply != FlashReply::ACCEPTED {
            return self.fail(LeaseStage::COMPLETE, reply);
        }
        let ram_token = park_token(&token);
        let end_at = sdk::systimer_ticks();
        let ended =
            park::ram_owner_end::<M>(&ram_token, PARK_CHANNEL, EXCHANGE_TICKS, EXCHANGE_POLLS);
        let released_at = sdk::systimer_ticks();
        if !ended {
            return self.fail(LeaseStage::END, FlashReply::FAILED);
        }
        let now = self.now();
        let reply =
            self.coord
                .released(&token, FlashParty::PEER, stopped::observe().verified(), now);
        if reply != FlashReply::ACCEPTED {
            return self.fail(LeaseStage::RELEASED, reply);
        }
        self.release_max = self.release_max.max(released_at.wrapping_sub(end_at));
        self.leases += 1;
        true
    }
}

/// Mediated flash operations through the hooked SDK lock, bounded to the
/// owner-approved storage region by sector index.
pub trait FlashOps {
    /// Install `provider` into the hooks; task context, IRQs enabled.
    fn install(provider: *mut Provider) -> bool;
    fn read_id() -> [u8; 3];
    fn armed() -> bool;
    fn erase(sector: u32) -> bool;
    fn program(sector: u32, offset: u32, data: &[u8]) -> bool;
    fn read(sector: u32, offset: u32, bytes: &mut [u8]) -> bool;
}

/// Slot `slot` of the pair starting at region sector `BASE`.
fn slot_sector<const BASE: u32>(slot: c_uint) -> Option<u32> {
    (slot < 2).then_some(BASE + slot)
}

// Slot I/O callbacks: the context is unused; slots map to fixed sectors.
unsafe extern "C" fn io_read<F: FlashOps, const BASE: u32>(
    _context: *mut c_void,
    slot: c_uint,
    offset: u32,
    bytes: *mut c_void,
    size: usize,
) -> bool {
    let Some(sector) = slot_sector::<BASE>(slot) else {
        return false;
    };
    // SAFETY: the store passes a live buffer of `size` bytes.
    let bytes = unsafe { core::slice::from_raw_parts_mut(bytes.cast::<u8>(), size) };
    F::read(sector, offset, bytes)
}

unsafe extern "C" fn io_program<F: FlashOps, const BASE: u32>(
    _context: *mut c_void,
    slot: c_uint,
    offset: u32,
    bytes: *const c_void,
    size: usize,
) -> bool {
    let Some(sector) = slot_sector::<BASE>(slot) else {
        return false;
    };
    // SAFETY: as for `io_read`.
    let bytes = unsafe { core::slice::from_raw_parts(bytes.cast::<u8>(), size) };
    F::program(sector, offset, bytes)
}

unsafe extern "C" fn io_erase<F: FlashOps, const BASE: u32>(
    _context: *mut c_void,
    slot: c_uint,
) -> bool {
    slot_sector::<BASE>(slot).is_some_and(F::erase)
}

/// Two-unit slot I/O for the sector pair starting at `BASE`.
pub fn slot_io<F: FlashOps, const BASE: u32>() -> JournalIo {
    JournalIo {
        context: core::ptr::null_mut(),
        unit_bytes: sdk::SECTOR_BYTES,
        read: Some(io_read::<F, BASE>),
        program: Some(io_program::<F, BASE>),
        erase: Some(io_erase::<F, BASE>),
    }
}

/// The stress value for generation `g`; generation 0 is the defaults.
pub fn stress_settings(generation: u64) -> Settings {
    if generation == 0 {
        return Settings::default();
    }
    Settings {
        bowl_mode: BowlMode::SINGLE,
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
                        if reply != FlashReply::ACCEPTED {
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
                SettingsStatus::LOADED | SettingsStatus::EMPTY | SettingsStatus::RECOVERED
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
