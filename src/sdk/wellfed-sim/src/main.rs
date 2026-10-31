//! Virtual feeder worker: runs the real owner, board, events and watchdog
//! ports on the fake SDK with a file-backed synthetic journal, under the
//! PLAF103 firmware's feeding limits, against the measured mechanism
//! (`wellfed_sim::plant`). The world stands in for what the firmware runs
//! beside the owner: the runtime's watchdog feeder, which feeds the
//! hardware watchdog by the runtime's own rule from the owner's beat; the
//! loop's claim on the outputs around each feed; and a store writer that
//! takes its write turn and a real flash lease for each write.
//!
//! Usage: `virtual-feeder-worker JOURNAL PROFILE provision|recover`. Reads one
//! JSON [`Command`] per line on stdin and writes JSON-lines [`Event`]s on
//! stdout (the protocol in `wellfed_sim::protocol`); a power cut persists the
//! journal image and exits with status 75. Driven by the `virtual-feeder`
//! harness in this crate. The plant is the bench's measurements, not a
//! characterization of the mechanism.

use core::ffi::{c_uint, c_void};
use core::ptr;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::exit;

use clap::Parser;

use wellfed_core::feed::controller::FeedRequest;
use wellfed_core::feed::food_read::FoodReads;
use wellfed_core::feed::progress::{BowlMode, Direction};
use wellfed_core::feed::storage::{OwnerRecovery, OwnerResult, OwnerStorage, RotorPhase};
use wellfed_core::flash_coord::{FlashCoord, FlashParty, FlashReply, FlashToken};
use wellfed_core::store::feed_journal::{FeedJournal, JournalConfig, JournalError};
use wellfed_core::store::slot::SlotIo;
use wellfed_hal::interlock::{INTERLOCK, WRITER_POLL_MS, WriteStep, WriteTurn};
use wellfed_hal::owner::{Owner, OwnerConfig, OwnerState, Step};
use wellfed_hal::sdk::fake;
use wellfed_hal::stopped;
use wellfed_hal::supervision::beat;
use wellfed_hal::supervision::config::{FLASH_LIMITS, SYSTIMER_HZ, WATCHDOG};
use wellfed_hal::supervision::supervise::{self, Inputs, Startup};
use wellfed_hal::supervision::watchdog::Watchdog;
use wellfed_sim::plaf103::{self, FEED_BOUND_MS};
use wellfed_sim::plant::{Flip, Plant, PlantConfig, Turn};
use wellfed_sim::protocol::{
    Code, Command, Event, Fault, IoOperation, IoStage, Journal, Outcome, Profile, Reason, State,
    Status, StoragePhase, Submission, Wedge, WorkerArgs,
};
use wellfed_testkit::journal_flash::{FakeJournalFlash, FlashIo, UNIT};

/// The flash authority's and its peer's boot sessions.
const SESSION: u64 = 17;
const PEER: u64 = 29;

/// How long a background write holds the flash: one sector erase, which
/// the bench measured at 27-35 ms.
const LEASE_MS: u64 = 30;

/// The longest command line: a plant's parameters fit.
const MAX_LINE: usize = 1024;

/// The feedback input, PA18.
const FEEDBACK: u32 = 1 << 18;

/// The background store writer: what the settings writer's stores, the
/// event ring and the update stage do in the firmware's service task, one
/// store write at a time, each under the write turn and a flash lease.
#[derive(Default)]
struct Writer {
    /// A write waits for its turn.
    queued: bool,
    /// Writes still to queue, every `interval_ms`, the next at `next_ms`.
    remaining: u32,
    interval_ms: u64,
    next_ms: u64,
    /// When the writer next asks for its turn.
    poll_ms: u64,
    turn: WriteTurn,
    /// The lease being held, and when its erase ends.
    lease: Option<(FlashToken, u64)>,
    request: u64,
    done: u32,
}

impl Writer {
    fn waiting(&self) -> u32 {
        u32::from(self.queued || self.lease.is_some()) + self.remaining
    }
}

