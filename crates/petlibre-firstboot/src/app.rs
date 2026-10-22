//! KM4 application entry: logs and runs the synthetic diagnostic once.
//! Stopped-output startup and a bounded indicator probe, then the FW07 stage
//! D probe: join the configured network, check DNS and keep both, logging the
//! shim's events. Beside it, the FW08 transport task reports health to the
//! local server's broker, and the FW13 optics task exercises the emitters and
//! reads the food, outlet and feedback inputs, motor STOP throughout.
//!
//! Booted from bank 0 (the recovery role), the FW10 updater runs in place of
//! the transport and optics tasks, with a watchdog feeder beside it. In the
//! normal role, the FW11 confirmation task and the feeder confirm a healthy
//! build, and only then does the FW10 offer check start. A hung task stops
//! the feeding, so a hang becomes a watchdog reset that KM0 counts.

use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use petlibre_amebad::audio;
use petlibre_amebad::confirm::{self, Confirm, HookedStorage};
use petlibre_amebad::flash::hooks::{self, HookProvider, HookTicket};
use petlibre_amebad::flash::park::Hardware;
use petlibre_amebad::health::Boot;
use petlibre_amebad::provider::Provider;
use petlibre_amebad::supervise::{self, LIVENESS_MS, Startup};
use petlibre_amebad::transport::{self, Link};
use petlibre_amebad::updater::{self, Hooked, Hooks, Updater};
use petlibre_amebad::watch::{self, SdkGiveWay, Watch};
use petlibre_amebad::watchdog::Watchdog;
use petlibre_amebad::{config, leds, sdk, startup, stopped, trial};

use crate::probes::optics;
use crate::probes::wifi::{Probe, Record};

static PROBING: AtomicBool = AtomicBool::new(false);

/// Fixed-address probe storage touched only by the diagnostic task.
struct Slot<T>(UnsafeCell<Option<T>>);
// SAFETY: the diagnostic task is the only user; see `probe_step`.
unsafe impl<T> Sync for Slot<T> {}

impl<T> Slot<T> {
    /// The value, built on first use by `make` in a frame of its own. Built
    /// inline, a large value's temporary stays reserved in the task's frame
    /// beneath every later step: after the crate split, LTO inlined the
    /// optics probe's 3 KiB default and the task overflowed its 4 KiB stack.
    ///
    /// # Safety
    /// Only the slot's owning task calls this, with no other reference into
    /// the slot live.
    unsafe fn get_or_try_init(&self, make: impl FnOnce() -> Option<T>) -> Option<&mut T> {
        // SAFETY: the caller is the slot's only user.
        let slot = unsafe { &mut *self.0.get() };
        if slot.is_none() {
            fill(slot, make);
        }
        slot.as_mut()
    }

    /// As `get_or_try_init`, for a constructor that cannot fail.
    ///
    /// # Safety
    /// As for `get_or_try_init`.
    unsafe fn get_or_init(&self, make: impl FnOnce() -> T) -> &mut T {
        // SAFETY: the caller's contract.
        match unsafe { self.get_or_try_init(|| Some(make())) } {
            Some(value) => value,
            None => unreachable!(),
        }
    }
}

#[inline(never)]
#[cold]
fn fill<T>(slot: &mut Option<T>, make: impl FnOnce() -> Option<T>) {
    *slot = make();
}
static PROBE: Slot<Probe> = Slot(UnsafeCell::new(None));
/// Touched only by the transport task; see `net_task`.
static NET: Slot<Link> = Slot(UnsafeCell::new(None));
/// Touched only by the optics task; see `optics_task`.
static OPTICS: Slot<optics::Probe> = Slot(UnsafeCell::new(None));

struct OpticsLine(optics::Record);

impl core::fmt::Display for OpticsLine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        optics::describe(self.0, f)
    }
}

