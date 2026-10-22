//! KM4 application entry: logs and runs the synthetic diagnostic once.
//! Stopped-output startup and a bounded indicator probe, then the FW07 stage
//! D probe: join the configured network, check DNS and keep both, logging the
//! shim's events. Beside it, the FW08 transport task reports health to the
//! local server's broker, and the FW13 optics task exercises the emitters and
//! reads the food, outlet and feedback inputs, motor STOP throughout.
//!
//! Booted from bank 0 (the recovery role), the FW10 updater runs in place of
//! the transport and optics tasks, with a watchdog feeder beside it.

use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::flash::hooks::{self, HookProvider, HookTicket};
use crate::flash::park::Hardware;
use crate::health::Boot;
use crate::probes::flash::Provider;
use crate::probes::optics;
use crate::probes::wifi::{Probe, Record};
use crate::transport::{self, Link};
use crate::updater::{self, Hooked, Hooks, Updater};
use crate::watchdog::Watchdog;
use crate::{config, leds, sdk, startup, stopped};

static PHASE: AtomicU32 = AtomicU32::new(0);
static INPUT_MISMATCH: AtomicBool = AtomicBool::new(false);
static PROBING: AtomicBool = AtomicBool::new(false);

/// Fixed-address probe storage touched only by the diagnostic task.
struct Slot<T>(UnsafeCell<Option<T>>);
// SAFETY: the diagnostic task is the only user; see `probe_step`.
unsafe impl<T> Sync for Slot<T> {}
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
    let probe = unsafe { &mut *OPTICS.0.get() }.get_or_insert_with(optics::Probe::default);
    let step = probe.step::<Hardware>();
    if let Some(record) = step.record {
        let mut report = startup::Report::new();
        if let Ok(line) = report.record(format_args!("PLOPT1 {}\r\n", OpticsLine(record))) {
            sdk::log(line);
        }
    }
    step.delay_ms
}

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
    let probe = unsafe { &mut *PULSE.0.get() }.get_or_insert_with(Default::default);
    let step = probe.step::<Hardware>();
    if let Some(record) = step.record {
        let mut report = startup::Report::new();
        if let Ok(line) = report.record(format_args!("PLMOT1 {}\r\n", PulseLine(record))) {
            sdk::log(line);
        }
    }
    step.delay_ms
}

/// FW16 builds run the motor pulse in place of the optics probe.
#[cfg(feature = "fw16-pulse")]
fn start_optics() {
    let created = sdk::task_create(c"petlibre-pulse", 1024, 3, pulse_task, ptr::null_mut());
    sdk::log(if created {
        c"PLMOT1 start created=1\r\n"
    } else {
        c"PLMOT1 start created=0\r\n"
    });
}

/// Above the diagnostic and transport tasks, so their logging cannot hold
/// off the owner lease the supervisor enforces.
#[cfg(not(feature = "fw16-pulse"))]
fn start_optics() {
    let created = sdk::task_create(c"petlibre-optics", 1024, 3, optics_task, ptr::null_mut());
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
    let slot = unsafe { &mut *NET.0.get() };
    if slot.is_none() {
        let Some(config) = sdk::transport_config() else {
            sdk::log(c"PLMQT1 error config\r\n");
            return 0;
        };
        *slot = Some(Link::new(config, Boot::read(startup::snapshot())));
    }
    slot.as_mut()
        .map_or(0, |link| link.step(&mut log_transport))
}

fn start_transport() {
    let created = sdk::task_create(c"petlibre-net", 1536, 2, net_task, ptr::null_mut());
    sdk::log(if created {
        c"PLMQT1 start created=1\r\n"
    } else {
        c"PLMQT1 start created=0\r\n"
    });
}

/// Touched only by the updater task, except that the armed hooks reach the
/// provider through its context pointer.
static UPDATER: Slot<Updater> = Slot(UnsafeCell::new(None));
static PROVIDER: Slot<Provider> = Slot(UnsafeCell::new(None));
/// Opened by the updater task, then touched only by the feeder task.
static WATCHDOG: Slot<Watchdog> = Slot(UnsafeCell::new(None));
static WATCHDOG_OPEN: AtomicBool = AtomicBool::new(false);
/// The updater's last step, in uptime milliseconds.
static HEARTBEAT: AtomicU32 = AtomicU32::new(0);
/// The updater ended without activating: feed while outputs stay stopped.
static UPDATER_DONE: AtomicBool = AtomicBool::new(false);
/// Bank 1 is activated: stop feeding so the watchdog resets the platform.
static RESTART: AtomicBool = AtomicBool::new(false);
/// An updater step longer than this (a hang) stops the feeding.
const LIVENESS_MS: u32 = 10_000;
const FEED_MS: u32 = 250;

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
        // SAFETY: the feeder touches the slot only once WATCHDOG_OPEN is set.
        let watchdog = unsafe { &mut *WATCHDOG.0.get() }.get_or_insert_with(Watchdog::default);
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
    let slot = unsafe { &mut *UPDATER.0.get() };
    if slot.is_none() {
        let Some(config) = sdk::transport_config() else {
            sdk::log(c"PLOTA1 error stage=config\r\n");
            UPDATER_DONE.store(true, Ordering::Release);
            return 0;
        };
        *slot = Some(Updater::new(config, updater::public_key()));
    }
    let Some(updater) = slot.as_mut() else {
        return 0;
    };
    let provider =
        unsafe { &mut *PROVIDER.0.get() }.get_or_insert_with(Provider::default) as *mut Provider;
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