/// Process-wide simulation state, reached from SDK/journal callbacks.
struct World {
    owner: Owner<SimStorage>,
    watchdog: Watchdog,
    /// Refreshes the world's feeder has made; the watchdog's progress.
    feeds: u64,
    journal: FeedJournal<SimIo>,
    flash: FakeJournalFlash,
    base_io: Option<FlashIo>,
    authority: FlashCoord,
    disk: File,

    device_ms: u64,
    opened: bool,
    run_owner: bool,
    run_timer: bool,
    /// When the loop next steps the owner, at the delay its last step asked.
    step_ms: u64,

    plant: Plant,
    jammed: bool,
    wedge: Wedge,
    /// The current channel held by a `current` command.
    held_current: Option<u16>,
    /// The battery channel at rest, as a `battery` command set it; a drive
    /// on the pack alone sags it by the plant's sag.
    battery_rest: u16,
    /// Milliseconds the motor has driven one way, from the drive's start.
    drive_ms: u64,
    previous_motor: u32,

    writer: Writer,

    /// The firmware's food readings run, and the firmware's own hold on them.
    food_reads_on: bool,
    food_held: bool,
    food_reads: FoodReads,

    // Power-cut injection: cut after `cut_remaining` more mutations in storage phase
    // `cut_phase` (or at that phase's start when the count is already zero).
    cut_phase: StoragePhase,
    active_phase: StoragePhase,
    phase_start_mutations: u32,
    cut_remaining: u64,

    // Last reported outputs, so the hook only prints changes.
    observed_motor: u32,
    observed_optics: u32,
    /// The last status reported, so steps of time report only changes.
    reported: Option<Status>,
}

static mut WORLD: *mut World = ptr::null_mut();

/// The single world. Callbacks re-enter it, so each access is a short raw
/// place expression rather than a long-lived reference.
fn w() -> *mut World {
    // SAFETY: set once in `main` before any callback can run; single-threaded.
    unsafe { WORLD }
}

/// The fake flash, through the same pointer the journal I/O context holds.
/// Journal callbacks never keep a reference to it while they call back here.
fn flash_ptr() -> *mut FakeJournalFlash {
    // SAFETY: see `w`; a raw place projection creates no reference.
    unsafe { &raw mut (*w()).flash }
}

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    exit(2);
}

fn emit(event: &Event) {
    match serde_json::to_string(event) {
        Ok(line) => println!("{line}"),
        Err(_) => fail("Cannot encode worker event"),
    }
}

fn now() -> u64 {
    // SAFETY: see `w`.
    unsafe { (*w()).device_ms }
}

/// The device clock as the flash authority and the interlock read it.
fn now_ms() -> u32 {
    fake::with(|h| h.uptime_ms)
}

/// The harness's name for an owner code; a code it does not know is a
/// protocol mismatch, not something to report as a number.
fn code<C: Code>(index: u32) -> C {
    C::from_index(index.into()).unwrap_or_else(|| fail("Unknown owner status code"))
}

fn motor() -> u32 {
    let bits = (fake::with(|h| h.output[0]) >> 13) & 3;
    match bits {
        3 => 0,
        1 => 1,
        2 => 2,
        _ => 3,
    }
}

fn optics() -> u32 {
    let (a, b) = fake::with(|h| (h.output[0], h.output[1]));
    ((a >> 15) & 1) | (((b >> 3) & 1) << 1)
}

fn outputs_stopped() -> bool {
    motor() == 0 && optics() == 0
}

fn outputs() {
    let (m, o) = (motor(), optics());
    // SAFETY: see `w`.
    unsafe {
        if m != (*w()).observed_motor || o != (*w()).observed_optics {
            emit(&Event::Outputs {
                device_ms: (*w()).device_ms,
                motor: m,
                optics: o,
            });
            (*w()).observed_motor = m;
            (*w()).observed_optics = o;
        }
    }
}

fn save() {
    // SAFETY: see `w`.
    let ok = unsafe {
        let bytes = &(*flash_ptr()).bytes;
        let disk = &mut (*w()).disk;
        disk.seek(SeekFrom::Start(0)).is_ok()
            && disk.write_all(&bytes[0]).is_ok()
            && disk.write_all(&bytes[1]).is_ok()
            && disk.flush().is_ok()
    };

    if !ok {
        fail("Cannot persist synthetic journal");
    }
}

