//! The KM4 runtime every firmware runs inside.
//!
//! Startup: stopped outputs, the synthetic core diagnostic and the indicator
//! check, then the network join (the Wi-Fi probe, stepped by the
//! diagnostic task). The firmware's own tasks start (`firmware!`) from
//! either bank, then the service task: it samples power, confirms a healthy
//! build with the feeder (bank 1 only), opens the settings writer, and runs
//! the update stage (`ota::update`), which takes offers over the firmware's
//! device API. Booted from bank 0, the build is the fallback a failed update
//! lands on: it never confirms, opens the settings writer at once, so it
//! saves and feeds, and installs offers into bank 1. Nothing opens an MQTT
//! session. A hung runtime task stops the feeding, so a hang becomes a
//! watchdog reset that KM0 counts.

use core::ffi::{CStr, c_void};
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use wellfed_core::flash_coord::FlashCoord;
use wellfed_core::{deadline_passed, until};
use wellfed_hal::flash::hooks::{self, HookProvider, HookTicket};
use wellfed_hal::flash::park::Hardware;
use wellfed_hal::flash_lock::arming::Hooks;
use wellfed_hal::flash_lock::provider::{LeaseFault, Provider};
use wellfed_hal::ota::confirm::{self, Confirm, HookedStorage};
use wellfed_hal::ota::update::{self as update, Floor, SdkFlash, StoreSlots, Update};
use wellfed_hal::supervision::beat;
use wellfed_hal::supervision::supervise::{self, LIVENESS_MS, Startup};
use wellfed_hal::supervision::watchdog::Watchdog;
use wellfed_hal::supervision::{self, config, crumb};
use wellfed_hal::{cell::TaskCell, leds, net_status, power, rng, sdk, startup, stopped, trial};

use crate::slot::Slot;
use crate::wifi::{Probe, Record};

unsafe extern "C" {
    /// The firmware's start hook, defined by `firmware!` in the firmware
    /// crate. A firmware that defines none does not link.
    fn wellfed_firmware_start();
}

unsafe extern "Rust" {
    /// One step of the settings writer, which owns the device's record
    /// stores (settings, Wi-Fi, keys, events): it arms the flash hooks with
    /// `provider` (through [`SdkHooks`]) the first time the build is
    /// unarmed, logs its own `PLCFG1` records, and returns the delay to its
    /// next step, 0 to end. The store schema is the product's, so the
    /// product's runtime crate defines it and every firmware links that
    /// crate; a firmware that links none does not link. The definition
    /// (`wellfed_feeder_runtime::writer`) must keep this signature: the
    /// linker does not compare Rust signatures, so it pins its own.
    ///
    /// # Safety
    /// Called only from the service task; `provider` is the runtime's fixed
    /// provider, never borrowed.
    fn wellfed_store_step(provider: *mut Provider) -> u32;
}

/// Creates one of the firmware's tasks. `step` runs, then the task sleeps
/// for the milliseconds it returned; zero ends the task. Firmware tasks
/// start only in the normal role, after the startup checks passed, and come
/// from the firmware's budget of [`FIRMWARE_TASKS`](crate::FIRMWARE_TASKS):
/// past it, `spawn` refuses rather than take a runtime task's slot.
///
/// # Safety
/// `step` must be sound to call from the new task with a null context, again
/// and again, until it returns zero.
pub unsafe fn spawn(
    name: &'static CStr,
    stack_words: u16,
    priority: u32,
    step: sdk::TaskStep,
) -> bool {
    if !crate::tasks::reserve() {
        sdk::log(c"PLFW1 error firmware_tasks=full\r\n");
        return false;
    }

    // SAFETY: the caller's contract, with no context.
    let created = unsafe { sdk::task_create(name, stack_words, priority, step, ptr::null_mut()) };
    if !created {
        crate::tasks::release();
    }
    created
}

/// The firmware's startup work that holds the KM0 flash handshake (an
/// optics or pulse probe) has ended, so another flash writer may
/// bind it. The runtime does not confirm the build before this is reported.
/// A firmware with no such work reports it from its start hook.
pub fn startup_done(done: bool) {
    PROBE_DONE.store(done, Ordering::Release);
}

/// Stops feeding the watchdog, so the platform resets and KM0 counts it,
/// after the Wi-Fi task left the network. For reboots the device API or
/// setup asks for, and trial tests that must behave like a hung candidate.
pub fn request_reset() {
    restart();
}

/// Every intentional restart: stop feeding the watchdog, and have the
/// Wi-Fi task leave the network meanwhile (`supervision::stopping`). The
/// feeding stops at once either way: a leave that hangs cannot hold the
/// reset.
fn restart() {
    supervision::stopping();
    RESTART.store(true, Ordering::Release);
}

/// Whether the feeder opened the watchdog at startup.
pub fn watchdog_open() -> bool {
    WATCHDOG_OPEN.load(Ordering::Acquire)
}

/// The firmware keeps the ADC for itself (a probe, or a feeding owner), so
/// the runtime's power monitor never opens it. Health reports power as
/// unknown unless the firmware publishes it: a feeding owner does, from its
/// captures. Call from the start hook.
pub fn claim_adc() {
    ADC_CLAIMED.store(true, Ordering::Release);
}

