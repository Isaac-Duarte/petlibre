//! KM4 application entry: logs and runs the synthetic diagnostic once.
//! Stopped-output startup and a bounded indicator probe, then the FW07 stage
//! B probe: radio on and scan only, networks reported by SSID CRC.

use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::probes::wifi::{Probe, Record};
use crate::startup;
use crate::{leds, sdk, stopped};

static PHASE: AtomicU32 = AtomicU32::new(0);
static INPUT_MISMATCH: AtomicBool = AtomicBool::new(false);
static PROBING: AtomicBool = AtomicBool::new(false);

/// Fixed-address probe storage touched only by the diagnostic task.
struct Slot<T>(UnsafeCell<Option<T>>);
// SAFETY: the diagnostic task is the only user; see `probe_step`.
unsafe impl<T> Sync for Slot<T> {}
static PROBE: Slot<Probe> = Slot(UnsafeCell::new(None));

fn bit(value: bool) -> u8 {
    u8::from(value)
}

fn log_probe(record: Record) {
    let mut report = startup::Report::new();
    let line = match record {
        Record::Start { created } => {
            report.record(format_args!("PLWIF1 start created={}\r\n", bit(created)))
        }
        Record::On { result, ms } => {
            report.record(format_args!("PLWIF1 on result={result} ms={ms}\r\n"))
        }
        Record::Scan {
            n,
            result,
            ms,
            count,
            dropped,
            stopped,
        } => report.record(format_args!(
            "PLWIF1 scan n={n} result={result} ms={ms} count={count} dropped={dropped} stopped={}\r\n",
            bit(stopped)
        )),
        Record::Ap { n, index, ap } => report.record(format_args!(
            "PLWIF1 ap n={n} i={index} ssid_crc={:08x} rssi={} channel={} security={:08x}\r\n",
            ap.ssid_crc, ap.rssi, ap.channel, ap.security
        )),
        Record::Done { scans } => report.record(format_args!("PLWIF1 done scans={scans}\r\n")),
        Record::Timeout { stage, ms } => report.record(format_args!(
            "PLWIF1 error timeout stage={stage} ms={ms}\r\n"
        )),
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
    sdk::log(c"\r\nWellfed local firmware: Rust bringup\r\n");
    let mut report = startup::Report::new();
    if let Ok(line) = report.start(startup::snapshot()) {
        sdk::log(line);
    }
    let passed = wellfed_core::diagnostic::run();
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
pub extern "C" fn wellfed_app_start() {
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
    if !sdk::task_create(c"wellfed", 1024, 1, diagnostic_task, ptr::null_mut()) {
        sdk::log(c"Wellfed: diagnostic task creation failed\r\n");
        sdk::log(c"PLFW1 error task=create\r\n");
    }
}