unsafe extern "C" fn optics_task(_: *mut c_void) -> u32 {
    // SAFETY: only this task touches the slot; the probe stays in place.
    let probe = unsafe { OPTICS.get_or_init(optics::Probe::default) };
    let step = probe.step::<Hardware>();
    if let Some(record) = step.record {
        let mut report = startup::Report::new();
        if let Ok(line) = report.record(format_args!("PLOPT1 {}\r\n", OpticsLine(record))) {
            sdk::log(line);
        }
    }
    PROBE_DONE.store(step.delay_ms == 0, Ordering::Release);
    step.delay_ms
}

/// The optics and pulse probes' stacks. Building a probe in place copies it
/// through two constructor frames (about 3.6 KiB), so 4 KiB overflowed once
/// LTO inlined differently after the crate split.
const PROBE_STACK_WORDS: u16 = 2048;

/// Touched only by the pulse task; see `pulse_task`.
#[cfg(feature = "fw16-pulse")]
static PULSE: Slot<crate::probes::pulse::Probe> = Slot(UnsafeCell::new(None));

#[cfg(feature = "fw16-pulse")]
struct PulseLine(crate::probes::pulse::Record);

#[cfg(feature = "fw16-pulse")]
impl core::fmt::Display for PulseLine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        crate::probes::pulse::describe(self.0, f)
    }
}

#[cfg(feature = "fw16-pulse")]
unsafe extern "C" fn pulse_task(_: *mut c_void) -> u32 {
    // SAFETY: only this task touches the slot; the probe stays in place.
    let probe = unsafe { PULSE.get_or_init(Default::default) };
    let step = probe.step::<Hardware>();
    if let Some(record) = step.record {
        let mut report = startup::Report::new();
        if let Ok(line) = report.record(format_args!("PLMOT1 {}\r\n", PulseLine(record))) {
            sdk::log(line);
        }
    }
    PROBE_DONE.store(step.delay_ms == 0, Ordering::Release);
    step.delay_ms
}

/// FW16 builds run the motor pulse in place of the optics probe.
#[cfg(feature = "fw16-pulse")]
fn start_optics() {
    let created = sdk::task_create(
        c"petlibre-pulse",
        PROBE_STACK_WORDS,
        3,
        pulse_task,
        ptr::null_mut(),
    );
    sdk::log(if created {
        c"PLMOT1 start created=1\r\n"
    } else {
        c"PLMOT1 start created=0\r\n"
    });
}

/// Above the diagnostic and transport tasks, so their logging cannot hold
/// off the owner lease the supervisor enforces.
#[cfg(not(any(feature = "fw16-pulse", feature = "fw11-reset")))]
fn start_optics() {
    let created = sdk::task_create(
        c"petlibre-optics",
        PROBE_STACK_WORDS,
        3,
        optics_task,
        ptr::null_mut(),
    );
    sdk::log(if created {
        c"PLOPT1 start created=1 plan=2\r\n"
    } else {
        c"PLOPT1 start created=0 plan=2\r\n"
    });
    const _: () = assert!(optics::PLAN == 2);
}

/// A transport record as the body of a `PLMQT1` line.
struct Line(transport::Record);

impl core::fmt::Display for Line {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        transport::describe(self.0, f)
    }
}

fn log_transport(record: transport::Record) {
    let mut report = startup::Report::new();
    if let Ok(line) = report.record(format_args!("PLMQT1 {}\r\n", Line(record))) {
        sdk::log(line);
    }
}

unsafe extern "C" fn net_task(_: *mut c_void) -> u32 {
    // SAFETY: only this task touches the slot.
    let link = unsafe {
        NET.get_or_try_init(|| {
            let link = sdk::transport_config()
                .map(|config| Link::new(config, Boot::read(startup::snapshot())));
            if link.is_none() {
                sdk::log(c"PLMQT1 error config\r\n");
            }
            link
        })
    };
    let Some(link) = link else {
        return 0;
    };

    NET_RUNNING.store(true, Ordering::Release);
    let delay = link.step(&mut log_transport);
    NET_READY.store(link.ready(), Ordering::Release);
    NET_BEAT.store(sdk::uptime_ms(), Ordering::Release);

    if delay == 0 {
        NET_RUNNING.store(false, Ordering::Release);
    }
    delay
}