fn cut() -> ! {
    save();
    // SAFETY: see `w`.
    unsafe {
        if (*w()).disk.sync_all().is_err() {
            fail("Cannot sync synthetic cut image");
        }
        emit(&Event::PowerCut {
            device_ms: (*w()).device_ms,
            phase: (*w()).active_phase,
            mutations: (*flash_ptr()).mutations,
            rotor_ms: (*w()).plant.position_ms(),
        });
    }

    let _ = std::io::stdout().flush();
    exit(75);
}

fn mutation(_: *mut c_void) {
    save();
    // SAFETY: see `w`.
    unsafe {
        let world = w();
        if (*world).active_phase != StoragePhase::None
            && (*world).cut_phase == (*world).active_phase
        {
            (*world).cut_remaining = (*world).cut_remaining.wrapping_sub(1);
            if (*world).cut_remaining == 0 {
                cut();
            }
        }
    }
}

fn io_event(
    operation: IoOperation,
    stage: IoStage,
    slot: c_uint,
    offset: u32,
    size: usize,
    ok: bool,
) {
    emit(&Event::JournalIo {
        device_ms: now(),
        operation,
        stage,
        slot,
        offset,
        bytes: size,
        ok,
    });
}

/// The base flash, bound once the world is in place and before any journal
/// I/O.
fn base() -> FlashIo {
    // SAFETY: see `w`.
    unsafe { (*w()).base_io }.expect("the base flash is bound")
}

/// The journal's flash: the fake NOR, reported as `JournalIo` events once
/// provisioning is done.
#[derive(Clone, Copy)]
struct SimIo {
    traced: bool,
}

impl SlotIo for SimIo {
    fn unit_bytes(&self) -> u32 {
        base().unit_bytes()
    }

    fn read(&self, slot: u32, offset: u32, bytes: &mut [u8]) -> bool {
        let ok = base().read(slot, offset, bytes);
        if self.traced {
            io_event(
                IoOperation::Read,
                IoStage::End,
                slot,
                offset,
                bytes.len(),
                ok,
            );
        }
        ok
    }

    fn program(&self, slot: u32, offset: u32, bytes: &[u8]) -> bool {
        if !self.traced {
            return base().program(slot, offset, bytes);
        }

        // Journal writes must only happen with the motor and optics off; the
        // worker fails loudly if the owner ever programs or erases flash
        // while either is on.
        assert!(outputs_stopped());
        let size = bytes.len();
        io_event(
            IoOperation::Program,
            IoStage::Begin,
            slot,
            offset,
            size,
            false,
        );
        let ok = base().program(slot, offset, bytes);
        io_event(IoOperation::Program, IoStage::End, slot, offset, size, ok);
        ok
    }

    fn erase(&self, slot: u32) -> bool {
        if !self.traced {
            return base().erase(slot);
        }

        assert!(outputs_stopped());
        io_event(IoOperation::Erase, IoStage::Begin, slot, 0, UNIT, false);
        let ok = base().erase(slot);
        io_event(IoOperation::Erase, IoStage::End, slot, 0, UNIT, ok);
        ok
    }
}

/// The `cut` command can only target a phase that writes.
fn storage_begin(phase: StoragePhase) {
    assert!(outputs_stopped());
    // SAFETY: see `w`.
    unsafe {
        let world = w();
        (*world).active_phase = phase;
        (*world).phase_start_mutations = (*flash_ptr()).mutations;
        emit(&Event::StorageBegin {
            device_ms: (*world).device_ms,
            phase,
        });
        if (*world).cut_phase == phase && (*world).cut_remaining == 0 {
            cut();
        }
    }
}

fn storage_end(ok: bool) {
    // SAFETY: see `w`.
    unsafe {
        let world = w();
        emit(&Event::StorageEnd {
            device_ms: (*world).device_ms,
            phase: (*world).active_phase,
            mutations: (*flash_ptr()).mutations - (*world).phase_start_mutations,
            ok,
        });
        (*world).active_phase = StoragePhase::None;
    }
}

