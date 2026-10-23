//! Virtual feeder worker: runs the real owner, board, events and watchdog
//! ports on the fake SDK with a file-backed synthetic journal.
//!
//! Usage: `virtual-feeder-worker JOURNAL PROFILE provision|recover`. Reads one
//! JSON [`Command`] per line on stdin and writes JSON-lines [`Event`]s on
//! stdout (the protocol in `wellfed_sim::protocol`); a power cut persists the
//! journal image and exits with status 75. Driven by the `virtual-feeder`
//! harness in this crate; the synthetic profile is not hardware calibration.

use core::ffi::{c_uint, c_void};
use core::ptr;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::exit;

use clap::Parser;

use wellfed_core::feed::controller::{FeedLimits, FeedRequest};
use wellfed_core::feed::progress::{BowlMode, Direction};
use wellfed_core::feed::sensor_inputs::SensorLimits;
use wellfed_core::feed::storage::{OwnerRecovery, OwnerResult, OwnerStorage};
use wellfed_core::flash_coord::{FlashCoord, FlashLimits, FlashReply};
use wellfed_core::store::feed_journal::{FeedJournal, JournalConfig, JournalError};
use wellfed_core::store::slot::JournalIo;
use wellfed_hal::owner::{Owner, OwnerConfig};
use wellfed_hal::sdk::fake;
use wellfed_hal::supervision::events::EventsConfig;
use wellfed_hal::supervision::watchdog::WatchdogConfig;
use wellfed_sim::protocol::{
    Code, Command, Event, Fault, IoOperation, IoStage, Journal, Outcome, Profile, Reason, State,
    Status, StoragePhase, WorkerArgs,
};
use wellfed_testkit::journal_flash::{FakeJournalFlash, UNIT};

/// Synthetic profile copied from the owner tests; not hardware calibration.
fn config() -> OwnerConfig {
    OwnerConfig {
        feed: FeedLimits {
            feedback_inhibit_ms: 5,
            feedback_timeout_ms: 30,
            direction_pause_ms: 3,
            reverse_ms: 3,
            max_duration_ms: 120,
            max_observation_gap_ms: 4,
            current_limit_ma: 300,
            max_portions: 3,
            max_reverse_attempts: 0,
        },

        sensors: SensorLimits {
            optical_settle_ms: 2,
            food_empty_delay_ms: 5,
            outlet_block_delay_ms: 5,
            power_settle_ms: 2,
            max_observation_gap_ms: 4,
            max_age_ms: 2,
            battery_min_scaled: 1650,
        },

        events: EventsConfig {
            ticks_per_second: 32_000,
            timer_period_ms: 2,
            max_feedback_age_ms: 2,
            feedback_capacity: 8,
        },

        watchdog: WatchdogConfig {
            systimer_hz: 32_000,
            clock_min_hz: 32_000,
            clock_max_hz: 32_000,
            min_refresh_ms: 1,
            max_progress_gap_ms: 50,
            max_expiry_ms: 126,
            count_id: 5,
            divisor: 63,
        },

        bowl_mode: BowlMode::SINGLE,
        sample_period_ms: 1,
        prepare_timeout_ms: 20,
        optics_max_on_ms: 140,
        motor_deadtime_ms: 2,
        motor_lease_ms: 12,
        adc_poll_limit: 8,
        wake_lock_id: 16,
        flash: ptr::null_mut(),
    }
}

/// Process-wide simulation state, reached from SDK/journal callbacks.
struct World {
    owner: Owner,
    journal: FeedJournal,
    flash: FakeJournalFlash,
    base_io: JournalIo,
    authority: FlashCoord,
    disk: File,

    device_ms: u64,
    opened: bool,
    run_owner: bool,
    run_timer: bool,

    // Synthetic feedback plant: toggles the feedback edge every `auto_period` ms of drive.
    auto_period: u32,
    plant_ms: u32,
    previous_motor: u32,
    feedback_high: bool,

    // Power-cut injection: cut after `cut_remaining` more mutations in storage phase
    // `cut_phase` (or at that phase's start when the count is already zero).
    cut_phase: StoragePhase,
    active_phase: StoragePhase,
    phase_start_mutations: u32,
    cut_remaining: u64,

    // Last reported outputs, so the hook only prints changes.
    observed_motor: u32,
    observed_optics: u32,
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
    // SAFETY: see `w`.
    let device_ms = unsafe { (*w()).device_ms };
    emit(&Event::JournalIo {
        device_ms,
        operation,
        stage,
        slot,
        offset,
        bytes: size,
        ok,
    });
}

unsafe extern "C" fn read_io(
    context: *mut c_void,
    slot: c_uint,
    offset: u32,
    bytes: *mut c_void,
    size: usize,
) -> bool {
    // SAFETY: the worker's world outlives every journal callback.
    let read = unsafe { (*w()).base_io.read }.expect("the base flash reads");
    // SAFETY: forwards the journal's own arguments to the base flash.
    let ok = unsafe { read(context, slot, offset, bytes, size) };
    io_event(IoOperation::Read, IoStage::End, slot, offset, size, ok);
    ok
}