static ADC_CLAIMED: AtomicBool = AtomicBool::new(false);
/// The service task's power stage; see `power_step`.
static POWER: Slot<power::Monitor> = Slot::new();

struct PowerLine(power::Record);

impl core::fmt::Display for PowerLine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        power::describe(self.0, f)
    }
}

/// Samples the battery and adapter for health. It waits for the
/// startup probe to release its board, which owns the ADC until then.
fn power_step() -> u32 {
    static LOGGED: AtomicBool = AtomicBool::new(false);
    if ADC_CLAIMED.load(Ordering::Acquire) {
        if !LOGGED.swap(true, Ordering::Relaxed) {
            sdk::log(c"PLBAT1 claimed\r\n");
        }
        // Idle rather than end: the service task exits once no stage runs,
        // and confirmation starts only after this, so ending here left
        // confirmation unstepped until the watchdog reset the trial.
        return power::SAMPLE_MS;
    }
    if !PROBE_DONE.load(Ordering::Acquire) {
        return POWER_WAIT_MS;
    }

    let Some(mut monitor) = POWER.enter_or_init(Default::default) else {
        return 0;
    };
    let step = monitor.step();
    if let Some(record) = step.record {
        let mut report = startup::Report::new();
        if let Ok(line) = report.record(format_args!("PLBAT1 {}\r\n", PowerLine(record))) {
            sdk::log(line);
        }
    }
    step.delay_ms
}

const POWER_WAIT_MS: u32 = 1_000;

static PROBING: AtomicBool = AtomicBool::new(false);
static PROBE: Slot<Probe> = Slot::new();

/// The lease provider the SDK flash-lock hooks call. Never borrowed: every
/// task passes its raw pointer, because the armed hooks dereference it from
/// whichever task holds the SDK flash lock. Arming writes it from one task, in
/// sequence: the service task's confirmation, then its settings writer if
/// the hooks are still unarmed (bank 0 never confirms).
static PROVIDER: TaskCell<Provider> = TaskCell::new(Provider::UNBOUND);
/// Opened by whichever task arms first (the feeder task at startup), then
/// handed to the feeder task, which feeds it.
static WATCHDOG: Slot<Watchdog> = Slot::new();
static WATCHDOG_OPEN: AtomicBool = AtomicBool::new(false);

/// The confirmation's last step, in uptime milliseconds.
static HEARTBEAT: AtomicU32 = AtomicU32::new(0);
/// The confirmation ended, or never runs (bank 0): feed while outputs stay
/// stopped.
static CONFIRM_DONE: AtomicBool = AtomicBool::new(false);
/// Bank 1 gave way or was activated: stop feeding so the watchdog resets
/// the platform.
static RESTART: AtomicBool = AtomicBool::new(false);
static UPDATE_RUNNING: AtomicBool = AtomicBool::new(false);
/// The startup optics (or pulse) probe has ended and released the KM0
/// handshake, so another writer may bind it.
static PROBE_DONE: AtomicBool = AtomicBool::new(false);

const FEED_MS: u32 = 250;
// Every checkpoint refreshes the hardware: `fed_ms` relies on it.
const _: () = assert!(FEED_MS > config::WATCHDOG.min_refresh_ms);

/// Supervision state for the feeder (`supervise.rs`), as `Startup::code`.
static STARTUP: AtomicU8 = AtomicU8::new(Startup::Running.code());
/// The diagnostic task's last step, while startup runs.
static STARTUP_BEAT: AtomicU32 = AtomicU32::new(0);
/// Bank 1 without a confirmation record for this build.
static TRIAL: AtomicBool = AtomicBool::new(false);
/// This boot confirmed the build.
static CONFIRMED: AtomicBool = AtomicBool::new(false);

/// Startup stopped on a determined failure; ends the diagnostic task.
fn startup_failed() -> u32 {
    STARTUP.store(Startup::Failed.code(), Ordering::Release);
    0
}

/// Running bank 1 with no confirmation record naming this build.
fn on_trial() -> bool {
    let mut record = [0u8; wellfed_core::ota::CONFIRM_LEN];
    startup::snapshot().bank() == Some(1)
        && sdk::storage_read(trial::CONFIRM_SECTOR, 0, &mut record)
        && wellfed_core::ota::confirmed(&record, &trial::identity()).is_none()
}
unsafe extern "C" fn hook_ready(context: *mut c_void) -> bool {
    // SAFETY: the context is the fixed provider; called with no lease held.
    unsafe { (*context.cast::<Provider>()).ready() }
}

unsafe extern "C" fn hook_acquire(context: *mut c_void, ticket: *mut HookTicket) -> bool {
    // SAFETY: as above; the interposer passes a live ticket.
    unsafe { (*context.cast::<Provider>()).acquire::<Hardware>(&mut *ticket) }
}

unsafe extern "C" fn hook_release(context: *mut c_void, ticket: *const HookTicket) -> bool {
    // SAFETY: as above.
    unsafe { (*context.cast::<Provider>()).release::<Hardware>(&*ticket) }
}