/// The owner's storage: the journal, each call reported as a storage phase.
struct SimStorage;

impl OwnerStorage for SimStorage {
    fn recover(&mut self, recovery: &mut OwnerRecovery) -> bool {
        storage_begin(StoragePhase::Recover);
        // SAFETY: see `w`; the journal lives in the leaked world.
        let ok = unsafe { (*w()).opened && OwnerStorage::recover(&mut (*w()).journal, recovery) };
        storage_end(ok);
        ok
    }

    fn reserve(&mut self, request: &FeedRequest) -> bool {
        storage_begin(StoragePhase::Reserve);
        // SAFETY: as for `recover`.
        let ok = unsafe { OwnerStorage::reserve(&mut (*w()).journal, request) };
        storage_end(ok);
        ok
    }

    fn finish(&mut self, result: &OwnerResult, phase: &OwnerRecovery) -> bool {
        storage_begin(StoragePhase::Finish);
        // SAFETY: as for `recover`.
        let ok = unsafe { OwnerStorage::finish(&mut (*w()).journal, result, phase) };
        storage_end(ok);
        ok
    }

    fn resolve(&mut self) -> Option<OwnerResult> {
        storage_begin(StoragePhase::Resolve);
        // SAFETY: as for `recover`.
        let result = unsafe { OwnerStorage::resolve(&mut (*w()).journal) };
        storage_end(result.is_some());
        result
    }

    fn last_result(&self) -> Option<OwnerResult> {
        // SAFETY: as for `recover`; a read of the journal's view, no flash.
        unsafe { (*w()).journal.last_result() }
    }
}

fn current(response: u64) -> Status {
    // SAFETY: see `w`.
    unsafe {
        let world = w();
        let s = (*world).owner.status();
        let opened = (*world).opened;
        let view = (*world).journal.view;

        Status {
            device_ms: (*world).device_ms,
            response,
            state: code::<State>(s.state.code()),
            fault: code::<Fault>(s.fault.code()),
            supervisor_fault: s.supervisor_fault.code(),
            watchdog_fault: (*world).watchdog.fault.code(),
            id: s.result.id,
            completed: s.result.completed,
            outcome: code::<Outcome>(s.result.outcome.code()),
            reason: code::<Reason>(s.result.reason.code()),
            durable: s.result_durable,
            stopped: s.stopped,
            optics_off: s.optics_off,
            phase_known: s.phase_known,
            at_mark: s.phase == RotorPhase::AtMark,
            homed: s.trace.homed_ms.is_some(),
            high_water_id: if opened {
                view.recovery.high_water_id
            } else {
                0
            },
            pending: opened && view.recovery.pending_request,
            journal_error: (*world).journal.error.code(),
            motor: motor(),
            optics: optics(),
            watchdog_resets: fake::with(|h| h.watchdog_reset_requests),
            rotor_ms: (*world).plant.position_ms(),
            writes_done: (*world).writer.done,
            writes_waiting: (*world).writer.waiting(),
            supply_low: s.supply_low,
            food: s.food,
            food_reading: s.food_reading,
            food_reads: s.food_reads,
            sensing: stopped::observe().sensing(),
        }
    }
}

/// The status that ends a command.
fn status(response: u64) {
    let status = current(response);
    // SAFETY: see `w`.
    unsafe { (*w()).reported = Some(status) };
    emit(&Event::Status(status));
}

/// A step of time inside a command: report the status if it changed.
fn changes() {
    let status = current(0);
    // SAFETY: see `w`.
    unsafe {
        let world = w();
        if (*world).reported.is_some_and(|last| last.same_as(&status)) {
            return;
        }
        (*world).reported = Some(status);
    }
    emit(&Event::Changed(status));
}

fn watchdog_reset() -> bool {
    fake::with(|h| h.watchdog_reset_requests) > 0
}

/// SYSTIMER ticks in the millisecond ending at `ms`, at the measured
/// 32 768 Hz.
fn ticks(ms: u64) -> u32 {
    let at = |ms: u64| ms * u64::from(SYSTIMER_HZ) / 1000;
    u32::try_from(at(ms) - at(ms - 1)).expect("33 ticks a millisecond at most")
}

