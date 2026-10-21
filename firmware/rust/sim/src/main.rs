//! Virtual feeder worker: runs the real owner, board, events and watchdog
//! ports on the fake SDK with a file-backed synthetic journal.
//!
//! Usage: `virtual-feeder-worker JOURNAL MODE PROVISION`. Reads one command per
//! line on stdin and writes JSON-lines events on stdout; a power cut persists
//! the journal image and exits with status 75. Driven by
//! `tools/virtual-feeder.py`; the synthetic profile is not hardware calibration.

use core::ffi::{c_uint, c_void};
use core::ptr;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, Read, Seek, SeekFrom, Write};
use std::process::exit;

use petlibre_amebad::events::EventsConfig;
use petlibre_amebad::owner::{Owner, OwnerConfig};
use petlibre_amebad::sdk::fake;
use petlibre_amebad::watchdog::WatchdogConfig;
use petlibre_core::feed_controller::{FeedLimits, FeedRequest};
use petlibre_core::feed_journal::{FeedJournal, JournalConfig, JournalError, JournalIo};
use petlibre_core::feed_progress::{BowlMode, Direction};
use petlibre_core::feed_storage::{OwnerRecovery, OwnerResult, OwnerStorage};
use petlibre_core::flash_coord::{FlashCoord, FlashLimits, FlashReply};
use petlibre_core::sensor_inputs::SensorLimits;
use petlibre_testkit::journal_flash::{FakeJournalFlash, UNIT};

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
    auto_period: u32,
    plant_ms: u32,
    previous_motor: u32,
    feedback_high: bool,
    cut_phase: u32,
    active_phase: u32,
    phase_start_mutations: u32,
    cut_remaining: u64,
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
            println!(
                r#"{{"event":"outputs","device_ms":{},"motor":{m},"optics":{o}}}"#,
                (*w()).device_ms
            );
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
        println!(
            r#"{{"event":"power_cut","device_ms":{},"phase":{},"mutations":{}}}"#,
            (*w()).device_ms,
            (*w()).active_phase,
            (*flash_ptr()).mutations
        );
    }
    let _ = std::io::stdout().flush();
    exit(75);
}

fn mutation(_: *mut c_void) {
    save();
    // SAFETY: see `w`.
    unsafe {
        let world = w();
        if (*world).active_phase != 0 && (*world).cut_phase == (*world).active_phase {
            (*world).cut_remaining = (*world).cut_remaining.wrapping_sub(1);
            if (*world).cut_remaining == 0 {
                cut();
            }
        }
    }
}

fn io_event(operation: &str, stage: &str, slot: c_uint, offset: u32, size: usize, ok: bool) {
    // SAFETY: see `w`.
    let device_ms = unsafe { (*w()).device_ms };
    println!(
        r#"{{"event":"journal_io","device_ms":{device_ms},"operation":"{operation}","stage":"{stage}","slot":{slot},"offset":{offset},"bytes":{size},"ok":{ok}}}"#
    );
}

unsafe extern "C" fn read_io(
    context: *mut c_void,
    slot: c_uint,
    offset: u32,
    bytes: *mut c_void,
    size: usize,
) -> bool {
    let read = unsafe { (*w()).base_io.read.unwrap() };
    let ok = unsafe { read(context, slot, offset, bytes, size) };
    io_event("read", "end", slot, offset, size, ok);
    ok
}

unsafe extern "C" fn program_io(
    context: *mut c_void,
    slot: c_uint,
    offset: u32,
    bytes: *const c_void,
    size: usize,
) -> bool {
    assert!(motor() == 0 && optics() == 0);
    io_event("program", "begin", slot, offset, size, false);
    let program = unsafe { (*w()).base_io.program.unwrap() };
    let ok = unsafe { program(context, slot, offset, bytes, size) };
    io_event("program", "end", slot, offset, size, ok);
    ok
}