/// KM4 never parks for KM0: a flash IPC request from KM0 is a hook fault.
unsafe extern "C" fn hook_peer(_context: *mut c_void, _status: u32, _channel: u32) -> bool {
    false
}

/// The SDK flash-lock hooks, as the runtime installs and arms them.
pub struct SdkHooks;

impl Hooks for SdkHooks {
    fn watchdog() -> bool {
        if WATCHDOG_OPEN.load(Ordering::Acquire) {
            return true; // The normal role opened it at startup.
        }

        let opened = WATCHDOG
            .enter_or_init(Watchdog::default)
            .is_some_and(|mut watchdog| watchdog.open(&config::WATCHDOG));
        // The feeder task feeds what this task opened.
        WATCHDOG.release();
        WATCHDOG_OPEN.store(opened, Ordering::Release);
        opened
    }

    fn install(provider: *mut Provider) -> bool {
        let callbacks = HookProvider {
            context: provider.cast(),
            ready: Some(hook_ready),
            acquire: Some(hook_acquire),
            release: Some(hook_release),
            peer: Some(hook_peer),
            fault: Some(hooks::wellfed_amebad_flash_provider_fault),
        };
        // SAFETY: the interposer copies the callbacks; the provider is static.
        unsafe { hooks::wellfed_amebad_flash_hooks_install(&callbacks) }
    }

    fn armed() -> bool {
        hooks::wellfed_amebad_flash_hooks_armed()
    }
}

/// Opens the watchdog on its first step, before the startup diagnostic and
/// probes run, then feeds it as `supervise::feed` decides: while startup and
/// then the role's tasks keep stepping and outputs stay stopped. A hang, a
/// hook fault (IRQs masked), an activation, or a trial image that fails or
/// never confirms all stop the feeding, and the watchdog resets the
/// platform, which KM0's boot trial counts.
unsafe extern "C" fn feeder_task(_: *mut c_void) -> u32 {
    static PROGRESS: AtomicU32 = AtomicU32::new(0);
    static STOPPED: AtomicBool = AtomicBool::new(false);
    // Uptime of the last checkpoint. Each one refreshes the hardware (the
    // feed period exceeds the refresh rate limit), so a reset's host
    // timestamp, less this, times the watchdog's expiry.
    static FED_MS: AtomicU32 = AtomicU32::new(0);
    // Every pass, fed or not: the tick hook's crumb counts from here.
    crumb::fed();
    if !WATCHDOG_OPEN.load(Ordering::Acquire) {
        let trial = on_trial();
        TRIAL.store(trial, Ordering::Release);
        let opened = SdkHooks::watchdog();
        let mut report = startup::Report::new();
        if let Ok(line) = report.record(format_args!(
            "PLSUP1 open watchdog={} trial={}\r\n",
            bit(opened),
            bit(trial)
        )) {
            sdk::log(line);
        }
        if !opened {
            return 0; // Unsupervised: nothing to feed.
        }
    }

    let now = sdk::uptime_ms();
    let fresh = |beat: &AtomicU32| now.wrapping_sub(beat.load(Ordering::Acquire)) < LIVENESS_MS;
    let alive = (!SERVICE_RUNNING.load(Ordering::Acquire) || fresh(&SERVICE_BEAT))
        && (CONFIRM_DONE.load(Ordering::Acquire) || fresh(&HEARTBEAT));

    // Stopped, or motor STOP with exactly the supervised emitters lit (the
    // optics probe): the feeder must not trip on a sensing window (observed).
    let outputs = stopped::observe();
    let inputs = supervise::Inputs {
        now_ms: now,
        startup: Startup::from_code(STARTUP.load(Ordering::Acquire)),
        startup_beat_ms: STARTUP_BEAT.load(Ordering::Acquire),
        trial: TRIAL.load(Ordering::Acquire),
        confirmed: CONFIRMED.load(Ordering::Acquire),
        tasks_alive: alive,
        outputs_ok: outputs.verified() || outputs.sensing(),
        owner: beat::read(),
    };
    let feeding = supervise::feed(&inputs);
    // A supervision stop (a trial's deadline, a hung task) leaves the
    // network as a restart does, but only once it has outlasted the
    // progress gap and the reset is certain.
    supervision::supervised(feeding, now);
    if RESTART.load(Ordering::Acquire) || !feeding {
        if !STOPPED.swap(true, Ordering::Relaxed) {
            // The owner's beat age, gap and supervision; zeros with no owner.
            let (owner_age, owner_gap, supervised) = inputs.owner.map_or((0, 0, false), |b| {
                (now.wrapping_sub(b.at_ms), b.gap_ms, b.supervised)
            });
            let mut report = startup::Report::new();
            if let Ok(line) = report.record(format_args!(
                "PLSUP1 stop startup={} trial={} confirmed={} alive={} outputs={} restart={} \
                 owner={} owner_age={} owner_gap={} supervised={} fed_ms={} ms={}\r\n",
                STARTUP.load(Ordering::Relaxed),
                bit(inputs.trial),
                bit(inputs.confirmed),
                bit(alive),
                bit(inputs.outputs_ok),
                bit(RESTART.load(Ordering::Relaxed)),
                bit(inputs.owner.is_some()),
                owner_age,
                owner_gap,
                bit(supervised),
                FED_MS.load(Ordering::Relaxed),
                now
            )) {
                sdk::log(line);
            }
        }
        return FEED_MS;
    }
    {
        if let Some(mut watchdog) = WATCHDOG.enter() {
            let progress = PROGRESS.fetch_add(1, Ordering::Relaxed) + 1;
            let gap = feed_gap();
            if watchdog.checkpoint(u64::from(progress)) {
                FED_MS.store(now, Ordering::Relaxed);
            } else if !STOPPED.swap(true, Ordering::Relaxed) {
                supervision::stopping();
                // A refused checkpoint latches (a gap past the progress
                // limit, an altered register, a clock fault): the hardware
                // is never refreshed again and resets the platform within
                // its expiry. Say so once, with the gap that led to it.
                let mut report = startup::Report::new();
                if let Ok(line) = report.record(format_args!(
                    "PLSUP1 stop watchdog_fault={} gap_ms={} fed_ms={} ms={}\r\n",
                    watchdog.fault.code(),
                    gap,
                    FED_MS.load(Ordering::Relaxed),
                    now
                )) {
                    sdk::log(line);
                }
            }
        }
    }
    FEED_MS
}