/// One millisecond, and everything that runs beside the loop.
fn tick() {
    // SAFETY: see `w`.
    let (ms, run_timer) = unsafe {
        (*w()).device_ms += 1;
        ((*w()).device_ms, (*w()).run_timer)
    };
    fake::with(|h| h.uptime_ms = h.uptime_ms.wrapping_add(1));
    fake::advance(ticks(ms), run_timer);

    turn_rotor();
    write_step();
    feed_watchdog();
}

/// PA18 to `flip`'s level, interrupting only if the capture sees it.
fn feedback(flip: Flip) {
    if flip.captured {
        fake::edge(flip.high);
    } else {
        fake::with(|h| {
            if flip.high {
                h.input[0] |= FEEDBACK;
            } else {
                h.input[0] &= !FEEDBACK;
            }
        });
    }
    emit(&Event::Feedback {
        device_ms: now(),
        high: flip.high,
        captured: flip.captured,
    });
}

/// The motor's last millisecond: the rotor turns while it drives (unless
/// jammed, or wedged going forward), the current channel reads its surge,
/// its load, a stall or rest, and on the pack alone (PA19 low) the battery
/// channel sags under the same surge and load.
fn turn_rotor() {
    let m = motor();
    let driving = matches!(m, 1 | 2);
    // SAFETY: see `w`.
    let flips = unsafe {
        let world = w();
        if !driving || m != (*world).previous_motor {
            (*world).drive_ms = 0;
        }
        (*world).previous_motor = m;
        if driving {
            (*world).drive_ms += 1;
        }

        // Driving back turns the rotor off a wedge, which may free it.
        if m == 2 && (*world).wedge == Wedge::Frees {
            (*world).wedge = Wedge::Clear;
        }
        let stalled = m == 1 && (*world).wedge != Wedge::Clear;

        let c = *(*world).plant.config();
        let reading = if !driving {
            c.idle_adc
        } else if stalled {
            c.stall_adc
        } else if (*world).drive_ms <= u64::from(c.surge_ms) {
            c.surge_adc
        } else {
            (*world).plant.drive_reading()
        };
        let reading = (*world).held_current.unwrap_or(reading);
        let sag = if !driving {
            0
        } else if (*world).drive_ms <= u64::from(c.surge_ms) {
            c.surge_sag_adc
        } else {
            c.drive_sag_adc
        };
        let rest = (*world).battery_rest;
        fake::with(|h| {
            h.current_adc = reading;
            let on_pack = h.input[0] & (1 << 19) == 0;
            h.battery_adc = if on_pack {
                rest.saturating_sub(sag)
            } else {
                rest
            };
        });

        // Drive0 turns the rotor forward, Drive1 back.
        let turn = if m == 1 { Turn::Forward } else { Turn::Back };
        if driving && !(*world).jammed && !stalled {
            (*world).plant.drive(turn)
        } else {
            Vec::new()
        }
    };
    for flip in flips {
        feedback(flip);
    }
}

/// The background writer's millisecond: queue the next write when due,
/// ask for the write turn at its poll, and run a granted write's lease.
fn write_step() {
    let ms = now();
    // SAFETY: see `w`.
    unsafe {
        let writer = &raw mut (*w()).writer;
        if !(*writer).queued
            && (*writer).lease.is_none()
            && (*writer).remaining > 0
            && ms >= (*writer).next_ms
        {
            (*writer).queued = true;
            (*writer).remaining -= 1;
            (*writer).next_ms = ms + (*writer).interval_ms;
            (*writer).poll_ms = ms;
        }

        if let Some((token, ends)) = (*writer).lease {
            if ms >= ends {
                release_lease(&token);
            }
            return;
        }
        if !(*writer).queued || ms < (*writer).poll_ms {
            return;
        }
        match (*writer).turn.begin(&INTERLOCK, now_ms(), outputs_stopped) {
            WriteStep::Go(_) => take_lease(ms),
            WriteStep::Wait => (*writer).poll_ms = ms + u64::from(WRITER_POLL_MS),
        }
    }
}

