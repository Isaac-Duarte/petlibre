//! FW06 bench probe on KM4.
//!
//! Stage B: announce a fresh authority session and bind the KM0 peer's READY
//! into a `FlashCoord` only after stopped-output readback.
//!
//! FW15 (owner-approved region `sdk::STORAGE_OFFSET`): with `Provider`
//! installed in the SDK flash-lock hooks, run the feed journal on real NOR
//! through mediated erase/program and mapped reads. One power-on gives three
//! boots, chosen by the backup marker (survives a watchdog reset, cleared by
//! power loss):
//!
//! - mode 0: require the whole region blank, create the journal, reserve and
//!   finish two requests (CANCELLED before motion: nothing moves), reserve a
//!   third and reset deliberately with it pending.
//! - mode 1: reopen; the third request must read pending and new requests
//!   must be refused. Write a torn record (half a record, no commit) into the
//!   inactive slot and reset.
//! - mode 2: reopen past the torn record; still pending and refusing. Then
//!   wipe the journal sectors back to blank and idle.
//!
//! Arming sets `ATTEMPT_BIT` with the mode; only a boot's planned end replaces
//! the marker, so an unexpected fault resets once and the next boot skips.
//!
//! FW06 stages C-E (park round trip, mediated reads, scratch-sector cycles)
//! passed on hardware and are retained in history; their lease sequence
//! lives in `Provider`.

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
use core::ffi::{c_uint, c_void};
use petlibro_core::feed_controller::{FeedReason, FeedRequest};
use petlibro_core::feed_journal::{
    FeedJournal, JournalConfig, JournalError, JournalIo, RECORD_BYTES,
};
use petlibro_core::feed_progress::{BowlMode, Direction};
use petlibro_core::feed_storage::{OwnerOutcome, OwnerRecovery, OwnerResult};

/// KM4 boot count; KM0 keeps its own in backup register 4.
pub const BOOT_COUNT_REGISTER: u32 = 3;
pub const POLL_MS: u32 = 10;
pub const READY_TIMEOUT_MS: u32 = 2000;
pub const OP_GAP_MS: u32 = 10;
pub const FEED_PERIOD_MS: u32 = 250;
pub const ATTEMPT_BIT: u32 = 0x100;
/// Journal sectors within the storage region: slot 0 and slot 1.
pub const JOURNAL_SECTORS: [u32; 2] = [0, 1];
/// Installation binding for the research unit's bench journal.
pub const JOURNAL_BINDING: u64 = 0x5046_5731_3501_0001;
pub const MAX_PORTIONS: u16 = 12;
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

fn journal_sector(slot: c_uint) -> Option<u32> {
    JOURNAL_SECTORS.get(slot as usize).copied()
}

// Journal callbacks: the context is unused; slots map to fixed sectors.
unsafe extern "C" fn io_read<F: FlashOps>(
    _context: *mut c_void,
    slot: c_uint,
    offset: u32,
    bytes: *mut c_void,
    size: usize,
) -> bool {
    let Some(sector) = journal_sector(slot) else {
        return false;
    };
    // SAFETY: the journal passes a live buffer of `size` bytes.
    let bytes = unsafe { core::slice::from_raw_parts_mut(bytes.cast::<u8>(), size) };
    F::read(sector, offset, bytes)
}

unsafe extern "C" fn io_program<F: FlashOps>(
    _context: *mut c_void,
    slot: c_uint,
    offset: u32,
    bytes: *const c_void,
    size: usize,
) -> bool {
    let Some(sector) = journal_sector(slot) else {
        return false;
    };
    // SAFETY: as for `io_read`.
    let bytes = unsafe { core::slice::from_raw_parts(bytes.cast::<u8>(), size) };
    F::program(sector, offset, bytes)
}

unsafe extern "C" fn io_erase<F: FlashOps>(_context: *mut c_void, slot: c_uint) -> bool {
    journal_sector(slot).is_some_and(F::erase)
}

fn journal_io<F: FlashOps>() -> JournalIo {
    JournalIo {
        context: core::ptr::null_mut(),
        unit_bytes: sdk::SECTOR_BYTES,
        read: Some(io_read::<F>),
        program: Some(io_program::<F>),
        erase: Some(io_erase::<F>),
    }
}

const CONFIG: JournalConfig = JournalConfig {
    binding: JOURNAL_BINDING,
    mode: BowlMode::SINGLE,
    max_portions: MAX_PORTIONS,
};

fn request(id: u64) -> FeedRequest {
    FeedRequest {
        id,
        portions: 1,
        mode: BowlMode::SINGLE,
        direction: Direction::D0,
        credit_next_event: false,
        phase_known: true,
    }
}