/// The feeder task's last checkpoint attempt and the longest gap between
/// two, on the system timer: the FreeRTOS tick stops during every flash
/// lease, so uptime would hide the time the watchdog's progress limit
/// counts.
static FEED_AT_TICKS: AtomicU32 = AtomicU32::new(0);
static FEED_GAP_MAX_MS: AtomicU32 = AtomicU32::new(0);

/// The gap since the previous checkpoint attempt, in milliseconds (zero
/// for the first), and the longest one so far.
fn feed_gap() -> u32 {
    let ticks = sdk::systimer_ticks();
    let last = FEED_AT_TICKS.swap(ticks, Ordering::Relaxed);
    if last == 0 {
        return 0;
    }
    let gap = ticks_ms(ticks.wrapping_sub(last));
    FEED_GAP_MAX_MS.fetch_max(gap, Ordering::Relaxed);
    gap
}

fn ticks_ms(ticks: u32) -> u32 {
    let ms = u64::from(ticks) * 1000 / u64::from(config::SYSTIMER_HZ);
    u32::try_from(ms).unwrap_or(u32::MAX)
}

/// The service task's confirmation stage; see `confirm_step`.
static CONFIRM: Slot<Confirm> = Slot::new();
/// The service task's update stage, which owns the update store: the
/// confirmation raises its floor through it, so the store has one writer.
static UPDATE: Slot<Update<StoreSlots<SdkHooks>>> = Slot::new();

fn new_update() -> Update<StoreSlots<SdkHooks>> {
    let bank = startup::snapshot().bank().unwrap_or(1);
    Update::new(
        bank,
        trial::identity(),
        update::public_key(),
        StoreSlots::new(),
    )
}

/// Raise the update store's floor for this build, as the confirmation
/// writes its record.
fn raise_floor() -> Floor {
    UPDATE
        .enter_or_init(new_update)
        .map_or(Floor::Failed, |mut stage| stage.raise_floor())
}

struct ConfirmLine(confirm::Record);

impl core::fmt::Display for ConfirmLine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        confirm::describe(self.0, f)
    }
}

/// Confirm this build once it has stayed healthy, after the startup
/// probe has released the KM0 handshake.
fn confirm_step() -> u32 {
    let now = sdk::uptime_ms();
    HEARTBEAT.store(now, Ordering::Release);
    let Some(mut confirm) = CONFIRM.enter_or_init(Confirm::default) else {
        return 0;
    };
    let provider = PROVIDER.get();
    // A live link (associated, leased, its gateway answering) stands in for the
    // broker session the normal role no longer opens. Nothing here needs the
    // internet: a feeder on a LAN without a route out confirms.
    let healthy = net_status::read().live
        && PROBE_DONE.load(Ordering::Acquire)
        && stopped::observe().verified();

    let mut storage = HookedStorage::<SdkHooks>::new();
    // SAFETY: the provider is a static, never borrowed (see `PROVIDER`).
    let step = unsafe {
        confirm.step::<Hardware, SdkHooks>(provider, &mut storage, raise_floor, healthy, now)
    };

    if let Some(record) = step.record {
        let mut report = startup::Report::new();
        if let Ok(line) = report.record(format_args!("PLCNF1 {}\r\n", ConfirmLine(record))) {
            sdk::log(line);
        }
    }

    HEARTBEAT.store(sdk::uptime_ms(), Ordering::Release);
    if step.delay_ms == 0 {
        CONFIRM_DONE.store(true, Ordering::Release);
        if matches!(step.record, Some(confirm::Record::Confirmed { .. })) {
            CONFIRMED.store(true, Ordering::Release);
        }
        if matches!(
            step.record,
            Some(confirm::Record::Already | confirm::Record::Confirmed { .. })
        ) {
            start_settings();
        }
    }
    step.delay_ms
}