/// A store write's lease: requested, quiesced with the outputs read
/// stopped, parked by the peer, then held for the erase.
fn take_lease(ms: u64) {
    let now = now_ms();
    let inactive = outputs_stopped();
    // SAFETY: see `w`.
    unsafe {
        let world = w();
        (*world).writer.request += 1;
        let authority = &raw mut (*world).authority;
        let id = (*world).writer.request;
        let granted =
            (*authority).request(FlashParty::Local, SESSION, id, now) == FlashReply::Accepted && {
                let token = (*authority).token;
                (*authority).quiesced(&token, inactive, now) == FlashReply::Accepted
                    && (*authority).parked(&token, FlashParty::Peer, now) == FlashReply::Accepted
            };
        emit(&Event::Lease {
            device_ms: ms,
            active: true,
        });
        if granted {
            (*world).writer.lease = Some(((*authority).token, ms + LEASE_MS));
        } else {
            end_write(false);
        }
    }
}

fn release_lease(token: &FlashToken) {
    let now = now_ms();
    let inactive = outputs_stopped();
    // SAFETY: see `w`.
    let ok = unsafe {
        let authority = &raw mut (*w()).authority;
        (*authority).complete(token, FlashParty::Local, now) == FlashReply::Accepted
            && (*authority).released(token, FlashParty::Peer, inactive, now) == FlashReply::Accepted
    };
    end_write(ok);
}

/// The write is over: the turn goes back, and a write that failed its
/// lease is not counted done (the authority's fault shows in the owner).
fn end_write(ok: bool) {
    INTERLOCK.end_write();
    // SAFETY: see `w`.
    unsafe {
        let writer = &raw mut (*w()).writer;
        (*writer).lease = None;
        (*writer).queued = false;
        if ok {
            (*writer).done += 1;
        }
    }
    emit(&Event::Lease {
        device_ms: now(),
        active: false,
    });
}

/// The runtime's watchdog feeder, once the role has started: feed while
/// outputs read stopped or the owner supervises them, and the owner's beat
/// stays fresh.
fn feed_watchdog() {
    let outputs = stopped::observe();
    let inputs = Inputs {
        now_ms: now_ms(),
        startup: Startup::Started,
        tasks_alive: true,
        outputs_ok: outputs.verified() || outputs.sensing(),
        owner: beat::read(),
        ..Inputs::default()
    };
    if !supervise::feed(&inputs) {
        return;
    }
    // SAFETY: see `w`.
    unsafe {
        let world = w();
        (*world).feeds += 1;
        let progress = (*world).feeds;
        let _ = (*world).watchdog.checkpoint(progress);
    }
}

/// The loop's turn: the owner's step when its delay is up, then, as the
/// firmware's loop does, the outputs' claim handed back once the owner
/// rests with its outputs stopped.
fn poll() {
    // SAFETY: see `w`.
    unsafe {
        let world = w();
        let due = (*world).device_ms >= (*world).step_ms;
        // As the firmware's loop does it, before the owner's step. The worker
        // has no mailbox, so work queued for a writer is the writer's own.
        if (*world).food_reads_on && (*world).run_owner && (*world).owner.running() && due {
            let held = (*world).food_held || (*world).writer.queued;
            let reads = &raw mut (*world).food_reads;
            let now = now_ms();
            (*world)
                .owner
                .food_read_step(&mut *reads, &INTERLOCK, now, held);
        }
        if (*world).run_owner
            && (*world).owner.running()
            && due
            && let Step::Wait(ms) = (*world).owner.step()
        {
            (*world).step_ms = (*world).device_ms + u64::from(ms);
        }

        let s = (*world).owner.status();
        let resting = matches!(
            s.state,
            OwnerState::Starting
                | OwnerState::Idle
                | OwnerState::Terminal
                | OwnerState::Fault
                | OwnerState::Stopped
        );
        if resting && s.stopped && s.optics_off {
            INTERLOCK.release_feed();
        }
    }
}

/// Ticks and polls, as the loop runs between commands.
fn run(ms: u32) {
    for _ in 0..ms {
        tick();
        // The harness restarts the worker on the reset's own millisecond.
        if watchdog_reset() {
            return;
        }
        poll();
        changes();
    }
}