/// An honest bench result: cancelled before any motion.
fn cancelled(id: u64) -> (OwnerResult, OwnerRecovery) {
    (
        OwnerResult {
            id,
            requested: 1,
            completed: 0,
            outcome: OwnerOutcome::CANCELLED,
            reason: FeedReason::CANCELLED,
            motion_possible: false,
        },
        OwnerRecovery {
            high_water_id: id,
            direction: Direction::D0,
            credit_next_event: false,
            phase_known: true,
            pending_request: false,
        },
    )
}

c_enum!(JournalOp { CREATE = 0, RESERVE = 1, FINISH = 2, OPEN = 3, TORN = 4, WIPE = 5 });

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
    /// Blank sectors of the region and the flash ID before mode 0 writes.
    Preimage {
        id: [u8; 3],
        blank_sectors: u32,
    },
    /// One journal operation, its result and the time it took.
    Op {
        op: JournalOp,
        id: u64,
        ok: bool,
        error: JournalError,
        ticks: u32,
    },
    /// The journal state after open or an operation.
    View {
        sequence: u64,
        high_water: u64,
        pending: bool,
        result_id: u64,
        outcome: OwnerOutcome,
        slot: u32,
    },
    /// A new request while one is pending must be refused.
    Refuse {
        id: u64,
        reserved: bool,
    },
    /// The planned end of this boot: deliberate reset (`reset`) or idle.
    End {
        mode: u32,
        reset: bool,
        leases: u64,
        frozen: u64,
        erases: u32,
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
    Run(u32),
    Hold,
    Idle,
    Done,
}

/// Keep at a fixed address once started: the watchdog registers itself.
#[derive(Debug, Default)]
pub struct Probe {
    pub watchdog: Watchdog,
    pub journal: FeedJournal,
    clock: TickClock,
    stage: Stage,
    mode: u32,
    authority: u64,
    progress: u64,
    foreign_seen: bool,
    erases: u32,
    pending: [Option<Record>; 2],
}

fn next(delay_ms: u32, record: Option<Record>) -> Step {
    Step { delay_ms, record }
}