struct UpdateLine(update::Record);

impl core::fmt::Display for UpdateLine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.0.tag())?;
        f.write_str(" ")?;
        update::describe(self.0, f)
    }
}

/// Take the device API's update requests: check offers, give way on bank
/// 1, install into bank 1 on bank 0.
fn update_step() -> u32 {
    let now = sdk::uptime_ms();
    let Some(mut stage) = UPDATE.enter_or_init(new_update) else {
        return 0;
    };
    let mut flash = SdkFlash::<SdkHooks>::new();
    let step = stage.step(&mut flash, now);

    if let Some(record) = step.record {
        let mut report = startup::Report::new();
        if let Ok(line) = report.record(format_args!("{}\r\n", UpdateLine(record))) {
            sdk::log(line);
        }
        if matches!(record, update::Record::Progress { .. }) {
            log_leases();
        }
    }
    if step.restart {
        restart();
        sdk::log(c"PLOTA1 restart watchdog=1\r\n");
        return 0;
    }
    step.delay_ms
}

/// The install's heartbeat, beside each progress record: the flash leases
/// so far and their slowest exchanges (32 768 Hz ticks), the longest any
/// held IRQs masked and how many held them past 100 ms, and the feeder
/// task's longest gap between checkpoints. A reset that cuts an install
/// short leaves the last of these as the state just before it.
fn log_leases() {
    // Copied with IRQs masked, through the raw pointer (the provider is
    // never borrowed; see `PROVIDER`): a lease updates these counters in
    // its release, which runs with IRQs masked, so with them masked here
    // no update is half done and the u64s cannot tear on this core.
    let provider = PROVIDER.get();
    let (leases, park_max, release_max, masked_max, masked_long) = {
        let _guard = sdk::IrqGuard::lock();
        // SAFETY: a static's fields, read in place with no reference taken.
        unsafe {
            (
                ptr::read_volatile(&raw const (*provider).leases),
                ptr::read_volatile(&raw const (*provider).park_max),
                ptr::read_volatile(&raw const (*provider).release_max),
                ptr::read_volatile(&raw const (*provider).masked_max),
                ptr::read_volatile(&raw const (*provider).masked_long),
            )
        }
    };

    let mut report = startup::Report::new();
    if let Ok(line) = report.record(format_args!(
        "PLOTA1 leases count={} park_max={} release_max={} masked_max_ms={} masked_long={} \
         feed_gap_max_ms={}\r\n",
        leases,
        park_max,
        release_max,
        ticks_ms(masked_max),
        masked_long,
        FEED_GAP_MAX_MS.load(Ordering::Relaxed),
    )) {
        sdk::log(line);
    }
}

/// The runtime's record storage: the SDK wrappers, refused unless the
/// hooks are armed (reads are mapped and need no lease). The settings
/// writer's, and the feeding owner's journal (`JournalSlots`).
pub struct SdkStorage;

/// The feed journal's two sectors, through the armed hooks.
pub type JournalSlots = crate::storage::Slots<SdkStorage, { crate::storage::JOURNAL_BASE }>;

/// Whether the flash hooks are armed, so that record writes can succeed.
/// The settings writer arms them once the build is confirmed.
pub fn storage_armed() -> bool {
    SdkHooks::armed()
}

/// The lease authority the armed flash hooks use, for a feeding owner
/// (`OwnerConfig::flash`): each lease blocks the owner's outputs while it
/// runs. Unbound, so it blocks every output, until the hooks are armed;
/// arming binds it once and nothing rebinds it in the normal role.
pub fn flash_authority() -> *mut FlashCoord {
    // SAFETY: only the address is taken; nothing is borrowed (see `PROVIDER`).
    unsafe { &raw mut (*PROVIDER.get()).coord }
}

impl crate::storage::FlashOps for SdkStorage {
    fn install(provider: *mut Provider) -> bool {
        SdkHooks::install(provider)
    }
    fn read_id() -> [u8; 3] {
        [0; 3] // Unused by the settings writer.
    }
    fn armed() -> bool {
        SdkHooks::armed()
    }
    fn erase(sector: u32) -> bool {
        // SAFETY: the hooks are armed; the settings writer owns the sectors
        // it is given.
        SdkHooks::armed() && unsafe { sdk::storage_erase(sector) }
    }
    fn program(sector: u32, offset: u32, data: &[u8]) -> bool {
        // SAFETY: as in `erase`.
        SdkHooks::armed() && unsafe { sdk::storage_program(sector, offset, data) }
    }
    fn read(sector: u32, offset: u32, bytes: &mut [u8]) -> bool {
        sdk::storage_read(sector, offset, bytes)
    }
}

static SETTINGS_RUNNING: AtomicBool = AtomicBool::new(false);

/// Load the device settings, arm the hooks, then save signed changes: one
/// step of the store writer the firmware links (`wellfed_store_step`).
fn settings_step() -> u32 {
    let provider = PROVIDER.get();
    // SAFETY: the provider is a static, never borrowed (see `PROVIDER`),
    // and only this task steps the writer.
    let delay_ms = unsafe { wellfed_store_step(provider) };
    if delay_ms == 0 {
        SETTINGS_RUNNING.store(false, Ordering::Release);
    }
    delay_ms
}