fn start_transport() {
    let created = sdk::task_create(c"petlibre-net", 1536, 2, net_task, ptr::null_mut());
    sdk::log(if created {
        c"PLMQT1 start created=1\r\n"
    } else {
        c"PLMQT1 start created=0\r\n"
    });
}

/// Touched only by the audio task; see `audio_task`.
static SPEAKER: Slot<audio::Speaker> = Slot(UnsafeCell::new(None));

struct AudioLine(audio::Record);

impl core::fmt::Display for AudioLine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        audio::describe(self.0, f)
    }
}

/// FW24: the startup chime, then tunes the transport queues.
unsafe extern "C" fn audio_task(_: *mut c_void) -> u32 {
    // SAFETY: only this task touches the slot.
    let speaker = unsafe { SPEAKER.get_or_init(Default::default) };
    let step = speaker.step();
    if let Some(record) = step.record {
        let mut report = startup::Report::new();
        if let Ok(line) = report.record(format_args!("PLAUD1 {}\r\n", AudioLine(record))) {
            sdk::log(line);
        }
    }
    step.delay_ms
}

/// Speaker variant: touched only by the stream task; see `stream_task`.
#[cfg(feature = "speaker")]
static STREAMER: Slot<petlibre_amebad::stream::Streamer> = Slot(UnsafeCell::new(None));

#[cfg(feature = "speaker")]
struct StreamLine(petlibre_amebad::stream::Record);

#[cfg(feature = "speaker")]
impl core::fmt::Display for StreamLine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        petlibre_amebad::stream::describe(self.0, f)
    }
}

/// Speaker variant: plays signed network audio streams.
#[cfg(feature = "speaker")]
unsafe extern "C" fn stream_task(_: *mut c_void) -> u32 {
    // SAFETY: only this task touches the slot.
    let streamer = unsafe { STREAMER.get_or_init(Default::default) };
    let step = streamer.step();
    if let Some(record) = step.record {
        let mut report = startup::Report::new();
        if let Ok(line) = report.record(format_args!("PLSPK1 {}\r\n", StreamLine(record))) {
            sdk::log(line);
        }
    }
    step.delay_ms
}

#[cfg(feature = "speaker")]
fn start_stream() {
    let created = sdk::task_create(c"petlibre-spk", 1024, 2, stream_task, ptr::null_mut());
    sdk::log(if created {
        c"PLSPK1 start created=1\r\n"
    } else {
        c"PLSPK1 start created=0\r\n"
    });
}

#[cfg(not(feature = "speaker"))]
fn start_stream() {}

/// Below the feeder and the optics probe: a late page is an audible gap,
/// never a missed lease.
fn start_audio() {
    let created = sdk::task_create(c"petlibre-aud", 1024, 2, audio_task, ptr::null_mut());
    sdk::log(if created {
        c"PLAUD1 start created=1\r\n"
    } else {
        c"PLAUD1 start created=0\r\n"
    });
}

/// Touched only by the updater task, except that the armed hooks reach the
/// provider through its context pointer.
static UPDATER: Slot<Updater> = Slot(UnsafeCell::new(None));
static PROVIDER: Slot<Provider> = Slot(UnsafeCell::new(None));
/// Opened by the updater task, then touched only by the feeder task.
static WATCHDOG: Slot<Watchdog> = Slot(UnsafeCell::new(None));
static WATCHDOG_OPEN: AtomicBool = AtomicBool::new(false);

/// The flash writer's (updater or FW11 confirmation) last step, in uptime
/// milliseconds.
static HEARTBEAT: AtomicU32 = AtomicU32::new(0);
/// The writer ended without activating: feed while outputs stay stopped.
static UPDATER_DONE: AtomicBool = AtomicBool::new(false);
/// Bank 1 is activated: stop feeding so the watchdog resets the platform.
static RESTART: AtomicBool = AtomicBool::new(false);