unsafe extern "C" fn program_io(
    context: *mut c_void,
    slot: c_uint,
    offset: u32,
    bytes: *const c_void,
    size: usize,
) -> bool {
    // Journal writes must only happen with the motor and optics off; the worker
    // fails loudly if the owner ever programs or erases flash while either is on.
    assert!(motor() == 0 && optics() == 0);
    io_event(
        IoOperation::Program,
        IoStage::Begin,
        slot,
        offset,
        size,
        false,
    );
    // SAFETY: the worker's world outlives every journal callback.
    let program = unsafe { (*w()).base_io.program }.expect("the base flash programs");
    // SAFETY: forwards the journal's own arguments to the base flash.
    let ok = unsafe { program(context, slot, offset, bytes, size) };
    io_event(IoOperation::Program, IoStage::End, slot, offset, size, ok);
    ok
}

unsafe extern "C" fn erase_io(context: *mut c_void, slot: c_uint) -> bool {
    assert!(motor() == 0 && optics() == 0);
    io_event(IoOperation::Erase, IoStage::Begin, slot, 0, UNIT, false);
    // SAFETY: the worker's world outlives every journal callback.
    let erase = unsafe { (*w()).base_io.erase }.expect("the base flash erases");
    // SAFETY: forwards the journal's own arguments to the base flash.
    let ok = unsafe { erase(context, slot) };
    io_event(IoOperation::Erase, IoStage::End, slot, 0, UNIT, ok);
    ok
}