/// Starts the settings stage in the service task, which steps it from its
/// next pass. `created` keeps its meaning for the bench: the stage runs.
fn start_settings() {
    SETTINGS_RUNNING.store(true, Ordering::Release);
    sdk::log(c"PLCFG1 start created=1\r\n");
}

/// The normal role's confirmation stage. The feeder, started with the
/// application, already supervises it.
fn start_confirm(service: bool) {
    HEARTBEAT.store(sdk::uptime_ms(), Ordering::Release);
    CONFIRM_RUNNING.store(service, Ordering::Release);
    let opened = WATCHDOG_OPEN.load(Ordering::Acquire);
    sdk::log(match (service, opened) {
        (true, true) => c"PLCNF1 start created=1 watchdog=1\r\n",
        (true, false) => c"PLCNF1 start created=1 watchdog=0\r\n",
        _ => c"PLCNF1 start created=0 watchdog=0\r\n",
    });
}

static POWER_RUNNING: AtomicBool = AtomicBool::new(false);
static CONFIRM_RUNNING: AtomicBool = AtomicBool::new(false);
/// The service task's last pass, while any of its stages runs.
static SERVICE_BEAT: AtomicU32 = AtomicU32::new(0);
static SERVICE_RUNNING: AtomicBool = AtomicBool::new(false);

/// When each stage next wants a step; `None` means now.
#[derive(Default)]
struct Due {
    power: Option<u32>,
    confirm: Option<u32>,
    settings: Option<u32>,
    update: Option<u32>,
}

/// The service task's own.
static DUE: Slot<Due> = Slot::new();

/// One stage of the service task: stepped when due, and stopped once its
/// step returns zero. Generic, so each step is a direct call the stack audit
/// follows.
fn stage(
    running: &AtomicBool,
    due: &mut Option<u32>,
    now: u32,
    step: impl FnOnce() -> u32,
) -> Option<u32> {
    if !running.load(Ordering::Acquire) {
        return None;
    }
    if due.is_none_or(|at| deadline_passed(now, at)) {
        let delay = step();
        SERVICE_BEAT.store(sdk::uptime_ms(), Ordering::Release);
        if delay == 0 {
            running.store(false, Ordering::Release);
            return None;
        }
        *due = Some(sdk::uptime_ms().wrapping_add(delay));
    }
    // A stage that fell due while earlier ones ran waits 0, not a wrapped
    // difference: that read as weeks and stalled the task until the watchdog.
    due.map(|at| until(sdk::uptime_ms(), at))
}

/// The runtime's work in one cooperative task: the power monitor, the
/// chain that confirms the build (bank 1) and opens the settings writer
/// (which arms the hooks if the confirmation did not), and the update
/// stage. One task instead of four leaves the firmware its share of the
/// eight task slots, and runs every storage and bank-1 write one after
/// another: no two of them arm or write at once. The longest step is an
/// offer's signature checks or one sector's erase and program, well inside
/// the feeder's liveness window.
unsafe extern "C" fn service_task(_: *mut c_void) -> u32 {
    let Some(mut due) = DUE.enter_or_init(Due::default) else {
        SERVICE_RUNNING.store(false, Ordering::Release);
        return 0;
    };

    let now = sdk::uptime_ms();
    SERVICE_BEAT.store(now, Ordering::Release);
    let waits = [
        stage(&POWER_RUNNING, &mut due.power, now, power_step),
        stage(&CONFIRM_RUNNING, &mut due.confirm, now, confirm_step),
        stage(&SETTINGS_RUNNING, &mut due.settings, now, settings_step),
        stage(&UPDATE_RUNNING, &mut due.update, now, update_step),
    ];

    match waits.into_iter().flatten().min() {
        Some(wait) => wait.max(1),
        None => {
            SERVICE_RUNNING.store(false, Ordering::Release);
            0
        }
    }
}

/// 16 KiB: an offer's Ed25519 checks are the deepest path, which the stack
/// audit bounds; 16 KiB keeps the audit's 25% margin. The task's stack comes
/// out of the heap.
const SERVICE_STACK_WORDS: u16 = 4096;

/// The service task, with the power monitor, the update stage and, on bank
/// 1, the confirmation as its first stages. Bank 0 never confirms: it opens
/// the settings writer at once.
fn start_service(bank: Option<u8>) {
    SERVICE_BEAT.store(sdk::uptime_ms(), Ordering::Release);
    SERVICE_RUNNING.store(true, Ordering::Release);
    POWER_RUNNING.store(true, Ordering::Release);
    UPDATE_RUNNING.store(true, Ordering::Release);
    // SAFETY: the step takes no context and keeps its state in statics.
    let created = unsafe {
        sdk::task_create(
            c"wellfed-svc",
            SERVICE_STACK_WORDS,
            2,
            service_task,
            ptr::null_mut(),
        )
    };
    if !created {
        SERVICE_RUNNING.store(false, Ordering::Release);
        POWER_RUNNING.store(false, Ordering::Release);
        UPDATE_RUNNING.store(false, Ordering::Release);
    }

    sdk::log(if created {
        c"PLBAT1 start created=1\r\n"
    } else {
        c"PLBAT1 start created=0\r\n"
    });
    if bank == Some(0) {
        CONFIRM_DONE.store(true, Ordering::Release);
        if created {
            start_settings();
        }
    } else {
        start_confirm(created);
    }
}