/// A feed, as the firmware's loop submits it: claim the outputs first,
/// letting a queued or running store write go first (time passes for the
/// writer meanwhile, not for the loop), then hand it to the owner.
fn submit(id: u64, portions: u16) -> Submission {
    // SAFETY: see `w`.
    unsafe { (*w()).owner.yield_food_read(&INTERLOCK) };
    let claimed = INTERLOCK.claim_feed_after_writes(
        FEED_BOUND_MS,
        // SAFETY: see `w`.
        || unsafe { (*w()).writer.queued || (*w()).writer.lease.is_some() },
        now_ms,
        |ms| {
            for _ in 0..ms {
                tick();
                changes();
            }
        },
    );
    if claimed.is_err() {
        return Submission::RejectHeld;
    }
    // SAFETY: see `w`.
    let result = unsafe { (*w()).owner.submit(id, portions) };
    code(result.code())
}

/// PA18 to the plant's level, without an interrupt: where the rotor rests.
fn place_rotor() {
    // SAFETY: see `w`.
    let high = unsafe { (*w()).plant.level() };
    fake::with(|h| {
        if high {
            h.input[0] |= FEEDBACK;
        } else {
            h.input[0] &= !FEEDBACK;
        }
    });
}

fn open_disk(path: &Path, provision: bool) -> File {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if provision {
        options.create_new(true);
    }
    options
        .open(path)
        .unwrap_or_else(|_| fail("Cannot open synthetic journal"))
}

fn main() {
    let args = WorkerArgs::parse();
    let provision = args.start == Journal::Provision;
    let mode = match args.profile {
        Profile::Single => BowlMode::Single,
        Profile::Double => BowlMode::Double,
    };
    let mut disk = open_disk(&args.journal, provision);

    let mut flash = FakeJournalFlash::default();
    if !provision {
        let mut image = Vec::new();
        if disk.read_to_end(&mut image).is_err() || image.len() != 2 * UNIT {
            fail("Invalid synthetic journal size");
        }
        flash.bytes[0].copy_from_slice(&image[..UNIT]);
        flash.bytes[1].copy_from_slice(&image[UNIT..]);
    }

    // Installed before any I/O is bound; the flash is not written directly later.
    flash.after_mutation = Some(mutation);
    let world = Box::into_raw(Box::new(World {
        owner: Owner::default(),
        watchdog: Watchdog::default(),
        feeds: 0,
        journal: FeedJournal::default(),
        flash,
        base_io: None,
        authority: FlashCoord::default(),
        disk,
        device_ms: 0,
        opened: false,
        run_owner: true,
        run_timer: true,
        step_ms: 0,
        plant: Plant::new(PlantConfig::default(), 0),
        jammed: false,
        wedge: Wedge::Clear,
        held_current: None,
        battery_rest: fake::with(|h| h.battery_adc),
        drive_ms: 0,
        previous_motor: 0,
        writer: Writer::default(),
        food_reads_on: false,
        food_held: false,
        food_reads: FoodReads::new(),
        cut_phase: StoragePhase::None,
        active_phase: StoragePhase::None,
        phase_start_mutations: 0,
        cut_remaining: 0,
        // Out of range, so the first output hook always reports.
        observed_motor: 99,
        observed_optics: 99,
        reported: None,
    }));

    // SAFETY: the world is leaked for the process lifetime; single-threaded.
    unsafe {
        WORLD = world;
        (*world).base_io = Some(FakeJournalFlash::io_at(flash_ptr()));
        let jc = JournalConfig {
            binding: 17,
            mode,
            max_portions: plaf103::MAX_PORTIONS,
        };

        if provision {
            let io = SimIo { traced: false };
            if (*world).journal.create(io, jc, Direction::D0, true, true) != JournalError::Ok {
                fail("Cannot provision synthetic journal");
            }
            save();
        }

        (*world).opened = (*world).journal.open(SimIo { traced: true }, jc) == JournalError::Ok;

        fake::reset_hardware();
        fake::with(|h| h.output_hook = Some(outputs));
        place_rotor();

        (*world).authority =
            FlashCoord::new(&FLASH_LIMITS, SESSION, 0).expect("the flash limits are valid");
        assert_eq!(
            (*world).authority.ready(SESSION, PEER, true, 0),
            FlashReply::Accepted
        );

        let c = OwnerConfig {
            bowl_mode: mode,
            flash: &raw mut (*world).authority,
            ..plaf103::OWNER
        };

        if !(*world).watchdog.open(&WATCHDOG) {
            fail("Watchdog open failed");
        }
        // The owner's first step, which recovers the journal, is the first
        // poll's, so a scenario can cut that recovery's writes.
        if !(*world).owner.open(&c, SimStorage) {
            fail("Owner open failed");
        }
    }
    status(0);

    let stdin = std::io::stdin();
    let mut line = String::new();

    loop {
        line.clear();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => fail("Worker input failed"),
        }

        if !line.ends_with('\n') || line.len() > MAX_LINE {
            fail("Overlong worker command");
        }

        let command: Command = serde_json::from_str(&line)
            .unwrap_or_else(|_| fail("Unknown or invalid worker command"));

        // SAFETY: see `w`.
        let response = unsafe { execute(w(), command) };
        status(response);
    }
}