/// The transport task's last step and state, for the feeder and FW11.
static NET_BEAT: AtomicU32 = AtomicU32::new(0);
static NET_RUNNING: AtomicBool = AtomicBool::new(false);
static NET_READY: AtomicBool = AtomicBool::new(false);
/// The offer-check task's last step, while it runs.
static WATCH_BEAT: AtomicU32 = AtomicU32::new(0);
static WATCH_RUNNING: AtomicBool = AtomicBool::new(false);
/// The startup optics (or pulse) probe has ended and released the KM0
/// handshake, so another writer may bind it.
static PROBE_DONE: AtomicBool = AtomicBool::new(false);

const FEED_MS: u32 = 250;

/// FW11 supervision state for the feeder (`supervise.rs`).
static STARTUP: AtomicU8 = AtomicU8::new(STARTUP_RUNNING);
const STARTUP_RUNNING: u8 = 0;
const STARTUP_STARTED: u8 = 1;
const STARTUP_FAILED: u8 = 2;
/// The diagnostic task's last step, while startup runs.
static STARTUP_BEAT: AtomicU32 = AtomicU32::new(0);
/// Bank 1 without a confirmation record for this build.
static TRIAL: AtomicBool = AtomicBool::new(false);
/// This boot confirmed the build.
static CONFIRMED: AtomicBool = AtomicBool::new(false);

/// Startup stopped on a determined failure; ends the diagnostic task.
fn startup_failed() -> u32 {
    STARTUP.store(STARTUP_FAILED, Ordering::Release);
    0
}

/// Running bank 1 with no confirmation record naming this build.
fn on_trial() -> bool {
    let mut record = [0u8; petlibre_core::ota::CONFIRM_LEN];
    startup::snapshot().bank() == Some(1)
        && sdk::storage_read(trial::CONFIRM_SECTOR, 0, &mut record)
        && petlibre_core::ota::confirmed(&record, &trial::identity()).is_none()
}
/// 24 KiB: Ed25519 verification under the manifest step needs about 12 KiB,
/// and 10 KiB overflowed on hardware.
const UPDATER_STACK_WORDS: u16 = 6144;

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

struct SdkHooks;

impl Hooks for SdkHooks {
    fn watchdog() -> bool {
        if WATCHDOG_OPEN.load(Ordering::Acquire) {
            return true; // The normal role opened it at startup.
        }

        // SAFETY: the feeder touches the slot only once WATCHDOG_OPEN is set.
        let watchdog = unsafe { WATCHDOG.get_or_init(Watchdog::default) };
        let opened = watchdog.open(&config::WATCHDOG);
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
            fault: Some(hooks::petlibre_amebad_flash_provider_fault),
        };
        // SAFETY: the interposer copies the callbacks; the provider is static.
        unsafe { hooks::petlibre_amebad_flash_hooks_install(&callbacks) }
    }

    fn armed() -> bool {
        hooks::petlibre_amebad_flash_hooks_armed()
    }
}

struct UpdateLine(updater::Record);

impl core::fmt::Display for UpdateLine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        updater::describe(self.0, f)
    }
}

unsafe extern "C" fn updater_task(_: *mut c_void) -> u32 {
    HEARTBEAT.store(sdk::uptime_ms(), Ordering::Release);
    // SAFETY: only this task creates references into the slots; the provider
    // is reached by raw pointer so hook callbacks never alias a live borrow.
    let updater = unsafe {
        UPDATER.get_or_try_init(|| {
            let updater =
                sdk::transport_config().map(|config| Updater::new(config, updater::public_key()));
            if updater.is_none() {
                sdk::log(c"PLOTA1 error stage=config\r\n");
                UPDATER_DONE.store(true, Ordering::Release);
            }
            updater
        })
    };
    let Some(updater) = updater else {
        return 0;
    };

    let provider = unsafe { PROVIDER.get_or_init(Provider::default) } as *mut Provider;
    let mut flash = Hooked::<SdkHooks>::new();
    // SAFETY: the provider slot is fixed for the program's lifetime.
    let step = unsafe { updater.step::<Hardware, SdkHooks>(provider, &mut flash) };

    if let Some(record) = step.record {
        let mut report = startup::Report::new();
        if let Ok(line) = report.record(format_args!("PLOTA1 {}\r\n", UpdateLine(record))) {
            sdk::log(line);
        }
    }

    HEARTBEAT.store(sdk::uptime_ms(), Ordering::Release);
    if step.delay_ms == 0 {
        if step.restart {
            RESTART.store(true, Ordering::Release);
            sdk::log(c"PLOTA1 restart watchdog=1\r\n");
        } else {
            // No update this boot: report health as the normal role would.
            UPDATER_DONE.store(true, Ordering::Release);
            start_transport();
        }
    }
    step.delay_ms
}