fn bit(value: bool) -> u8 {
    u8::from(value)
}

fn kind(kind: u32) -> &'static str {
    match kind {
        sdk::WifiEvent::ON => "on",
        sdk::WifiEvent::SCAN => "scan",
        sdk::WifiEvent::CONNECT => "connect",
        sdk::WifiEvent::DHCP => "dhcp",
        sdk::WifiEvent::LOST => "lost",
        sdk::WifiEvent::BACKOFF => "backoff",
        sdk::WifiEvent::GATEWAY => "gateway",
        sdk::WifiEvent::LEAVE => "leave",
        _ => "unknown",
    }
}

fn log_probe(record: Record) {
    let mut report = startup::Report::new();
    let line = match record {
        Record::Start { created } => {
            report.record(format_args!("PLWIF3 start created={}\r\n", bit(created)))
        }
        Record::Event { event: e, stopped } => report.record(format_args!(
            "PLWIF3 ev seq={} kind={} attempt={} result={} value={:08x} rssi={} security={:08x} ms={} stopped={}\r\n",
            e.seq,
            kind(e.kind),
            e.attempt,
            e.result,
            e.value,
            e.rssi,
            e.security,
            e.ms,
            bit(stopped)
        )),
        Record::Overrun { seq } => {
            report.record(format_args!("PLWIF3 error overrun seq={seq}\r\n"))
        }
        Record::Hold => report.record(format_args!(
            "PLWIF3 hold until=startup max_ms={WIFI_HOLD_MAX_MS}\r\n"
        )),
    };

    if let Ok(line) = line {
        sdk::log(line);
    }
}

/// The longest the radio waits for startup work: a firmware that never
/// reports `startup_done` still joins.
const WIFI_HOLD_MAX_MS: u32 = 180_000;

fn probe_step() -> u32 {
    let Some(mut probe) = PROBE.enter_or_init(Probe::default) else {
        return 0;
    };
    // The startup probe's windows run first, on either bank.
    let ready = PROBE_DONE.load(Ordering::Acquire) || sdk::uptime_ms() >= WIFI_HOLD_MAX_MS;
    let step = probe.step_when(ready);
    if let Some(record) = step.record {
        log_probe(record);
    }
    step.delay_ms
}

/// The boot's generator start. `first` (raw samples, before conditioning)
/// and `check` (a hash of one discarded output block) let the bench compare
/// boots without revealing a secret.
fn log_rng(started: rng::Started) {
    let first = wellfed_session::hex::Lower(&started.first);
    let check = rng::bytes().map(|block| {
        let digest = wellfed_session::mac::hmac_sha256(&[0; 32], &[b"wellfed-rng-check", &block]);
        u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]])
    });

    let mut report = startup::Report::new();
    let line = match (started.result, check) {
        (Ok(()), Some(check)) => report.record(format_args!(
            "PLRNG1 boot result=ready samples={} first={first} check={check:08x}\r\n",
            started.samples
        )),
        (Ok(()), None) => report.record(format_args!("PLRNG1 boot result=unavailable\r\n")),
        (Err(failure), _) => report.record(format_args!(
            "PLRNG1 boot result={failure} samples={} first={first}\r\n",
            started.samples
        )),
    };
    if let Ok(line) = line {
        sdk::log(line);
    }
}

fn log_gpio_routing(stage: &str) {
    let value = sdk::gpio_routing();
    let mut report = startup::Report::new();
    if let Ok(line) = report.record(format_args!(
        "PLIO1 stage={} groups={:08x} aon={:08x} control_a={:08x} control_b={:08x} share_pad={:08x} ext_a={:08x}\r\n",
        stage, value.groups, value.aon, value.control_a, value.control_b, value.share_pad, value.ext_a,
    )) {
        sdk::log(line);
    }
}

fn log_led_readback(led: leds::Led, high: bool, stage: &str, value: leds::Readback) {
    let mut report = startup::Report::new();
    if let Ok(line) = report.record(format_args!(
        "PLDBG1 pin={} stage={} want={} latch={} input={} highs={}/{} directions={} stopped={} pad={:08x}\r\n",
        led.name(),
        stage,
        u8::from(high),
        u8::from(value.latch_high),
        u8::from(value.input_high),
        value.input_highs,
        leds::INPUT_SAMPLES,
        u8::from(value.directions_ok),
        u8::from(value.stopped_ok),
        value.pad,
    )) {
        sdk::log(line);
    }
}