fn sector_blank<F: FlashOps>(sector: u32) -> bool {
    let mut chunk = [0u8; 64];
    (0..sdk::SECTOR_BYTES / 64)
        .all(|i| F::read(sector, i * 64, &mut chunk) && chunk.iter().all(|&b| b == u8::MAX))
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

    fn view(&self) -> Record {
        let v = self.journal.read().unwrap_or_default();
        Record::View {
            sequence: v.sequence,
            high_water: v.recovery.high_water_id,
            pending: v.recovery.pending_request,
            result_id: v.result.id,
            outcome: v.result.outcome,
            slot: self.journal.slot,
        }
    }

    fn op(&mut self, op: JournalOp, id: u64, run: impl FnOnce(&mut FeedJournal) -> bool) -> Record {
        let begin = sdk::systimer_ticks();
        let ok = run(&mut self.journal);
        let ticks = sdk::systimer_ticks().wrapping_sub(begin);
        self.feed();
        Record::Op {
            op,
            id,
            ok,
            error: self.journal.error,
            ticks,
        }
    }

    /// The boot's planned end: `marker` replaces the attempt marker.
    fn finish(&mut self, provider: *const Provider, marker: u32, reset: bool) -> Record {
        sdk::backup_write(MARKER_REGISTER, marker);
        // SAFETY: the caller's contract; no hook call is in progress.
        let p = unsafe { &*provider };
        self.stage = if reset { Stage::Hold } else { Stage::Idle };
        Record::End {
            mode: self.mode,
            reset,
            leases: p.leases,
            frozen: p.frozen,
            erases: self.erases,
            phase: p.coord.phase,
            fault: p.coord.fault,
        }
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
        if let Some(record) = self.pending[0].take() {
            self.pending.rotate_left(1);
            return next(OP_GAP_MS, Some(record));
        }
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
                if marker & ATTEMPT_BIT != 0 || marker > 2 {
                    sdk::backup_write(MARKER_REGISTER, 0);
                    return self.end(Record::Skip { marker });
                }
                self.mode = marker;
                sdk::backup_write(MARKER_REGISTER, marker | ATTEMPT_BIT);
                let watchdog = self.watchdog.open(&WATCHDOG);
                let installed = watchdog && F::install(provider);
                let record = Record::Start {
                    mode: self.mode,
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
                self.stage = Stage::Run(0);
                next(OP_GAP_MS, Some(record))
            }
            Stage::Run(step) => self.run::<F>(provider, step),
            Stage::Hold => {
                // Deliberate reset: no checkpoint, so the watchdog fires.
                next(FEED_PERIOD_MS, None)
            }
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

    fn run<F: FlashOps>(&mut self, provider: *mut Provider, step: u32) -> Step {
        self.stage = Stage::Run(step + 1);
        let io = journal_io::<F>();
        let gap = |record| next(OP_GAP_MS, Some(record));
        match (self.mode, step) {
            // Mode 0: blank region, create, two finished requests, one pending.
            (0, 0) => {
                let id = F::read_id();
                self.feed();
                let blank_sectors = (0..sdk::STORAGE_SECTORS)
                    .filter(|&s| sector_blank::<F>(s))
                    .count() as u32;
                let record = Record::Preimage { id, blank_sectors };
                if id != EXPECTED_ID || blank_sectors != sdk::STORAGE_SECTORS {
                    return self.stop(record);
                }
                gap(record)
            }
            (0, 1) => {
                // SAFETY: `journal_io` callbacks are sound for any context.
                let record = self.op(JournalOp::CREATE, 0, |j| unsafe {
                    j.create(io, CONFIG, Direction::D0, false, true) == JournalError::OK
                });
                self.checked(record)
            }
            (0, 2) | (0, 4) | (0, 6) => {
                let id = u64::from(step / 2);
                self.erases += 1;
                let record = self.op(JournalOp::RESERVE, id, |j| j.reserve(&request(id)));
                self.checked(record)
            }
            (0, 3) | (0, 5) => {
                let id = u64::from(step / 2);
                let (result, phase) = cancelled(id);
                self.erases += 1;
                let record = self.op(JournalOp::FINISH, id, |j| j.finish(&result, &phase));
                self.checked(record)
            }
            (0, 7) => {
                self.pending[0] = Some(self.finish(provider, 1, true));
                gap(self.view())
            }
            // Modes 1 and 2: reopen, require the pending request, refuse new ones.
            (1 | 2, 0) => {
                // SAFETY: as for create.
                let record = self.op(JournalOp::OPEN, 0, |j| unsafe {
                    j.open(io, CONFIG) == JournalError::OK
                });
                self.pending[0] = Some(self.view());
                self.checked(record)
            }
            (1 | 2, 1) => {
                let v = self.journal.read().unwrap_or_default();
                if !v.recovery.pending_request || v.recovery.high_water_id != 3 {
                    return self.stop(self.view());
                }
                let id = v.recovery.high_water_id + 1;
                let reserved = self.journal.reserve(&request(id));
                self.feed();
                let record = Record::Refuse { id, reserved };
                if reserved {
                    return self.stop(record);
                }
                gap(record)
            }
            (1, 2) => {
                // A torn record in the inactive slot: the first half of a copy of
                // the current record, without its commit marker.
                let slot = 1 - self.journal.slot;
                let sector = JOURNAL_SECTORS[slot as usize];
                let mut torn = [0u8; RECORD_BYTES / 2];
                let ok = F::read(JOURNAL_SECTORS[self.journal.slot as usize], 0, &mut torn)
                    && F::erase(sector)
                    && F::program(sector, 0, &torn);
                self.erases += 1;
                self.feed();
                let record = Record::Op {
                    op: JournalOp::TORN,
                    id: u64::from(slot),
                    ok,
                    error: self.journal.error,
                    ticks: 0,
                };
                if !ok {
                    return self.stop(record);
                }
                self.pending[0] = Some(self.finish(provider, 2, true));
                gap(record)
            }
            (2, 2) => {
                let begin = sdk::systimer_ticks();
                let ok = JOURNAL_SECTORS.iter().all(|&s| F::erase(s))
                    && JOURNAL_SECTORS.iter().all(|&s| sector_blank::<F>(s));
                self.erases += 2;
                self.feed();
                let record = Record::Op {
                    op: JournalOp::WIPE,
                    id: 0,
                    ok,
                    error: JournalError::OK,
                    ticks: sdk::systimer_ticks().wrapping_sub(begin),
                };
                if !ok {
                    return self.stop(record);
                }
                self.pending[0] = Some(self.finish(provider, 0, false));
                gap(record)
            }
            _ => self.stop(Record::ClockError),
        }
    }

    /// Continue after a successful journal operation; stop otherwise.
    fn checked(&mut self, record: Record) -> Step {
        match record {
            Record::Op { ok: true, .. } => next(OP_GAP_MS, Some(record)),
            _ => self.stop(record),
        }
    }
}