unsafe extern "C" fn erase_io(context: *mut c_void, slot: c_uint) -> bool {
    assert!(motor() == 0 && optics() == 0);
    io_event("erase", "begin", slot, 0, UNIT, false);
    let erase = unsafe { (*w()).base_io.erase.unwrap() };
    let ok = unsafe { erase(context, slot) };
    io_event("erase", "end", slot, 0, UNIT, ok);
    ok
}

fn storage_begin(phase: u32) {
    assert!(motor() == 0 && optics() == 0);
    // SAFETY: see `w`.
    unsafe {
        let world = w();
        (*world).active_phase = phase;
        (*world).phase_start_mutations = (*flash_ptr()).mutations;
        println!(
            r#"{{"event":"storage_begin","device_ms":{},"phase":{phase}}}"#,
            (*world).device_ms
        );
        if (*world).cut_phase == phase && (*world).cut_remaining == 0 {
            cut();
        }
    }
}

fn storage_end(ok: bool) {
    // SAFETY: see `w`.
    unsafe {
        let world = w();
        println!(
            r#"{{"event":"storage_end","device_ms":{},"phase":{},"mutations":{},"ok":{ok}}}"#,
            (*world).device_ms,
            (*world).active_phase,
            (*flash_ptr()).mutations - (*world).phase_start_mutations
        );
        (*world).active_phase = 0;
    }
}

fn journal_storage() -> OwnerStorage {
    // SAFETY: see `w`; the journal lives in the leaked world.
    unsafe { (*w()).journal.storage() }
}

unsafe extern "C" fn recover(_: *mut c_void, value: *mut OwnerRecovery) -> bool {
    storage_begin(3);
    let s = journal_storage();
    // SAFETY: the journal storage callbacks with a live output pointer.
    let ok = unsafe { (*w()).opened && s.recover.unwrap()(s.context, value) };
    storage_end(ok);
    ok
}

unsafe extern "C" fn reserve(_: *mut c_void, request: *const FeedRequest) -> bool {
    storage_begin(1);
    let s = journal_storage();
    // SAFETY: as for `recover`.
    let ok = unsafe { s.reserve.unwrap()(s.context, request) };
    storage_end(ok);
    ok
}

unsafe extern "C" fn finish(
    _: *mut c_void,
    result: *const OwnerResult,
    phase: *const OwnerRecovery,
) -> bool {
    storage_begin(2);
    let s = journal_storage();
    // SAFETY: as for `recover`.
    let ok = unsafe { s.finish.unwrap()(s.context, result, phase) };
    storage_end(ok);
    ok
}

fn status(response: i64) {
    // SAFETY: see `w`.
    unsafe {
        let world = w();
        let s = (*world).owner.status();
        let opened = (*world).opened;
        let view = (*world).journal.view;
        println!(
            concat!(
                r#"{{"event":"status","device_ms":{},"response":{},"state":{},"fault":{},"supervisor_fault":{},"watchdog_fault":{}"#,
                r#","id":{},"completed":{},"outcome":{},"reason":{}"#,
                r#","durable":{},"stopped":{},"optics_off":{},"phase_known":{}"#,
                r#","high_water_id":{},"pending":{},"journal_error":{}"#,
                r#","motor":{},"optics":{},"watchdog_resets":{}}}"#
            ),
            (*world).device_ms,
            response,
            s.state.0,
            s.fault.0,
            s.supervisor_fault.0,
            s.watchdog_fault.0,
            s.result.id,
            s.result.completed,
            s.result.outcome.0,
            s.result.reason.0,
            s.result_durable,
            s.stopped,
            s.optics_off,
            s.phase_known,
            if opened {
                view.recovery.high_water_id
            } else {
                0
            },
            opened && view.recovery.pending_request,
            (*world).journal.error.0,
            motor(),
            optics(),
            fake::with(|h| h.watchdog_reset_requests),
        );
    }
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
                println!(
                    r#"{{"event":"feedback","device_ms":{},"high":{high}}}"#,
                    (*world).device_ms
                );
            }
        }
        (*world).previous_motor = m;
        if (*world).run_owner && fake::with(|h| h.task_live) {
            fake::run_task();
        }
    }
}