/// Run one command; the result is the status `response`.
///
/// # Safety
///
/// `world` is the live world (see `w`).
unsafe fn execute(world: *mut World, command: Command) -> u64 {
    // SAFETY: the caller's contract.
    unsafe {
        match command {
            Command::Submit {
                args: (id, portions),
            } => return submit(id, portions).index(),
            Command::Cancel { args: [id] } => return (*world).owner.cancel(id).into(),
            Command::Ack { args: [id] } => return (*world).owner.acknowledge(id).into(),

            Command::Tick { .. } => tick(),
            Command::Poll { .. } => poll(),
            Command::Run { args: [ms] } => run(ms.0),

            Command::Cut {
                args: (phase, mutations),
            } => {
                (*world).cut_phase = phase.0;
                (*world).cut_remaining = mutations.0.into();
            }
            Command::PowerCut { .. } => cut(),
            Command::Plant {
                args: (plant, position),
            } => {
                (*world).plant = Plant::new(plant.0, position);
                place_rotor();
            }
            Command::Jam { args: [jammed] } => (*world).jammed = jammed.0,
            Command::Wedge { args: [wedge] } => (*world).wedge = wedge,
            Command::Edge { args: [high] } => feedback(Flip {
                high: high.0,
                captured: true,
            }),
            Command::Write {
                args: (count, interval),
            } => {
                let writer = &raw mut (*world).writer;
                (*writer).remaining += count.0;
                (*writer).interval_ms = interval.0.into();
                (*writer).next_ms = (*world).device_ms;
            }

            Command::Owner { args: [run] } => (*world).run_owner = run.0,
            Command::Timer { args: [run] } => (*world).run_timer = run.0,
            Command::AdcStuck { args: [stuck] } => fake::with(|h| h.stuck = stuck.0),
            Command::StopFails { args: [fails] } => {
                fake::with(|h| h.blocked_motor_write = fails.0);
            }
            Command::Current { args: [level] } => (*world).held_current = Some(level.0),
            Command::Battery { args: [level] } => {
                (*world).battery_rest = level.0;
                fake::with(|h| h.battery_adc = level.0);
            }

            Command::Inputs { args: [bits] } => fake::with(|h| {
                let bits = bits.0;
                let mask = (1 << 16) | (1 << 17) | (1 << 19);
                h.input[0] = (h.input[0] & !mask)
                    | if bits & 1 != 0 { 1 << 16 } else { 0 }
                    | if bits & 2 != 0 { 1 << 17 } else { 0 }
                    | if bits & 4 != 0 { 1 << 19 } else { 0 };
            }),

            Command::FoodReads { args: [on] } => (*world).food_reads_on = on.0,
            Command::FoodHeld { args: [held] } => (*world).food_held = held.0,

            Command::Status { .. } => {}
            Command::Stop { .. } => {
                (*world).owner.stop();
                (*world).owner.step();
            }
        }
    }

    0
}