/// The `cut` command can only target reserve or finish.
fn storage_begin(phase: StoragePhase) {
    assert!(motor() == 0 && optics() == 0);
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

fn journal_storage() -> OwnerStorage {
    // SAFETY: see `w`; the journal lives in the leaked world.
    unsafe { (*w()).journal.storage() }
}

unsafe extern "C" fn recover(_: *mut c_void, value: *mut OwnerRecovery) -> bool {
    storage_begin(StoragePhase::Recover);
    let s = journal_storage();
    // SAFETY: the journal storage callbacks with a live output pointer.
    let ok =
        unsafe { (*w()).opened && s.recover.expect("journal storage recovers")(s.context, value) };
    storage_end(ok);
    ok
}

unsafe extern "C" fn reserve(_: *mut c_void, request: *const FeedRequest) -> bool {
    storage_begin(StoragePhase::Reserve);
    let s = journal_storage();
    // SAFETY: as for `recover`.
    let ok = unsafe { s.reserve.expect("journal storage reserves")(s.context, request) };
    storage_end(ok);
    ok
}

unsafe extern "C" fn finish(
    _: *mut c_void,
    result: *const OwnerResult,
    phase: *const OwnerRecovery,
) -> bool {
    storage_begin(StoragePhase::Finish);
    let s = journal_storage();
    // SAFETY: as for `recover`.
    let ok = unsafe { s.finish.expect("journal storage finishes")(s.context, result, phase) };
    storage_end(ok);
    ok
}

fn status(response: u64) {
    // SAFETY: see `w`.
    let status = unsafe {
        let world = w();
        let s = (*world).owner.status();
        let opened = (*world).opened;
        let view = (*world).journal.view;

        Status {
            device_ms: (*world).device_ms,
            response,
            state: code::<State>(s.state.0),
            fault: code::<Fault>(s.fault.0),
            supervisor_fault: s.supervisor_fault.0,
            watchdog_fault: s.watchdog_fault.0,
            id: s.result.id,
            completed: s.result.completed,
            outcome: code::<Outcome>(s.result.outcome.0),
            reason: code::<Reason>(s.result.reason.0),
            durable: s.result_durable,
            stopped: s.stopped,
            optics_off: s.optics_off,
            phase_known: s.phase_known,
            high_water_id: if opened {
                view.recovery.high_water_id
            } else {
                0
            },
            pending: opened && view.recovery.pending_request,
            journal_error: (*world).journal.error.0,
            motor: motor(),
            optics: optics(),
            watchdog_resets: fake::with(|h| h.watchdog_reset_requests),
        }
    };

    emit(&Event::Status(status));
}

fn tick() {
    // SAFETY: see `w`.
    let run_timer = unsafe {
        (*w()).device_ms += 1;
        (*w()).run_timer
    };
    fake::advance(32, run_timer);
}

fn poll() {
    let m = motor();
    // SAFETY: see `w`.
    unsafe {
        let world = w();
        if m == 0 || m == 3 || m != (*world).previous_motor {
            (*world).plant_ms = 0;
        }

        if m != 0 && m != 3 && (*world).auto_period != 0 {
            (*world).plant_ms += 1;
            if (*world).plant_ms >= (*world).auto_period {
                (*world).plant_ms = 0;
                (*world).feedback_high = !(*world).feedback_high;
                let high = (*world).feedback_high;
                fake::edge(high);
                emit(&Event::Feedback {
                    device_ms: (*world).device_ms,
                    high,
                });
            }
        }

        (*world).previous_motor = m;
        if (*world).run_owner && fake::with(|h| h.task_live) {
            fake::run_task();
        }
    }
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
        Profile::Single => BowlMode::SINGLE,
        Profile::Double => BowlMode::DOUBLE,
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
        journal: FeedJournal::default(),
        flash,
        base_io: JournalIo::default(),
        authority: FlashCoord::default(),
        disk,
        device_ms: 0,
        opened: false,
        run_owner: true,
        run_timer: true,
        auto_period: 10,
        plant_ms: 0,
        previous_motor: 0,
        feedback_high: false,
        cut_phase: StoragePhase::None,
        active_phase: StoragePhase::None,
        phase_start_mutations: 0,
        cut_remaining: 0,
        // Out of range, so the first output hook always reports.
        observed_motor: 99,
        observed_optics: 99,
    }));

    // SAFETY: the world is leaked for the process lifetime; single-threaded.
    unsafe {
        WORLD = world;
        let io = FakeJournalFlash::io_at(flash_ptr());
        let jc = JournalConfig {
            binding: 17,
            mode,
            max_portions: 3,
        };

        if provision {
            if (*world).journal.create(io, jc, Direction::D0, true, true) != JournalError::OK {
                fail("Cannot provision synthetic journal");
            }
            save();
        }

        (*world).base_io = io;
        let wrapped = JournalIo {
            read: Some(read_io),
            program: Some(program_io),
            erase: Some(erase_io),
            ..io
        };
        (*world).opened = (*world).journal.open(wrapped, jc) == JournalError::OK;

        fake::reset_hardware();
        fake::with(|h| h.output_hook = Some(outputs));

        let limits = FlashLimits {
            // The whole lease must fit inside the 50 ms watchdog progress gap.
            quiesce_ms: 10,
            park_ms: 10,
            active_ms: 20,
            release_ms: 10,
            total_ms: 40,
        };
        (*world).authority =
            FlashCoord::new(&limits, 17, 0).expect("the simulated limits are valid");
        assert_eq!(
            (*world).authority.ready(17, 29, true, 0),
            FlashReply::ACCEPTED
        );

        let c = OwnerConfig {
            bowl_mode: mode,
            flash: &raw mut (*world).authority,
            ..config()
        };

        let storage = OwnerStorage {
            context: ptr::null_mut(),
            recover: Some(recover),
            reserve: Some(reserve),
            finish: Some(finish),
        };

        if !(*world).owner.start(&c, &storage) {
            fail("Owner start failed");
        }
    }

    fake::run_task();
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

        if !line.ends_with('\n') || line.len() > 255 {
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
            } => return (*world).owner.submit(id, portions).0.into(),
            Command::Cancel { args: [id] } => return (*world).owner.cancel(id).into(),
            Command::Ack { args: [id] } => return (*world).owner.acknowledge(id).into(),

            Command::Tick { .. } => tick(),
            Command::Poll { .. } => poll(),

            Command::Cut {
                args: (phase, mutations),
            } => {
                (*world).cut_phase = StoragePhase::from_index(phase.0.into())
                    .unwrap_or_else(|| fail("Invalid cut phase"));
                (*world).cut_remaining = mutations.0.into();
            }
            Command::PowerCut { .. } => cut(),
            Command::Auto { args: [period] } => {
                (*world).auto_period = period.0;
                (*world).plant_ms = 0;
            }
            Command::Edge { args: [high] } => {
                (*world).feedback_high = high.0;
                fake::edge(high.0);
            }

            Command::Owner { args: [run] } => (*world).run_owner = run.0,
            Command::Timer { args: [run] } => (*world).run_timer = run.0,
            Command::AdcStuck { args: [stuck] } => fake::with(|h| h.stuck = stuck.0),
            Command::Current { args: [level] } => fake::with(|h| h.current_adc = level.0),
            Command::Battery { args: [level] } => fake::with(|h| h.battery_adc = level.0),

            Command::Inputs { args: [bits] } => fake::with(|h| {
                let bits = bits.0;
                let mask = (1 << 16) | (1 << 17) | (1 << 19);
                h.input[0] = (h.input[0] & !mask)
                    | if bits & 1 != 0 { 1 << 16 } else { 0 }
                    | if bits & 2 != 0 { 1 << 17 } else { 0 }
                    | if bits & 4 != 0 { 1 << 19 } else { 0 };
            }),

            Command::Status { .. } => {}
            Command::Stop { .. } => {
                (*world).owner.stop();
                if fake::with(|h| h.task_live) {
                    fake::run_task();
                }
            }
        }
    }

    0
}