fn number(text: Option<&str>) -> u64 {
    match text {
        Some(s) if !s.is_empty() && !s.starts_with('-') => s
            .parse()
            .unwrap_or_else(|_| fail("Invalid unsigned integer")),
        _ => fail("Expected unsigned integer"),
    }
}

fn open_disk(path: &str, provision: bool) -> File {
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
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        fail("worker JOURNAL MODE PROVISION");
    }
    let (mode, provision) = (number(Some(&args[2])), number(Some(&args[3])));
    if mode > 1 || provision > 1 {
        fail("Invalid mode or provision flag");
    }
    let provision = provision == 1;
    let mut disk = open_disk(&args[1], provision);
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
        cut_phase: 0,
        active_phase: 0,
        phase_start_mutations: 0,
        cut_remaining: 0,
        observed_motor: 99,
        observed_optics: 99,
    }));
    // SAFETY: the world is leaked for the process lifetime; single-threaded.
    unsafe {
        WORLD = world;
        let io = FakeJournalFlash::io_at(flash_ptr());
        let jc = JournalConfig {
            binding: 17,
            mode: BowlMode(mode as u32),
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
        (*world).authority = FlashCoord::new(&limits, 17, 0).unwrap();
        assert_eq!(
            (*world).authority.ready(17, 29, true, 0),
            FlashReply::ACCEPTED
        );
        let c = OwnerConfig {
            bowl_mode: BowlMode(mode as u32),
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
        let mut words = line
            .split([' ', '\t', '\r', '\n'])
            .filter(|w| !w.is_empty());
        let op = words.next().unwrap_or_else(|| fail("Empty worker command"));
        let (a, b) = (words.next(), words.next());
        if words.next().is_some() {
            fail("Too many arguments");
        }
        let x = a.map_or(0, |s| number(Some(s)));
        let y = b.map_or(0, |s| number(Some(s)));
        let (a, b) = (a.is_some(), b.is_some());
        let mut response = 0i64;
        // SAFETY: see `w`.
        unsafe {
            let world = w();
            match op {
                "submit" if a && b && y <= u64::from(u16::MAX) => {
                    response = i64::from((*world).owner.submit(x, y as u16).0)
                }
                "cancel" if a && !b => response = i64::from((*world).owner.cancel(x)),
                "ack" if a && !b => response = i64::from((*world).owner.acknowledge(x)),
                "tick" if !a => tick(),
                "poll" if !a => poll(),
                "cut" if a && b && (1..=2).contains(&x) => {
                    (*world).cut_phase = x as u32;
                    (*world).cut_remaining = y;
                }
                "power_cut" if !a => cut(),
                "auto" if a && !b && x <= 100_000 => {
                    (*world).auto_period = x as u32;
                    (*world).plant_ms = 0;
                }
                "edge" if a && !b && x <= 1 => {
                    (*world).feedback_high = x != 0;
                    fake::edge(x != 0);
                }
                "owner" if a && !b && x <= 1 => (*world).run_owner = x != 0,
                "timer" if a && !b && x <= 1 => (*world).run_timer = x != 0,
                "adc_stuck" if a && !b && x <= 1 => fake::with(|h| h.stuck = x != 0),
                "current" if a && !b && x <= 4095 => fake::with(|h| h.current_adc = x as u16),
                "battery" if a && !b && x <= 4095 => fake::with(|h| h.battery_adc = x as u16),
                "inputs" if a && !b && x <= 7 => fake::with(|h| {
                    let mask = (1 << 16) | (1 << 17) | (1 << 19);
                    h.input[0] = (h.input[0] & !mask)
                        | if x & 1 != 0 { 1 << 16 } else { 0 }
                        | if x & 2 != 0 { 1 << 17 } else { 0 }
                        | if x & 4 != 0 { 1 << 19 } else { 0 };
                }),
                "status" if !a => {}
                "stop" if !a => {
                    (*world).owner.stop();
                    if fake::with(|h| h.task_live) {
                        fake::run_task();
                    }
                }
                _ => fail("Unknown or invalid worker command"),
            }
        }
        status(response);
    }
}