/// Opens the watchdog on its first step, before the startup diagnostic and
/// probes run, then feeds it as `supervise::feed` decides: while startup and
/// then the role's tasks keep stepping and outputs stay stopped. A hang, a
/// hook fault (IRQs masked), an activation, or a trial image that fails or
/// never confirms all stop the feeding, and the watchdog resets the
/// platform, which KM0's FW11 trial counts.
unsafe extern "C" fn feeder_task(_: *mut c_void) -> u32 {
    static PROGRESS: AtomicU32 = AtomicU32::new(0);
    static STOPPED: AtomicBool = AtomicBool::new(false);
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
            return 0; // Unsupervised, as before FW11; nothing to feed.
        }
    }

    let now = sdk::uptime_ms();
    let fresh = |beat: &AtomicU32| now.wrapping_sub(beat.load(Ordering::Acquire)) < LIVENESS_MS;
    let alive = (!NET_RUNNING.load(Ordering::Acquire) || fresh(&NET_BEAT))
        && (!WATCH_RUNNING.load(Ordering::Acquire) || fresh(&WATCH_BEAT))
        && (!SETTINGS_RUNNING.load(Ordering::Acquire) || fresh(&SETTINGS_BEAT))
        && (UPDATER_DONE.load(Ordering::Acquire) || fresh(&HEARTBEAT));

    // Stopped, or motor STOP with exactly the supervised emitters lit (the
    // optics probe): the feeder must not trip on a sensing window (observed).
    let outputs = stopped::observe();
    let inputs = supervise::Inputs {
        now_ms: now,
        startup: match STARTUP.load(Ordering::Acquire) {
            STARTUP_RUNNING => Startup::Running,
            STARTUP_STARTED => Startup::Started,
            _ => Startup::Failed,
        },
        startup_beat_ms: STARTUP_BEAT.load(Ordering::Acquire),
        trial: TRIAL.load(Ordering::Acquire),
        confirmed: CONFIRMED.load(Ordering::Acquire),
        tasks_alive: alive,
        outputs_ok: outputs.verified() || outputs.sensing(),
    };
    if RESTART.load(Ordering::Acquire) || !supervise::feed(&inputs) {
        if !STOPPED.swap(true, Ordering::Relaxed) {
            let mut report = startup::Report::new();
            if let Ok(line) = report.record(format_args!(
                "PLSUP1 stop startup={} trial={} confirmed={} alive={} outputs={} restart={} ms={}\r\n",
                STARTUP.load(Ordering::Relaxed),
                bit(inputs.trial),
                bit(inputs.confirmed),
                bit(alive),
                bit(inputs.outputs_ok),
                bit(RESTART.load(Ordering::Relaxed)),
                now
            )) {
                sdk::log(line);
            }
        }
        return FEED_MS;
    }
    {
        // SAFETY: the updater task no longer touches the slot once open.
        if let Some(watchdog) = unsafe { &mut *WATCHDOG.0.get() }.as_mut() {
            let progress = PROGRESS.fetch_add(1, Ordering::Relaxed) + 1;
            let _ = watchdog.checkpoint(u64::from(progress));
        }
    }
    FEED_MS
}

/// Touched only by the confirmation task; see `confirm_task`.
static CONFIRM: Slot<Confirm> = Slot(UnsafeCell::new(None));

struct ConfirmLine(confirm::Record);