/// Feeds the watchdog while the updater keeps stepping (or has ended) and
/// outputs stay stopped. A hung updater, a hook fault (IRQs masked) or an
/// activation all stop the feeding, and the watchdog resets the platform.
unsafe extern "C" fn feeder_task(_: *mut c_void) -> u32 {
    static PROGRESS: AtomicU32 = AtomicU32::new(0);
    if !WATCHDOG_OPEN.load(Ordering::Acquire) || RESTART.load(Ordering::Acquire) {
        return FEED_MS;
    }
    let beat = HEARTBEAT.load(Ordering::Acquire);
    let alive =
        UPDATER_DONE.load(Ordering::Acquire) || sdk::uptime_ms().wrapping_sub(beat) < LIVENESS_MS;
    if alive && stopped::observe().verified() {
        // SAFETY: the updater task no longer touches the slot once open.
        if let Some(watchdog) = unsafe { &mut *WATCHDOG.0.get() }.as_mut() {
            let progress = PROGRESS.fetch_add(1, Ordering::Relaxed) + 1;
            let _ = watchdog.checkpoint(u64::from(progress));
        }
    }
    FEED_MS
}

/// The recovery role: the updater, and the feeder above it so a long
/// network wait cannot starve the watchdog.
fn start_updater() {
    let created = sdk::task_create(c"petlibre-ota", 2560, 2, updater_task, ptr::null_mut())
        && sdk::task_create(c"petlibre-wdg", 512, 3, feeder_task, ptr::null_mut());
    sdk::log(if created {
        c"PLOTA1 tasks created=1\r\n"
    } else {
        c"PLOTA1 tasks created=0\r\n"
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
    let probe = unsafe { &mut *PROBE.0.get() }.get_or_insert_with(Probe::default);
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

fn abort_led(led: leds::Led, high: bool, stage: &str, value: leds::Readback) {
    let _ = leds::all_low();
    if !value.stopped_ok {
        let _ = stopped::initialize();
    }
    log_led_readback(led, high, stage, value);
    sdk::log(c"PLLED1 error step=unverified\r\n");
}

unsafe extern "C" fn diagnostic_task(_: *mut c_void) -> u32 {
    if PROBING.load(Ordering::Relaxed) {
        return probe_step();
    }
    let phase = PHASE.fetch_add(1, Ordering::Relaxed);
    if phase != 0 {
        let Some(step) = leds::probe_step((phase - 1) / 2) else {
            if leds::all_low() && stopped::observe().verified() {
                if INPUT_MISMATCH.load(Ordering::Relaxed) {
                    sdk::log(c"PLLED1 error finish=input_mismatch low=all\r\n");
                } else {
                    sdk::log(c"PLLED1 done low=all\r\n");
                    PROBING.store(true, Ordering::Relaxed);
                    if startup::snapshot().bank() == Some(0) {
                        start_updater();
                    } else {
                        start_transport();
                        start_optics();
                    }
                    return 1;
                }
            } else {
                sdk::log(c"PLLED1 error finish=unverified\r\n");
            }
            return 0;
        };
        if (phase - 1).is_multiple_of(2) {
            if !leds::request(step.led, step.high) {
                abort_led(step.led, step.high, "request", leds::observe(step.led));
                return 0;
            }
            // Yield with IRQs restored; do not inspect pad input immediately after DR.
            return leds::SETTLE_MS;
        }
        if step.high {
            log_gpio_routing(step.led.name());
        }
        let observed = leds::observe(step.led);
        let status = match observed.decision(step.high) {
            leds::Decision::Verified => "pass",
            leds::Decision::OutputOnly => "output",
            leds::Decision::InputMismatch => {
                INPUT_MISMATCH.store(true, Ordering::Relaxed);
                log_led_readback(step.led, step.high, "verify", observed);
                "fail"
            }
            leds::Decision::Abort => {
                abort_led(step.led, step.high, "verify", observed);
                return 0;
            }
        };
        let mut report = startup::Report::new();
        if let Ok(line) = report.record(format_args!(
            "PLLED1 pin={} level={} status={}\r\n",
            step.led.name(),
            u8::from(step.high),
            status,
        )) {
            sdk::log(line);
        }
        return step.delay_ms - leds::SETTLE_MS;
    }
    sdk::log(c"\r\nPetlibre local firmware: Rust bringup\r\n");
    let mut report = startup::Report::new();
    if let Ok(line) = report.start(startup::snapshot()) {
        sdk::log(line);
    }
    let passed = petlibre_core::diagnostic::run();
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
        return 0;
    }
    sdk::log(c"PLLED1 begin output_only=PA0,PA2,PA4\r\n");
    3000
}

/// Called once by the SDK's `example_entry`, before the scheduler starts.
#[unsafe(no_mangle)]
pub extern "C" fn petlibre_app_start() {
    let snapshot = stopped::initialize();
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
        return;
    }
    if !sdk::task_create(c"petlibre", 1024, 1, diagnostic_task, ptr::null_mut()) {
        sdk::log(c"Petlibre: diagnostic task creation failed\r\n");
        sdk::log(c"PLFW1 error task=create\r\n");
    }
}