unsafe extern "C" fn diagnostic_task(_: *mut c_void) -> u32 {
    STARTUP_BEAT.store(sdk::uptime_ms(), Ordering::Release);
    if PROBING.load(Ordering::Relaxed) {
        return probe_step();
    }

    sdk::log(c"\r\nWellfed local firmware: Rust bringup\r\n");
    let mut report = startup::Report::new();
    if let Ok(line) = report.start(startup::snapshot()) {
        sdk::log(line);
    }

    // Test variant: a determined startup failure, which a trial image
    // must turn into a counted reset. Opaque to the optimizer, or LTO drops
    // every path after a passing diagnostic and the variant no longer links
    // the image the link audit pins.
    let forced_fail = cfg!(feature = "trial-fail") && core::hint::black_box(true);
    let passed = wellfed_core::feed::diagnostic::run() && !forced_fail;
    sdk::log(if passed {
        c"Synthetic core diagnostic: PASS\r\n"
    } else {
        c"Synthetic core diagnostic: FAIL\r\n"
    });
    sdk::log(c"Synthetic diagnostic only; feeder backend stays unopened.\r\n");
    sdk::log(if passed {
        c"PLFW1 result diagnostic=pass backend=unopened\r\n"
    } else {
        c"PLFW1 result diagnostic=fail backend=unopened\r\n"
    });

    if !passed || !leds::initialize() {
        for led in leds::Led::ALL {
            log_led_readback(led, false, "initialize", leds::observe(led));
        }
        sdk::log(c"PLLED1 error initialize=unverified\r\n");
        return startup_failed();
    }

    // The indicators stay LOW (policy low-only-v1): the pulse probe
    // proved them and cost every boot about 15 s; a later card gives them meaning.
    if !(leds::all_low() && stopped::observe().verified()) {
        sdk::log(c"PLLED1 error finish=unverified\r\n");
        return startup_failed();
    }
    sdk::log(c"PLLED1 begin low_only\r\n");
    sdk::log(c"PLLED1 done low=all\r\n");

    PROBING.store(true, Ordering::Relaxed);
    // Before any other task can use the ADC or want a secret.
    log_rng(rng::start());
    // No MQTT session on either bank; requests, health and updates go over
    // the firmware's device API.
    // SAFETY: every firmware defines the hook with `firmware!`; it runs
    // once, here, in this task.
    unsafe { wellfed_firmware_start() };
    start_service(startup::snapshot().bank());
    STARTUP.store(Startup::Started.code(), Ordering::Release);
    1
}

/// Called once by the SDK's `example_entry`, before the scheduler starts.
#[unsafe(no_mangle)]
pub extern "C" fn wellfed_app_start() {
    let snapshot = stopped::initialize();
    // Before anything arms or writes the marker register's attempt bits.
    if let Some(fault) = LeaseFault::take() {
        let mut report = startup::Report::new();
        if let Ok(line) = report.record(format_args!(
            "PLSUP1 lease_fault stage={} phase={} fault={} reply={} elapsed_ms={}\r\n",
            fault.stage.code(),
            fault.phase.code(),
            fault.fault.code(),
            fault.reply.code(),
            fault.elapsed_ms
        )) {
            sdk::log(line);
        }
    }
    // Before the scheduler starts the tick hook that overwrites it. Only a
    // watchdog reset is a hang worth naming; any other reset just clears it.
    if let Some(crumb) = crumb::take()
        && startup::snapshot().reset & sdk::BOOT_KM4_WATCHDOG != 0
    {
        let mut report = startup::Report::new();
        if let Ok(line) = report.record(format_args!("PLSUP1 hang {crumb}\r\n")) {
            sdk::log(line);
        }
    }
    // Supervise from the first scheduled step, so a startup fault or
    // hang is a reset KM0 can count.
    // SAFETY: the step takes no context and keeps its state in statics.
    if !unsafe { sdk::task_create(c"wellfed-wdg", 512, 3, feeder_task, ptr::null_mut()) } {
        sdk::log(c"PLSUP1 error task=create\r\n");
    }
    let mut report = startup::Report::new();
    if let Ok(line) = report.record(format_args!(
        "PLCLK1 function_before={:08x} clock_before={:08x} function_after={:08x} clock_after={:08x} status={}\r\n",
        snapshot.clock.function_before, snapshot.clock.clock_before,
        snapshot.clock.function_after, snapshot.clock.clock_after,
        if snapshot.clock.verified() { "pass" } else { "fail" },
    )) {
        sdk::log(line);
    }

    log_gpio_routing("startup");
    if let Ok(line) = report.record(format_args!(
        "PLFW1 outputs pa={:08x} pb={:08x} dira={:08x} dirb={:08x} status={}\r\n",
        snapshot.pa,
        snapshot.pb,
        snapshot.dira,
        snapshot.dirb,
        if snapshot.verified() { "pass" } else { "fail" },
    )) {
        sdk::log(line);
    }

    if !snapshot.verified() {
        sdk::log(c"PLFW1 error outputs=unverified\r\n");
        startup_failed();
        return;
    }

    // SAFETY: the step takes no context and keeps its state in statics.
    if !unsafe { sdk::task_create(c"wellfed", 1024, 1, diagnostic_task, ptr::null_mut()) } {
        sdk::log(c"Wellfed: diagnostic task creation failed\r\n");
        sdk::log(c"PLFW1 error task=create\r\n");
        startup_failed();
    }
}