impl core::fmt::Display for ConfirmLine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        confirm::describe(self.0, f)
    }
}

/// FW11: confirm this build once it has stayed healthy, after the startup
/// probe has released the KM0 handshake.
unsafe extern "C" fn confirm_task(_: *mut c_void) -> u32 {
    let now = sdk::uptime_ms();
    HEARTBEAT.store(now, Ordering::Release);
    // SAFETY: only this task creates references into the slots; the provider
    // is reached by raw pointer so hook callbacks never alias a live borrow.
    let confirm = unsafe { CONFIRM.get_or_init(Confirm::default) };
    let provider = unsafe { PROVIDER.get_or_init(Provider::default) } as *mut Provider;
    let healthy = NET_READY.load(Ordering::Acquire)
        && PROBE_DONE.load(Ordering::Acquire)
        && stopped::observe().verified();

    let mut storage = HookedStorage::<SdkHooks>::new();
    // SAFETY: the provider slot is fixed for the program's lifetime.
    let step = unsafe { confirm.step::<Hardware, SdkHooks>(provider, &mut storage, healthy, now) };

    if let Some(record) = step.record {
        let mut report = startup::Report::new();
        if let Ok(line) = report.record(format_args!("PLCNF1 {}\r\n", ConfirmLine(record))) {
            sdk::log(line);
        }
    }

    HEARTBEAT.store(sdk::uptime_ms(), Ordering::Release);
    if step.delay_ms == 0 {
        UPDATER_DONE.store(true, Ordering::Release);
        if matches!(step.record, Some(confirm::Record::Confirmed { .. })) {
            CONFIRMED.store(true, Ordering::Release);
        }
        if matches!(
            step.record,
            Some(confirm::Record::Already | confirm::Record::Confirmed { .. })
        ) {
            // The settings writer arms the hooks first; the offer check
            // starts once that settles, so the two never arm at once.
            start_settings();
        }
    }
    step.delay_ms
}

/// Touched only by the offer-check task; see `watch_task`.
static WATCH: Slot<Watch> = Slot(UnsafeCell::new(None));

struct WatchLine(watch::Record);

impl core::fmt::Display for WatchLine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        watch::describe(self.0, f)
    }
}

/// FW10: once confirmed, check for an offer and give way to bank 0's
/// updater when one names another build.
unsafe extern "C" fn watch_task(_: *mut c_void) -> u32 {
    let now = sdk::uptime_ms();
    WATCH_BEAT.store(now, Ordering::Release);
    // SAFETY: only this task creates references into the slot; the provider
    // is reached by raw pointer (the confirmation task has ended).
    let watch = unsafe {
        WATCH.get_or_try_init(|| {
            let watch =
                sdk::transport_config().map(|config| Watch::new(config, updater::public_key()));
            if watch.is_none() {
                WATCH_RUNNING.store(false, Ordering::Release);
            }
            watch
        })
    };
    let Some(watch) = watch else {
        return 0;
    };

    let provider = unsafe { PROVIDER.get_or_init(Provider::default) } as *mut Provider;
    let mut bank = SdkGiveWay::<SdkHooks>::new();
    // SAFETY: the provider slot is fixed for the program's lifetime.
    let step = unsafe { watch.step::<Hardware, SdkHooks>(provider, &mut bank, now) };

    if let Some(record) = step.record {
        let mut report = startup::Report::new();
        if let Ok(line) = report.record(format_args!("PLWCH1 {}\r\n", WatchLine(record))) {
            sdk::log(line);
        }
    }

    WATCH_BEAT.store(sdk::uptime_ms(), Ordering::Release);
    if step.delay_ms == 0 {
        if step.restart {
            RESTART.store(true, Ordering::Release);
        }
        WATCH_RUNNING.store(false, Ordering::Release);
    }
    step.delay_ms
}

/// 24 KiB, as the updater: the manifest check verifies Ed25519.
/// The settings writer's storage: the SDK wrappers, refused unless the
/// hooks are armed (reads are mapped and need no lease).
struct SdkStorage;

