//! FW06 bench probe on KM4.
//!
//! Stage B: announce a fresh authority session and bind the KM0 peer's READY
//! into a `FlashCoord` only after stopped-output readback.
//!
//! Stage D: install `Provider` into the SDK flash-lock hooks, then read the
//! flash JEDEC ID `READS` times through them. Each read takes the hooked
//! `FLASH_Write_Lock`, which runs a full lease (quiesce, park KM0 in RAM),
//! sends the ROM read-ID command in SPIC user mode (no XIP while it runs) and
//! releases through `FLASH_Write_Unlock`. Nothing is erased or programmed.
//!
//! Stage C's park round trip with injected faults passed on hardware and is
//! retained in history; its lease sequence now lives in `Provider`.
//!
//! A hook fault halts in RAM until the watchdog resets the platform. The backup
//! marker is set to `ATTEMPT_MARKER` before arming, so the next boot skips the
//! reads and clears it: a fault resets at most once per power-on.

use petlibro_core::flash_coord::{
    FlashCoord, FlashFault, FlashParty, FlashPhase, FlashReply, FlashToken,
};
use petlibro_core::tick_clock::TickClock;

use crate::config::{FLASH_LIMITS, SYSTIMER_HZ, WATCHDOG};
use crate::flash_hooks::HookTicket;
use crate::flash_park::{self, Mmio, ParkToken, Reply};
use crate::flash_peer::{HEARTBEAT_REGISTER, MARKER_REGISTER, PARK_CHANNEL};
use crate::sdk;
use crate::stopped;
use crate::watchdog::Watchdog;

/// KM4 boot count; KM0 keeps its own in backup register 4.
pub const BOOT_COUNT_REGISTER: u32 = 3;
pub const POLL_MS: u32 = 10;
pub const READY_TIMEOUT_MS: u32 = 2000;
pub const READS: u32 = 100;
pub const BATCH: u32 = 25;
pub const READ_GAP_MS: u32 = 20;
pub const FEED_PERIOD_MS: u32 = 250;
pub const ATTEMPT_MARKER: u32 = 3;
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
        sdk::set_systick_ctrl(systick & !flash_park::SYSTICK_CONTROL_BITS);
        let begin_at = sdk::systimer_ticks();
        let park = park_token(&token);
        let acked =
            flash_park::ram_owner_begin::<M>(&park, PARK_CHANNEL, EXCHANGE_TICKS, EXCHANGE_POLLS);
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
        self.saved_systick = systick & crate::flash_hooks::SYSTICK_SAVED_BITS;
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
        let park = park_token(&token);
        let end_at = sdk::systimer_ticks();
        let ended =
            flash_park::ram_owner_end::<M>(&park, PARK_CHANNEL, EXCHANGE_TICKS, EXCHANGE_POLLS);
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

/// One mediated read of the flash JEDEC ID through the hooked SDK lock.
pub trait FlashOps {
    /// Install `provider` into the hooks; task context, IRQs enabled.
    fn install(provider: *mut Provider) -> bool;
    fn read_id() -> [u8; 3];
    fn armed() -> bool;
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
    /// Watchdog and hook installation before the reads.
    Start {
        marker: u32,
        watchdog: bool,
        installed: bool,
    },
    /// A previous boot faulted with the marker set; nothing was armed.
    Skip {
        marker: u32,
    },
    Batch {
        reads: u32,
        id: [u8; 3],
        mismatches: u32,
        park_max: u32,
        release_max: u32,
        frozen: u64,
        alive: u32,
    },
    Done {
        reads: u32,
        leases: u64,
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
    Read,
    Idle,
    Done,
}

/// Keep at a fixed address once started: the watchdog registers itself.
#[derive(Debug, Default)]
pub struct Probe {
    pub watchdog: Watchdog,
    clock: TickClock,
    stage: Stage,
    authority: u64,
    reads: u32,
    mismatches: u32,
    alive: u32,
    last_id: [u8; 3],
    last_beat: u32,
    progress: u64,
    foreign_seen: bool,
    pending_done: Option<Record>,
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
                self.authority = flash_park::session(count, raw, false);
                if !flash_park::hello::<M>(self.authority) {
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
                match flash_park::reply::<M>(self.authority) {
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
                        *p = Provider {
                            coord,
                            clock: self.clock,
                            authority: self.authority,
                            ..Provider::default()
                        };
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
                sdk::backup_write(MARKER_REGISTER, ATTEMPT_MARKER);
                let watchdog = self.watchdog.open(&WATCHDOG);
                let installed = watchdog && F::install(provider);
                let record = Record::Start {
                    marker,
                    watchdog,
                    installed,
                };
                if !installed {
                    // Nothing is armed for a hook fault; a fed or closed
                    // watchdog means the next boot may run the reads again.
                    sdk::backup_write(MARKER_REGISTER, 0);
                    if watchdog {
                        self.stage = Stage::Idle;
                        return next(FEED_PERIOD_MS, Some(record));
                    }
                    return self.end(record);
                }
                self.last_beat = beat();
                self.stage = Stage::Read;
                next(READ_GAP_MS, Some(record))
            }
            Stage::Read => {
                let beat_before = beat();
                let id = F::read_id();
                self.reads += 1;
                self.mismatches += u32::from(id != EXPECTED_ID);
                self.last_id = id;
                self.alive += u32::from(beat_before != self.last_beat);
                self.last_beat = beat();
                self.progress += 1;
                let _ = self.watchdog.checkpoint(self.progress);
                // SAFETY: the caller's contract; the hook call has returned.
                let p = unsafe { &*provider };
                let record = self.reads.is_multiple_of(BATCH).then_some(Record::Batch {
                    reads: self.reads,
                    id,
                    mismatches: self.mismatches,
                    park_max: p.park_max,
                    release_max: p.release_max,
                    frozen: p.frozen,
                    alive: self.alive,
                });
                if self.reads < READS {
                    return next(READ_GAP_MS, record);
                }
                let done = Record::Done {
                    reads: self.reads,
                    leases: p.leases,
                    phase: p.coord.phase,
                    fault: p.coord.fault,
                };
                sdk::backup_write(MARKER_REGISTER, 0);
                self.stage = Stage::Idle;
                // Two records: the final batch now, the summary next iteration.
                self.pending_done = Some(done);
                next(1, record)
            }
            Stage::Idle => {
                if let Some(done) = self.pending_done.take() {
                    return next(FEED_PERIOD_MS, Some(done));
                }
                // Feed only while every iteration re-verifies stopped outputs
                // and the hooks stay armed with an idle coordinator.
                // SAFETY: the caller's contract.
                let idle = unsafe { (*provider).ready() };
                if stopped::observe().verified() && idle && (F::armed() || self.reads == 0) {
                    self.progress += 1;
                    let _ = self.watchdog.checkpoint(self.progress);
                }
                next(FEED_PERIOD_MS, None)
            }
            Stage::Done => next(0, None),
        }
    }
}