impl crate::probes::flash::FlashOps for SdkStorage {
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
        SdkHooks::armed() && sdk::storage_erase(sector)
    }
    fn program(sector: u32, offset: u32, data: &[u8]) -> bool {
        SdkHooks::armed() && sdk::storage_program(sector, offset, data)
    }
    fn read(sector: u32, offset: u32, bytes: &mut [u8]) -> bool {
        sdk::storage_read(sector, offset, bytes)
    }
}

/// Touched only by the settings task; see `settings_task`.
static SETTINGS: Slot<crate::settings::Writer> = Slot(UnsafeCell::new(None));
/// The settings task's last step, while it runs.
static SETTINGS_BEAT: AtomicU32 = AtomicU32::new(0);
static SETTINGS_RUNNING: AtomicBool = AtomicBool::new(false);

struct SettingsLine(crate::settings::Record);

impl core::fmt::Display for SettingsLine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        crate::settings::describe(self.0, f)
    }
}

/// FW25: load the device settings, arm the hooks, then save signed changes.
unsafe extern "C" fn settings_task(_: *mut c_void) -> u32 {
    SETTINGS_BEAT.store(sdk::uptime_ms(), Ordering::Release);
    // SAFETY: only this task creates references into the slot; the provider
    // is reached by raw pointer so hook callbacks never alias a live borrow.
    let writer = unsafe { SETTINGS.get_or_init(Default::default) };
    let provider = unsafe { PROVIDER.get_or_init(Provider::default) } as *mut Provider;
    // SAFETY: the provider slot is fixed for the program's lifetime.
    let step = unsafe { writer.step::<Hardware, SdkHooks, SdkStorage>(provider) };

    if let Some(record) = step.record {
        let mut report = startup::Report::new();
        if let Ok(line) = report.record(format_args!("PLCFG1 {}\r\n", SettingsLine(record))) {
            sdk::log(line);
        }
    }
    if step.settled {
        start_watch();
    }
    SETTINGS_BEAT.store(sdk::uptime_ms(), Ordering::Release);
    if step.delay_ms == 0 {
        SETTINGS_RUNNING.store(false, Ordering::Release);
    }
    step.delay_ms
}

fn start_settings() {
    SETTINGS_BEAT.store(sdk::uptime_ms(), Ordering::Release);
    SETTINGS_RUNNING.store(true, Ordering::Release);
    let created = sdk::task_create(
        c"petlibre-cfg",
        SETTINGS_STACK_WORDS,
        2,
        settings_task,
        ptr::null_mut(),
    );
    if !created {
        SETTINGS_RUNNING.store(false, Ordering::Release);
        start_watch();
    }
    sdk::log(if created {
        c"PLCFG1 start created=1\r\n"
    } else {
        c"PLCFG1 start created=0\r\n"
    });
}

/// The store's 128-byte records and the arming state; 4 KiB like the
/// confirmation task (the build's stack audit checks the bound).
const SETTINGS_STACK_WORDS: u16 = 1024;

fn start_watch() {
    WATCH_BEAT.store(sdk::uptime_ms(), Ordering::Release);
    WATCH_RUNNING.store(true, Ordering::Release);
    let created = sdk::task_create(
        c"petlibre-wch",
        UPDATER_STACK_WORDS,
        2,
        watch_task,
        ptr::null_mut(),
    );
    if !created {
        WATCH_RUNNING.store(false, Ordering::Release);
    }

    sdk::log(if created {
        c"PLWCH1 start created=1\r\n"
    } else {
        c"PLWCH1 start created=0\r\n"
    });
}

/// The normal role's FW11 confirmation task. The feeder, started with the
/// application, already supervises it.
fn start_confirm() {
    HEARTBEAT.store(sdk::uptime_ms(), Ordering::Release);
    let created = sdk::task_create(c"petlibre-cnf", 1024, 2, confirm_task, ptr::null_mut());
    let opened = WATCHDOG_OPEN.load(Ordering::Acquire);
    sdk::log(match (created, opened) {
        (true, true) => c"PLCNF1 start created=1 watchdog=1\r\n",
        (true, false) => c"PLCNF1 start created=1 watchdog=0\r\n",
        _ => c"PLCNF1 start created=0 watchdog=0\r\n",
    });
}

/// The recovery role: the updater, below the feeder so a long network wait
/// cannot starve the watchdog.
fn start_updater() {
    HEARTBEAT.store(sdk::uptime_ms(), Ordering::Release);
    let created = sdk::task_create(
        c"petlibre-ota",
        UPDATER_STACK_WORDS,
        2,
        updater_task,
        ptr::null_mut(),
    );
    sdk::log(if created {
        c"PLOTA1 tasks created=1\r\n"
    } else {
        c"PLOTA1 tasks created=0\r\n"
    });
}

/// FW11 trial test, in place of the optics probe: stop feeding the
/// watchdog, as a hung candidate would, so KM0 counts the reset. Outputs
/// stay stopped.
#[cfg(feature = "fw11-reset")]
fn start_optics() {
    RESTART.store(true, Ordering::Release);
    sdk::log(if WATCHDOG_OPEN.load(Ordering::Acquire) {
        c"PLTRL1 test=reset watchdog=1\r\n"
    } else {
        c"PLTRL1 test=reset watchdog=0\r\n"
    });
}

fn bit(value: bool) -> u8 {
    u8::from(value)
}

fn kind(kind: u32) -> &'static str {
    match kind {
        1 => "on",
        2 => "scan",
        3 => "connect",
        4 => "dhcp",
        5 => "lost",
        6 => "backoff",
        7 => "dns",
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
    };

    if let Ok(line) = line {
        sdk::log(line);
    }
}

fn probe_step() -> u32 {
    // SAFETY: only this task touches the slot.
    let probe = unsafe { PROBE.get_or_init(Probe::default) };
    let step = probe.step();
    if let Some(record) = step.record {
        log_probe(record);
    }
    step.delay_ms
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

    sdk::log(c"\r\nPetlibre local firmware: Rust bringup\r\n");
    let mut report = startup::Report::new();
    if let Ok(line) = report.start(startup::snapshot()) {
        sdk::log(line);
    }

    // FW11 test variant: a determined startup failure, which a trial image
    // must turn into a counted reset. Opaque to the optimizer, or LTO drops
    // every path after a passing diagnostic and the variant no longer links
    // the image the link audit pins.
    let forced_fail = cfg!(feature = "fw11-fail") && core::hint::black_box(true);
    let passed = petlibre_core::diagnostic::run() && !forced_fail;
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

    // The indicators stay LOW (policy low-only-v1): the FW04 pulse probe
    // proved them and cost every boot about 15 s; FW23 gives them meaning.
    if !(leds::all_low() && stopped::observe().verified()) {
        sdk::log(c"PLLED1 error finish=unverified\r\n");
        return startup_failed();
    }
    sdk::log(c"PLLED1 begin low_only\r\n");
    sdk::log(c"PLLED1 done low=all\r\n");

    PROBING.store(true, Ordering::Relaxed);
    if startup::snapshot().bank() == Some(0) {
        start_updater();
    } else {
        start_transport();
        start_optics();
        start_confirm();
        start_audio();
        start_stream();
    }
    STARTUP.store(STARTUP_STARTED, Ordering::Release);
    1
}

/// Called once by the SDK's `example_entry`, before the scheduler starts.
#[unsafe(no_mangle)]
pub extern "C" fn petlibre_app_start() {
    let snapshot = stopped::initialize();
    // FW11: supervise from the first scheduled step, so a startup fault or
    // hang is a reset KM0 can count.
    if !sdk::task_create(c"petlibre-wdg", 512, 3, feeder_task, ptr::null_mut()) {
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

    if !sdk::task_create(c"petlibre", 1024, 1, diagnostic_task, ptr::null_mut()) {
        sdk::log(c"Petlibre: diagnostic task creation failed\r\n");
        sdk::log(c"PLFW1 error task=create\r\n");
        startup_failed();
    }
}
